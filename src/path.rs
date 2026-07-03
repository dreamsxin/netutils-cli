//! HTTP 请求路径分析：DNS、代理、出口、traceroute、TCP/TLS/HTTP timing。

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::output::{print_json, print_json_error, OutputMode};
use crate::table::print_table;

const MAX_REDIRECTS: usize = 5;

#[derive(Serialize)]
pub struct PathReport {
    pub input: String,
    pub final_url: String,
    pub dns: DnsPath,
    pub proxy: ProxyPath,
    pub egress: Option<EgressPath>,
    pub redirects: Vec<RedirectHop>,
    pub network_path: Vec<TraceHopPath>,
    pub http: HttpPath,
    pub note: String,
}

#[derive(Serialize)]
pub struct DnsPath {
    pub host: String,
    pub ips: Vec<String>,
}

#[derive(Serialize)]
pub struct ProxyPath {
    pub mode: String,
    pub value: Option<String>,
}

#[derive(Serialize)]
pub struct EgressPath {
    pub interface: String,
    pub ip: String,
    pub iftype: String,
    pub tun_mode: bool,
}

#[derive(Serialize)]
pub struct RedirectHop {
    pub url: String,
    pub status: u16,
    pub location: Option<String>,
}

#[derive(Serialize)]
pub struct TraceHopPath {
    pub ttl: u32,
    pub ip: Option<String>,
    pub rtt_ms: Option<f64>,
}

#[derive(Serialize)]
pub struct HttpPath {
    pub url: String,
    pub status: Option<u16>,
    pub success: bool,
    pub error: Option<String>,
    pub timing: TimingPath,
}

#[derive(Default, Serialize)]
pub struct TimingPath {
    pub dns_ms: Option<f64>,
    pub proxy_connect_ms: Option<f64>,
    pub connect_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    pub ttfb_ms: Option<f64>,
    pub total_ms: f64,
}

#[derive(Clone)]
struct ParsedUrl {
    scheme: String,
    host: String,
    port: u16,
    path: String,
    is_https: bool,
    normalized: String,
}

pub async fn run(
    input: &str,
    max_hops: u32,
    timeout: Duration,
    proxy: Option<String>,
    no_proxy: bool,
    mode: OutputMode,
) {
    let start_url = normalize_url(input);
    let parsed = match parse_url(&start_url) {
        Some(parsed) => parsed,
        None => {
            let msg = format!("invalid URL: {}", input);
            if mode == OutputMode::Json {
                print_json_error(&msg);
            } else {
                println!("  {}", msg.red());
            }
            return;
        }
    };

    let proxy_value = if no_proxy {
        None
    } else {
        proxy.or_else(crate::util::get_system_proxy_addr)
    };
    let proxy_path = ProxyPath {
        mode: if no_proxy {
            "direct-forced".to_string()
        } else if proxy_value.is_some() {
            "proxy".to_string()
        } else {
            "direct".to_string()
        },
        value: proxy_value.clone(),
    };

    let client = match build_client(timeout, proxy_value.as_deref()) {
        Ok(client) => client,
        Err(err) => {
            let msg = format!("failed to build HTTP client: {}", err);
            if mode == OutputMode::Json {
                print_json_error(&msg);
            } else {
                println!("  {}", msg.red());
            }
            return;
        }
    };

    let (redirects, final_url) = collect_redirects(&client, &parsed.normalized).await;
    let final_parsed = parse_url(&final_url).unwrap_or(parsed);
    let dns_start = Instant::now();
    let ips = crate::util::resolve_host_all(&final_parsed.host).await;
    let dns_ms = dns_start.elapsed().as_secs_f64() * 1000.0;

    let interfaces = crate::info::collect_interfaces();
    let egress = crate::info::collect_egress(&interfaces).map(|egress| EgressPath {
        interface: egress.interface,
        ip: egress.ip,
        iftype: egress.iftype,
        tun_mode: egress.tun_mode,
    });

    let trace_target = ips.first().copied();
    let network_path = match trace_target {
        Some(target) => collect_network_path(target, max_hops).await,
        None => Vec::new(),
    };

    let http = if proxy_value.is_some() {
        reqwest_timing(
            &client,
            &final_parsed.normalized,
            proxy_value.as_deref(),
            timeout,
        )
        .await
    } else {
        manual_timing(&final_parsed, &ips, timeout, dns_ms).await
    };

    let report = PathReport {
        input: input.to_string(),
        final_url: final_parsed.normalized,
        dns: DnsPath {
            host: final_parsed.host,
            ips: ips.iter().map(IpAddr::to_string).collect(),
        },
        proxy: proxy_path,
        egress,
        redirects,
        network_path,
        http,
        note: "Local perspective only; proxy/TUN/CDN paths may hide downstream hops.".to_string(),
    };

    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn normalize_url(input: &str) -> String {
    if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{}", input)
    }
}

