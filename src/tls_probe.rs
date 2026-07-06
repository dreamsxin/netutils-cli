//! TLS handshake and certificate diagnostics.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;
use tokio_rustls::TlsConnector;
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
use trust_dns_resolver::TokioAsyncResolver;
use x509_parser::prelude::*;

use crate::output::{print_json, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
pub struct TlsReport {
    pub target: String,
    pub host: String,
    pub port: u16,
    pub sni: String,
    pub resolved_ips: Vec<String>,
    pub route: Option<TlsRoute>,
    pub success: bool,
    pub error: Option<String>,
    pub timings: TlsTimings,
    pub tls: Option<TlsInfo>,
    pub certificates: Vec<CertificateInfo>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TlsRoute {
    pub target_ip: String,
    pub interface: Option<String>,
    pub gateway: Option<String>,
    pub source: String,
}

#[derive(Debug, Default, Serialize)]
pub struct TlsTimings {
    pub dns_ms: f64,
    pub route_ms: Option<f64>,
    pub tcp_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    pub total_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct TlsInfo {
    pub connected_ip: String,
    pub protocol_version: Option<String>,
    pub cipher_suite: Option<String>,
    pub alpn: Option<String>,
    pub certificate_count: usize,
}

#[derive(Debug, Serialize)]
pub struct CertificateInfo {
    pub position: usize,
    pub subject: Option<String>,
    pub issuer: Option<String>,
    pub not_before: Option<String>,
    pub not_after: Option<String>,
    pub dns_names: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    host: String,
    port: u16,
}

pub async fn run(
    target: &str,
    port_override: Option<u16>,
    sni_override: Option<String>,
    timeout: Duration,
    alpn: &str,
    mode: OutputMode,
) {
    let parsed = match parse_target(target) {
        Some(mut parsed) => {
            if let Some(port) = port_override {
                parsed.port = port;
            }
            parsed
        }
        None => {
            let report = error_report(target, "invalid target");
            output(report, mode);
            return;
        }
    };

    let sni = sni_override.unwrap_or_else(|| parsed.host.clone());
    let alpn_protocols = parse_alpn(alpn);
    let total_start = Instant::now();

    let dns_start = Instant::now();
    let ips = resolve_fast(&parsed.host, timeout).await;
    let dns_ms = dns_start.elapsed().as_secs_f64() * 1000.0;
    if ips.is_empty() {
        let report = TlsReport {
            target: target.to_string(),
            host: parsed.host,
            port: parsed.port,
            sni,
            resolved_ips: Vec::new(),
            route: None,
            success: false,
            error: Some("DNS resolve failed".to_string()),
            timings: TlsTimings {
                dns_ms,
                total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
                ..Default::default()
            },
            tls: None,
            certificates: Vec::new(),
            notes: notes(),
        };
        output(report, mode);
        return;
    }

    let route_start = Instant::now();
    let route = ips.first().and_then(|ip| {
        crate::route_probe::route_to_target(&ip.to_string()).map(|route| TlsRoute {
            target_ip: ip.to_string(),
            interface: route.interface,
            gateway: route.gateway,
            source: route.source,
        })
    });
    let route_ms = route_start.elapsed().as_secs_f64() * 1000.0;

    let tcp_start = Instant::now();
    let mut last_error = None;
    let mut tcp_stream = None;
    let mut connected_ip = None;
    for ip in ips.iter().copied().take(8) {
        let addr = SocketAddr::new(ip, parsed.port);
        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                tcp_stream = Some(stream);
                connected_ip = Some(ip);
                break;
            }
            Ok(Err(err)) => last_error = Some(format!("TCP: {err}")),
            Err(_) => last_error = Some("TCP: timeout".to_string()),
        }
    }
    let tcp_ms = tcp_start.elapsed().as_secs_f64() * 1000.0;
    let Some(tcp_stream) = tcp_stream else {
        let report = TlsReport {
            target: target.to_string(),
            host: parsed.host,
            port: parsed.port,
            sni,
            resolved_ips: ips.iter().map(IpAddr::to_string).collect(),
            route,
            success: false,
            error: last_error.or_else(|| Some("TCP: failed".to_string())),
            timings: TlsTimings {
                dns_ms,
                route_ms: Some(route_ms),
                tcp_ms: Some(tcp_ms),
                total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
                ..Default::default()
            },
            tls: None,
            certificates: Vec::new(),
            notes: notes(),
        };
        output(report, mode);
        return;
    };

    let tls_start = Instant::now();
    let connector = match build_connector(&alpn_protocols) {
        Ok(connector) => connector,
        Err(err) => {
            let report = TlsReport {
                target: target.to_string(),
                host: parsed.host,
                port: parsed.port,
                sni,
                resolved_ips: ips.iter().map(IpAddr::to_string).collect(),
                route,
                success: false,
                error: Some(format!("TLS config: {err}")),
                timings: TlsTimings {
                    dns_ms,
                    route_ms: Some(route_ms),
                    tcp_ms: Some(tcp_ms),
                    total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
                    ..Default::default()
                },
                tls: None,
                certificates: Vec::new(),
                notes: notes(),
            };
            output(report, mode);
            return;
        }
    };

    let server_name = match rustls::pki_types::ServerName::try_from(sni.clone()) {
        Ok(name) => name,
        Err(err) => {
            let report = TlsReport {
                target: target.to_string(),
                host: parsed.host,
                port: parsed.port,
                sni,
                resolved_ips: ips.iter().map(IpAddr::to_string).collect(),
                route,
                success: false,
                error: Some(format!("TLS SNI: {err}")),
                timings: TlsTimings {
                    dns_ms,
                    route_ms: Some(route_ms),
                    tcp_ms: Some(tcp_ms),
                    total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
                    ..Default::default()
                },
                tls: None,
                certificates: Vec::new(),
                notes: notes(),
            };
            output(report, mode);
            return;
        }
    };

    let tls_stream =
        match tokio::time::timeout(timeout, connector.connect(server_name, tcp_stream)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => {
                let report = TlsReport {
                    target: target.to_string(),
                    host: parsed.host,
                    port: parsed.port,
                    sni,
                    resolved_ips: ips.iter().map(IpAddr::to_string).collect(),
                    route,
                    success: false,
                    error: Some(format!("TLS: {err}")),
                    timings: TlsTimings {
                        dns_ms,
                        route_ms: Some(route_ms),
                        tcp_ms: Some(tcp_ms),
                        tls_ms: Some(tls_start.elapsed().as_secs_f64() * 1000.0),
                        total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
                    },
                    tls: None,
                    certificates: Vec::new(),
                    notes: notes(),
                };
                output(report, mode);
                return;
            }
            Err(_) => {
                let report = TlsReport {
                    target: target.to_string(),
                    host: parsed.host,
                    port: parsed.port,
                    sni,
                    resolved_ips: ips.iter().map(IpAddr::to_string).collect(),
                    route,
                    success: false,
                    error: Some("TLS: timeout".to_string()),
                    timings: TlsTimings {
                        dns_ms,
                        route_ms: Some(route_ms),
                        tcp_ms: Some(tcp_ms),
                        tls_ms: Some(tls_start.elapsed().as_secs_f64() * 1000.0),
                        total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
                    },
                    tls: None,
                    certificates: Vec::new(),
                    notes: notes(),
                };
                output(report, mode);
                return;
            }
        };
    let tls_ms = tls_start.elapsed().as_secs_f64() * 1000.0;

    let (_, connection) = tls_stream.get_ref();
    let peer_certs = connection.peer_certificates().unwrap_or_default();
    let certs = peer_certs
        .iter()
        .enumerate()
        .map(|(idx, cert)| parse_certificate(idx + 1, cert.as_ref()))
        .collect::<Vec<_>>();
    let tls = TlsInfo {
        connected_ip: connected_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "--".to_string()),
        protocol_version: connection
            .protocol_version()
            .map(|version| format!("{version:?}")),
        cipher_suite: connection
            .negotiated_cipher_suite()
            .map(|suite| format!("{:?}", suite.suite())),
        alpn: connection
            .alpn_protocol()
            .map(|proto| String::from_utf8_lossy(proto).to_string()),
        certificate_count: peer_certs.len(),
    };

    let report = TlsReport {
        target: target.to_string(),
        host: parsed.host,
        port: parsed.port,
        sni,
        resolved_ips: ips.iter().map(IpAddr::to_string).collect(),
        route,
        success: true,
        error: None,
        timings: TlsTimings {
            dns_ms,
            route_ms: Some(route_ms),
            tcp_ms: Some(tcp_ms),
            tls_ms: Some(tls_ms),
            total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
        },
        tls: Some(tls),
        certificates: certs,
        notes: notes(),
    };
    output(report, mode);
}