fn parse_url(url: &str) -> Option<ParsedUrl> {
    let (scheme, rest) = url.split_once("://")?;
    let is_https = scheme.eq_ignore_ascii_case("https");
    if !is_https && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }

    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return None;
    }

    let (host, port) = if let Some(host) = authority.strip_prefix('[') {
        let (host, after) = host.split_once(']')?;
        let port = after
            .strip_prefix(':')
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(if is_https { 443 } else { 80 });
        (host.to_string(), port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        match port.parse::<u16>() {
            Ok(port) => (host.to_string(), port),
            Err(_) => (authority.to_string(), if is_https { 443 } else { 80 }),
        }
    } else {
        (authority.to_string(), if is_https { 443 } else { 80 })
    };

    let normalized = format!("{}://{}{}", scheme, authority, path);
    Some(ParsedUrl {
        scheme: scheme.to_string(),
        host,
        port,
        path: path.to_string(),
        is_https,
        normalized,
    })
}

fn build_client(timeout: Duration, proxy: Option<&str>) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    if let Some(proxy_url) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy_url)?);
    } else {
        builder = builder.no_proxy();
    }
    builder.build()
}

async fn collect_redirects(
    client: &reqwest::Client,
    start_url: &str,
) -> (Vec<RedirectHop>, String) {
    let mut redirects = Vec::new();
    let mut current = start_url.to_string();

    for _ in 0..MAX_REDIRECTS {
        let response = match client.get(&current).send().await {
            Ok(response) => response,
            Err(_) => break,
        };
        let status = response.status();
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(|v| resolve_location(&current, v));

        redirects.push(RedirectHop {
            url: current.clone(),
            status: status.as_u16(),
            location: location.clone(),
        });

        if !status.is_redirection() {
            break;
        }
        match location {
            Some(next) => current = next,
            None => break,
        }
    }

    (redirects, current)
}

fn resolve_location(current: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    let Some(parsed) = parse_url(current) else {
        return location.to_string();
    };
    if location.starts_with('/') {
        format!("{}://{}{}", parsed.scheme, parsed.host, location)
    } else {
        let base = parsed
            .path
            .rsplit_once('/')
            .map(|(base, _)| base)
            .unwrap_or("");
        format!(
            "{}://{}/{}{}",
            parsed.scheme,
            parsed.host,
            base.trim_start_matches('/'),
            location
        )
    }
}

async fn collect_network_path(target: IpAddr, max_hops: u32) -> Vec<TraceHopPath> {
    crate::traceroute::collect_trace_quick(target, max_hops)
        .await
        .into_iter()
        .map(|hop| {
            let probe = hop.probes.iter().find(|p| p.ip.is_some());
            TraceHopPath {
                ttl: hop.ttl,
                ip: probe.and_then(|p| p.ip.clone()),
                rtt_ms: probe.and_then(|p| p.rtt_ms),
            }
        })
        .collect()
}