fn build_connector(alpn_protocols: &[Vec<u8>]) -> Result<TlsConnector, rustls::Error> {
    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.iter().cloned().collect(),
    };
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(root_store)
    .with_no_client_auth();
    config.alpn_protocols = alpn_protocols.to_vec();
    Ok(TlsConnector::from(Arc::new(config)))
}

async fn resolve_fast(host: &str, timeout: Duration) -> Vec<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return vec![ip];
    }
    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());
    match tokio::time::timeout(timeout, resolver.lookup_ip(host)).await {
        Ok(Ok(ips)) => dedup_ips(ips.iter().collect()),
        Ok(Err(_)) | Err(_) => Vec::new(),
    }
}

fn dedup_ips(ips: Vec<IpAddr>) -> Vec<IpAddr> {
    let mut seen = std::collections::HashSet::new();
    ips.into_iter().filter(|ip| seen.insert(*ip)).collect()
}

fn parse_certificate(position: usize, der: &[u8]) -> CertificateInfo {
    match X509Certificate::from_der(der) {
        Ok((_, cert)) => {
            let dns_names = cert
                .subject_alternative_name()
                .ok()
                .flatten()
                .map(|san| {
                    san.value
                        .general_names
                        .iter()
                        .filter_map(|name| match name {
                            GeneralName::DNSName(name) => Some(name.to_string()),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            CertificateInfo {
                position,
                subject: Some(cert.subject().to_string()),
                issuer: Some(cert.issuer().to_string()),
                not_before: Some(cert.validity().not_before.to_string()),
                not_after: Some(cert.validity().not_after.to_string()),
                dns_names,
                error: None,
            }
        }
        Err(err) => CertificateInfo {
            position,
            subject: None,
            issuer: None,
            not_before: None,
            not_after: None,
            dns_names: Vec::new(),
            error: Some(err.to_string()),
        },
    }
}

fn parse_target(input: &str) -> Option<Target> {
    let without_scheme = input
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(input);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    if authority.is_empty() {
        return None;
    }
    parse_authority(authority)
}

fn parse_authority(authority: &str) -> Option<Target> {
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(host) = authority.strip_prefix('[') {
        let (host, after) = host.split_once(']')?;
        let port = after
            .strip_prefix(':')
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(443);
        return Some(Target {
            host: host.to_string(),
            port,
        });
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() && !host.is_empty() => Some(Target {
            host: host.to_string(),
            port: port.parse().ok()?,
        }),
        _ => Some(Target {
            host: authority.to_string(),
            port: 443,
        })
        .filter(|target| !target.host.is_empty()),
    }
}

fn parse_alpn(input: &str) -> Vec<Vec<u8>> {
    input
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.as_bytes().to_vec())
        .collect()
}

fn error_report(target: &str, error: &str) -> TlsReport {
    TlsReport {
        target: target.to_string(),
        host: String::new(),
        port: 443,
        sni: String::new(),
        resolved_ips: Vec::new(),
        route: None,
        success: false,
        error: Some(error.to_string()),
        timings: TlsTimings::default(),
        tls: None,
        certificates: Vec::new(),
        notes: notes(),
    }
}

fn notes() -> Vec<String> {
    vec![
        "This command performs a direct TLS handshake from the local host; proxy/TUN clients may still hide downstream routing.".to_string(),
        "Certificate details come from the chain returned by the server during this handshake.".to_string(),
    ]
}

fn output(report: TlsReport, mode: OutputMode) {
    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn print_report(report: &TlsReport) {
    println!();
    println!("{}", "🔐 TLS Probe".bold());
    println!("  Target: {}", report.target);
    println!("  Host: {}:{}", report.host, report.port);
    println!("  SNI: {}", report.sni);

    println!();
    println!("{}", "DNS".bold());
    if report.resolved_ips.is_empty() {
        println!("  <resolve failed>");
    } else {
        println!("  {}", report.resolved_ips.join(", "));
    }

    println!();
    println!("{}", "Route".bold());
    if let Some(route) = &report.route {
        println!("  Target IP: {}", route.target_ip);
        println!(
            "  Interface: {}",
            route.interface.as_deref().unwrap_or("--")
        );
        println!("  Gateway: {}", route.gateway.as_deref().unwrap_or("--"));
        println!("  Source: {}", route.source);
    } else {
        println!("  <not available>");
    }

    println!();
    println!("{}", "Handshake".bold());
    println!(
        "  Result: {}",
        if report.success {
            "ok".green().to_string()
        } else {
            "failed".red().to_string()
        }
    );
    if let Some(error) = &report.error {
        println!("  Error: {}", error);
    }
    let rows = vec![
        vec!["DNS".to_string(), format!("{:.2}ms", report.timings.dns_ms)],
        vec!["Route".to_string(), fmt_ms(report.timings.route_ms)],
        vec!["TCP".to_string(), fmt_ms(report.timings.tcp_ms)],
        vec!["TLS".to_string(), fmt_ms(report.timings.tls_ms)],
        vec![
            "Total".to_string(),
            format!("{:.2}ms", report.timings.total_ms),
        ],
    ];
    print_table(&["Phase", "Time"], &rows);

    if let Some(tls) = &report.tls {
        println!();
        println!("{}", "TLS".bold());
        println!("  Connected IP: {}", tls.connected_ip);
        println!(
            "  Version: {}",
            tls.protocol_version.as_deref().unwrap_or("--")
        );
        println!("  Cipher: {}", tls.cipher_suite.as_deref().unwrap_or("--"));
        println!("  ALPN: {}", tls.alpn.as_deref().unwrap_or("--"));
        println!("  Certificates: {}", tls.certificate_count);
    }

    if !report.certificates.is_empty() {
        println!();
        println!("{}", "Certificates".bold());
        let rows = report
            .certificates
            .iter()
            .map(|cert| {
                vec![
                    cert.position.to_string(),
                    cert.subject.clone().unwrap_or_else(|| "--".to_string()),
                    cert.issuer.clone().unwrap_or_else(|| "--".to_string()),
                    cert.not_after.clone().unwrap_or_else(|| "--".to_string()),
                    cert.dns_names
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    cert.error.clone().unwrap_or_else(|| "--".to_string()),
                ]
            })
            .collect::<Vec<_>>();
        print_table(
            &["#", "Subject", "Issuer", "Not After", "DNS Names", "Error"],
            &rows,
        );
    }

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn fmt_ms(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}ms"))
        .unwrap_or_else(|| "--".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_host() {
        assert_eq!(
            parse_target("example.com"),
            Some(Target {
                host: "example.com".to_string(),
                port: 443
            })
        );
    }

    #[test]
    fn parses_host_port_and_url() {
        assert_eq!(
            parse_target("https://example.com:8443/path"),
            Some(Target {
                host: "example.com".to_string(),
                port: 8443
            })
        );
    }

    #[test]
    fn parses_ipv6_authority() {
        assert_eq!(
            parse_target("[2001:db8::1]:443"),
            Some(Target {
                host: "2001:db8::1".to_string(),
                port: 443
            })
        );
    }

    #[test]
    fn parses_alpn_list() {
        assert_eq!(
            parse_alpn("h2, http/1.1"),
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }
}