async fn reqwest_timing(
    client: &reqwest::Client,
    url: &str,
    proxy: Option<&str>,
    timeout: Duration,
) -> HttpPath {
    let start = Instant::now();
    let proxy_connect_ms = match proxy {
        Some(proxy) => measure_proxy_connect(proxy, timeout).await,
        None => None,
    };
    match client.get(url).send().await {
        Ok(response) => HttpPath {
            url: url.to_string(),
            status: Some(response.status().as_u16()),
            success: response.status().is_success(),
            error: None,
            timing: TimingPath {
                proxy_connect_ms,
                total_ms: start.elapsed().as_secs_f64() * 1000.0,
                ..Default::default()
            },
        },
        Err(err) => HttpPath {
            url: url.to_string(),
            status: None,
            success: false,
            error: Some(err.to_string()),
            timing: TimingPath {
                proxy_connect_ms,
                total_ms: start.elapsed().as_secs_f64() * 1000.0,
                ..Default::default()
            },
        },
    }
}

async fn measure_proxy_connect(proxy: &str, timeout: Duration) -> Option<f64> {
    let endpoint = parse_proxy_endpoint(proxy)?;
    let ips = crate::util::resolve_host_all(&endpoint.host).await;
    if ips.is_empty() {
        return None;
    }

    let start = Instant::now();
    for ip in ips.into_iter().take(8) {
        let addr = SocketAddr::new(ip, endpoint.port);
        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(_)) => return Some(start.elapsed().as_secs_f64() * 1000.0),
            Ok(Err(_)) | Err(_) => continue,
        }
    }
    None
}

struct ProxyEndpoint {
    host: String,
    port: u16,
}

fn parse_proxy_endpoint(proxy: &str) -> Option<ProxyEndpoint> {
    let (scheme, rest) = proxy.split_once("://").unwrap_or(("http", proxy));
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "https" => 443,
        "socks" | "socks4" | "socks4a" | "socks5" | "socks5h" => 1080,
        _ => 80,
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        return None;
    }

    if let Some(host) = authority.strip_prefix('[') {
        let (host, after) = host.split_once(']')?;
        let port = after
            .strip_prefix(':')
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(default_port);
        return Some(ProxyEndpoint {
            host: host.to_string(),
            port,
        });
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().ok()?;
            (host, port)
        }
        None => (authority, default_port),
    };
    if host.is_empty() {
        None
    } else {
        Some(ProxyEndpoint {
            host: host.to_string(),
            port,
        })
    }
}

async fn manual_timing(
    parsed: &ParsedUrl,
    ips: &[IpAddr],
    timeout: Duration,
    dns_ms: f64,
) -> HttpPath {
    let total_start = Instant::now();
    if ips.is_empty() {
        return HttpPath {
            url: parsed.normalized.clone(),
            status: None,
            success: false,
            error: Some("DNS resolve failed".to_string()),
            timing: TimingPath {
                dns_ms: Some(dns_ms),
                total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
                ..Default::default()
            },
        };
    }

    let connect_start = Instant::now();
    let mut tcp_stream = None;
    let mut last_error = None;
    for ip in ips.iter().copied().take(8) {
        let addr = SocketAddr::new(ip, parsed.port);
        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                tcp_stream = Some(stream);
                break;
            }
            Ok(Err(err)) => last_error = Some(format!("TCP: {}", err)),
            Err(_) => last_error = Some("TCP: timeout".to_string()),
        }
    }
    let connect_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
    let Some(tcp_stream) = tcp_stream else {
        return HttpPath {
            url: parsed.normalized.clone(),
            status: None,
            success: false,
            error: last_error.or_else(|| Some("TCP: failed".to_string())),
            timing: TimingPath {
                dns_ms: Some(dns_ms),
                connect_ms: Some(connect_ms),
                total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
                ..Default::default()
            },
        };
    };

    if parsed.is_https {
        timing_https(parsed, timeout, tcp_stream, dns_ms, connect_ms, total_start).await
    } else {
        timing_http(parsed, timeout, tcp_stream, dns_ms, connect_ms, total_start).await
    }
}

async fn timing_https(
    parsed: &ParsedUrl,
    timeout: Duration,
    tcp_stream: tokio::net::TcpStream,
    dns_ms: f64,
    connect_ms: f64,
    total_start: Instant,
) -> HttpPath {
    let tls_start = Instant::now();
    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.iter().cloned().collect(),
    };
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(root_store)
    .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
    let server_name = match rustls::pki_types::ServerName::try_from(parsed.host.clone()) {
        Ok(name) => name,
        Err(err) => {
            return http_error(
                parsed,
                format!("TLS: {}", err),
                dns_ms,
                connect_ms,
                None,
                total_start,
            )
        }
    };
    let tls_stream =
        match tokio::time::timeout(timeout, connector.connect(server_name, tcp_stream)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => {
                return http_error(
                    parsed,
                    format!("TLS: {}", err),
                    dns_ms,
                    connect_ms,
                    Some(tls_start.elapsed().as_secs_f64() * 1000.0),
                    total_start,
                )
            }
            Err(_) => {
                return http_error(
                    parsed,
                    "TLS: timeout".to_string(),
                    dns_ms,
                    connect_ms,
                    Some(tls_start.elapsed().as_secs_f64() * 1000.0),
                    total_start,
                )
            }
        };
    let tls_ms = tls_start.elapsed().as_secs_f64() * 1000.0;
    timing_write_read(
        parsed,
        timeout,
        tls_stream,
        dns_ms,
        connect_ms,
        Some(tls_ms),
        total_start,
    )
    .await
}

async fn timing_http(
    parsed: &ParsedUrl,
    timeout: Duration,
    tcp_stream: tokio::net::TcpStream,
    dns_ms: f64,
    connect_ms: f64,
    total_start: Instant,
) -> HttpPath {
    timing_write_read(
        parsed,
        timeout,
        tcp_stream,
        dns_ms,
        connect_ms,
        None,
        total_start,
    )
    .await
}

async fn timing_write_read<S>(
    parsed: &ParsedUrl,
    timeout: Duration,
    mut stream: S,
    dns_ms: f64,
    connect_ms: f64,
    tls_ms: Option<f64>,
    total_start: Instant,
) -> HttpPath
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: netutils/0.3 path\r\nConnection: close\r\n\r\n",
        parsed.path, parsed.host
    );
    if tokio::time::timeout(timeout, stream.write_all(request.as_bytes()))
        .await
        .map(|r| r.is_err())
        .unwrap_or(true)
    {
        return http_error(
            parsed,
            "HTTP: write failed".to_string(),
            dns_ms,
            connect_ms,
            tls_ms,
            total_start,
        );
    }

    let ttfb_start = Instant::now();
    let mut buf = [0u8; 4096];
    let n = match tokio::time::timeout(timeout, stream.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        Ok(Err(err)) => {
            return http_error(
                parsed,
                format!("HTTP: {}", err),
                dns_ms,
                connect_ms,
                tls_ms,
                total_start,
            )
        }
        Err(_) => {
            return http_error(
                parsed,
                "HTTP: read timeout".to_string(),
                dns_ms,
                connect_ms,
                tls_ms,
                total_start,
            )
        }
    };
    let ttfb_ms = ttfb_start.elapsed().as_secs_f64() * 1000.0;
    let response = String::from_utf8_lossy(&buf[..n]);
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok());

    HttpPath {
        url: parsed.normalized.clone(),
        status,
        success: status.map(|s| (200..400).contains(&s)).unwrap_or(false),
        error: None,
        timing: TimingPath {
            dns_ms: Some(dns_ms),
            connect_ms: Some(connect_ms),
            tls_ms,
            ttfb_ms: Some(ttfb_ms),
            total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
            ..Default::default()
        },
    }
}

fn http_error(
    parsed: &ParsedUrl,
    error: String,
    dns_ms: f64,
    connect_ms: f64,
    tls_ms: Option<f64>,
    total_start: Instant,
) -> HttpPath {
    HttpPath {
        url: parsed.normalized.clone(),
        status: None,
        success: false,
        error: Some(error),
        timing: TimingPath {
            dns_ms: Some(dns_ms),
            connect_ms: Some(connect_ms),
            tls_ms,
            total_ms: total_start.elapsed().as_secs_f64() * 1000.0,
            ..Default::default()
        },
    }
}

fn print_report(report: &PathReport) {
    println!();
    println!("{}", "🧭 HTTP Request Path".bold());
    println!("  URL: {}", report.input);
    println!("  Final URL: {}", report.final_url);

    println!();
    println!("{}", "DNS".bold());
    if report.dns.ips.is_empty() {
        println!("  {} -> <resolve failed>", report.dns.host);
    } else {
        println!("  {} -> {}", report.dns.host, report.dns.ips.join(", "));
    }

    println!();
    println!("{}", "Proxy".bold());
    println!(
        "  {}{}",
        report.proxy.mode,
        report
            .proxy
            .value
            .as_ref()
            .map(|v| format!(" ({})", v))
            .unwrap_or_default()
    );

    println!();
    println!("{}", "Local Egress".bold());
    if let Some(egress) = &report.egress {
        println!("  Interface: {}", egress.interface);
        println!("  IP: {}", egress.ip);
        println!("  Type: {}", egress.iftype);
        println!("  TUN Mode: {}", if egress.tun_mode { "yes" } else { "no" });
    } else {
        println!("  <unknown>");
    }

    if !report.redirects.is_empty() {
        println!();
        println!("{}", "Redirects".bold());
        let rows = report
            .redirects
            .iter()
            .map(|hop| {
                vec![
                    hop.status.to_string(),
                    hop.url.clone(),
                    hop.location.clone().unwrap_or_else(|| "--".to_string()),
                ]
            })
            .collect::<Vec<_>>();
        print_table(&["Status", "URL", "Location"], &rows);
    }

    println!();
    println!("{}", "Network Path".bold());
    if report.network_path.is_empty() {
        println!("  <not available>");
    } else {
        let rows = report
            .network_path
            .iter()
            .map(|hop| {
                vec![
                    hop.ttl.to_string(),
                    hop.ip.clone().unwrap_or_else(|| "*".to_string()),
                    hop.rtt_ms
                        .map(|rtt| format!("{:.2}ms", rtt))
                        .unwrap_or_else(|| "*".to_string()),
                ]
            })
            .collect::<Vec<_>>();
        print_table(&["Hop", "IP", "RTT"], &rows);
    }

    println!();
    println!("{}", "HTTP Timing".bold());
    println!(
        "  Status: {}",
        report
            .http
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "--".to_string())
    );
    if let Some(error) = &report.http.error {
        println!("  Error: {}", error);
    }
    let timing = &report.http.timing;
    let rows = vec![
        vec!["DNS".to_string(), fmt_ms(timing.dns_ms)],
        vec!["Proxy Connect".to_string(), fmt_ms(timing.proxy_connect_ms)],
        vec!["Connect".to_string(), fmt_ms(timing.connect_ms)],
        vec!["TLS".to_string(), fmt_ms(timing.tls_ms)],
        vec!["TTFB".to_string(), fmt_ms(timing.ttfb_ms)],
        vec!["Total".to_string(), format!("{:.2}ms", timing.total_ms)],
    ];
    print_table(&["Phase", "Time"], &rows);

    println!();
    println!("  {}", report.note.dimmed());
}

fn fmt_ms(value: Option<f64>) -> String {
    value
        .map(|v| format!("{:.2}ms", v))
        .unwrap_or_else(|| "--".to_string())
}
