//! Proxy hostname/DNS reachability probe.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
use trust_dns_resolver::TokioAsyncResolver;

use crate::output::{print_json, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
pub struct ProxyTestReport {
    pub target: String,
    pub url: String,
    pub host: String,
    pub local_resolve: LocalResolve,
    pub proxy: ProxyProbe,
    pub direct_request: RequestProbe,
    pub proxy_request: RequestProbe,
    pub conclusions: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LocalResolve {
    pub ok: bool,
    pub ips: Vec<String>,
    pub elapsed_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct ProxyProbe {
    pub configured: bool,
    pub value: Option<String>,
    pub dns_mode_hint: String,
    pub endpoint_host: Option<String>,
    pub endpoint_port: Option<u16>,
    pub route_interface: Option<String>,
    pub gateway: Option<String>,
    pub tcp_connect: TcpProbe,
}

#[derive(Debug, Serialize)]
pub struct TcpProbe {
    pub ok: bool,
    pub elapsed_ms: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequestProbe {
    pub mode: String,
    pub attempted: bool,
    pub ok: bool,
    pub status: Option<u16>,
    pub elapsed_ms: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct ParsedUrl {
    host: String,
    normalized: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyEndpoint {
    scheme: String,
    host: String,
    port: u16,
}

pub async fn run(
    target: &str,
    proxy: Option<String>,
    no_system_proxy: bool,
    timeout: Duration,
    mode: OutputMode,
) {
    let url = normalize_url(target);
    let parsed = match parse_url(&url) {
        Some(parsed) => parsed,
        None => {
            let report = error_report(target, &url, "invalid URL");
            output(report, mode);
            return;
        }
    };

    let local_resolve = resolve_local(&parsed.host, timeout).await;
    let proxy_value = if no_system_proxy {
        proxy
    } else {
        proxy.or_else(crate::util::get_system_proxy_addr)
    };

    let endpoint = proxy_value.as_deref().and_then(parse_proxy_endpoint);
    let route_target = match endpoint.as_ref() {
        Some(endpoint) if endpoint.host.parse::<IpAddr>().is_ok() => Some(endpoint.host.clone()),
        Some(endpoint) => resolve_fast(&endpoint.host, timeout)
            .await
            .into_iter()
            .next()
            .map(|ip| ip.to_string()),
        None => None,
    };
    let route = route_target
        .as_deref()
        .and_then(crate::route_probe::route_to_target);
    let tcp_connect = match (proxy_value.as_deref(), endpoint.as_ref()) {
        (Some(_), Some(endpoint)) => measure_tcp_connect(endpoint, timeout).await,
        (Some(_), None) => TcpProbe {
            ok: false,
            elapsed_ms: None,
            error: Some("invalid proxy URL".to_string()),
        },
        (None, _) => TcpProbe {
            ok: false,
            elapsed_ms: None,
            error: Some("proxy not configured".to_string()),
        },
    };

    let direct_request = if proxy_value.is_some() {
        run_request(&parsed.normalized, None, timeout, "direct").await
    } else {
        RequestProbe {
            mode: "direct".to_string(),
            attempted: false,
            ok: false,
            status: None,
            elapsed_ms: None,
            error: Some("proxy not configured".to_string()),
        }
    };
    let proxy_request = match proxy_value.as_deref() {
        Some(proxy_url) => run_request(&parsed.normalized, Some(proxy_url), timeout, "proxy").await,
        None => RequestProbe {
            mode: "proxy".to_string(),
            attempted: false,
            ok: false,
            status: None,
            elapsed_ms: None,
            error: Some("proxy not configured".to_string()),
        },
    };

    let proxy_probe = ProxyProbe {
        configured: proxy_value.is_some(),
        value: proxy_value.clone(),
        dns_mode_hint: proxy_value
            .as_deref()
            .map(proxy_dns_mode_hint)
            .unwrap_or_else(|| "no proxy configured".to_string()),
        endpoint_host: endpoint.as_ref().map(|endpoint| endpoint.host.clone()),
        endpoint_port: endpoint.as_ref().map(|endpoint| endpoint.port),
        route_interface: route.as_ref().and_then(|route| route.interface.clone()),
        gateway: route.as_ref().and_then(|route| route.gateway.clone()),
        tcp_connect,
    };

    let conclusions = build_conclusions(
        &local_resolve,
        &proxy_probe,
        &direct_request,
        &proxy_request,
    );
    let report = ProxyTestReport {
        target: target.to_string(),
        url: parsed.normalized,
        host: parsed.host,
        local_resolve,
        proxy: proxy_probe,
        direct_request,
        proxy_request,
        conclusions,
        notes: vec![
            "This proves whether a hostname request through the proxy works; most proxy protocols do not expose the exact remote DNS answer.".to_string(),
            "For SOCKS, use socks5h:// or socks4a:// when you need DNS resolution to happen on the proxy side.".to_string(),
        ],
    };

    output(report, mode);
}

fn output(report: ProxyTestReport, mode: OutputMode) {
    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn error_report(target: &str, url: &str, error: &str) -> ProxyTestReport {
    ProxyTestReport {
        target: target.to_string(),
        url: url.to_string(),
        host: String::new(),
        local_resolve: LocalResolve {
            ok: false,
            ips: Vec::new(),
            elapsed_ms: 0.0,
        },
        proxy: ProxyProbe {
            configured: false,
            value: None,
            dns_mode_hint: "not checked".to_string(),
            endpoint_host: None,
            endpoint_port: None,
            route_interface: None,
            gateway: None,
            tcp_connect: TcpProbe {
                ok: false,
                elapsed_ms: None,
                error: Some(error.to_string()),
            },
        },
        direct_request: RequestProbe {
            mode: "direct".to_string(),
            attempted: false,
            ok: false,
            status: None,
            elapsed_ms: None,
            error: Some(error.to_string()),
        },
        proxy_request: RequestProbe {
            mode: "proxy".to_string(),
            attempted: false,
            ok: false,
            status: None,
            elapsed_ms: None,
            error: Some(error.to_string()),
        },
        conclusions: vec![error.to_string()],
        notes: Vec::new(),
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
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split('/').next().unwrap_or(rest);
    if authority.is_empty() {
        return None;
    }
    let host = parse_authority_host(authority)?;
    Some(ParsedUrl {
        host,
        normalized: url.to_string(),
    })
}

fn parse_authority_host(authority: &str) -> Option<String> {
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(host) = authority.strip_prefix('[') {
        return Some(host.split_once(']')?.0.to_string());
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => {
            (!host.is_empty()).then(|| host.to_string())
        }
        _ => Some(authority.to_string()).filter(|host| !host.is_empty()),
    }
}

async fn resolve_local(host: &str, timeout: Duration) -> LocalResolve {
    let start = Instant::now();
    let ips = resolve_fast(host, timeout).await;
    LocalResolve {
        ok: !ips.is_empty(),
        ips: ips.iter().map(IpAddr::to_string).collect(),
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    }
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

async fn run_request(
    url: &str,
    proxy: Option<&str>,
    timeout: Duration,
    mode: &str,
) -> RequestProbe {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("netutils proxy-test");
    if let Some(proxy_url) = proxy {
        match reqwest::Proxy::all(proxy_url) {
            Ok(proxy) => builder = builder.proxy(proxy),
            Err(err) => {
                return RequestProbe {
                    mode: mode.to_string(),
                    attempted: false,
                    ok: false,
                    status: None,
                    elapsed_ms: None,
                    error: Some(format!("invalid proxy: {err}")),
                }
            }
        }
    } else {
        builder = builder.no_proxy();
    }

    let client = match builder.build() {
        Ok(client) => client,
        Err(err) => {
            return RequestProbe {
                mode: mode.to_string(),
                attempted: false,
                ok: false,
                status: None,
                elapsed_ms: None,
                error: Some(err.to_string()),
            }
        }
    };

    let start = Instant::now();
    match client.get(url).send().await {
        Ok(response) => RequestProbe {
            mode: mode.to_string(),
            attempted: true,
            ok: true,
            status: Some(response.status().as_u16()),
            elapsed_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
            error: None,
        },
        Err(err) => RequestProbe {
            mode: mode.to_string(),
            attempted: true,
            ok: false,
            status: None,
            elapsed_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
            error: Some(err.to_string()),
        },
    }
}

async fn measure_tcp_connect(endpoint: &ProxyEndpoint, timeout: Duration) -> TcpProbe {
    let ips = resolve_fast(&endpoint.host, timeout).await;
    if ips.is_empty() {
        return TcpProbe {
            ok: false,
            elapsed_ms: None,
            error: Some("proxy host resolve failed".to_string()),
        };
    }

    let start = Instant::now();
    let mut last_error = None;
    for ip in ips.into_iter().take(8) {
        let addr = SocketAddr::new(ip, endpoint.port);
        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(_)) => {
                return TcpProbe {
                    ok: true,
                    elapsed_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
                    error: None,
                }
            }
            Ok(Err(err)) => last_error = Some(err.to_string()),
            Err(_) => last_error = Some("timeout".to_string()),
        }
    }

    TcpProbe {
        ok: false,
        elapsed_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
        error: last_error.or_else(|| Some("tcp connect failed".to_string())),
    }
}

fn parse_proxy_endpoint(proxy: &str) -> Option<ProxyEndpoint> {
    let (scheme, rest) = proxy.split_once("://").unwrap_or(("http", proxy));
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
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
            scheme,
            host: host.to_string(),
            port,
        });
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (authority, default_port),
    };
    if host.is_empty() {
        None
    } else {
        Some(ProxyEndpoint {
            scheme,
            host: host.to_string(),
            port,
        })
    }
}

fn proxy_dns_mode_hint(proxy: &str) -> String {
    let scheme = proxy
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .unwrap_or_else(|| "http".to_string());
    match scheme.as_str() {
        "socks5h" | "socks4a" => "remote DNS requested by proxy URL scheme".to_string(),
        "socks" | "socks5" | "socks4" => {
            "SOCKS URL may resolve locally; use socks5h:// or socks4a:// for remote DNS".to_string()
        }
        "http" | "https" => {
            "HTTP proxy receives the hostname for CONNECT/absolute-form requests".to_string()
        }
        _ => "unknown proxy scheme; DNS behavior depends on the client/proxy".to_string(),
    }
}

fn build_conclusions(
    local: &LocalResolve,
    proxy: &ProxyProbe,
    direct: &RequestProbe,
    proxied: &RequestProbe,
) -> Vec<String> {
    let mut conclusions = Vec::new();
    if !proxy.configured {
        conclusions.push("No proxy configured; pass --proxy or enable a system proxy.".to_string());
        return conclusions;
    }
    if !proxy.tcp_connect.ok {
        conclusions.push(format!(
            "Proxy entry is not reachable: {}.",
            proxy.tcp_connect.error.as_deref().unwrap_or("unknown")
        ));
    }
    if proxied.ok && !local.ok {
        conclusions.push(
            "Proxy hostname request succeeded while local DNS failed; proxy-side DNS is working for this target."
                .to_string(),
        );
    } else if proxied.ok {
        conclusions.push(
            "Proxy hostname request succeeded; proxy can access this domain, but the exact remote DNS IP is not exposed."
                .to_string(),
        );
    } else if local.ok && direct.ok {
        conclusions.push(
            "Local direct access works but proxied access failed; check proxy DNS, proxy rules, or remote egress."
                .to_string(),
        );
    } else if local.ok {
        conclusions.push(
            "Local DNS resolved the domain, but proxied access failed; proxy DNS/rules/egress may be the fault point."
                .to_string(),
        );
    } else {
        conclusions.push(
            "Both local DNS and proxied hostname access failed; check the domain, resolver, and proxy availability."
                .to_string(),
        );
    }
    conclusions
}

fn print_report(report: &ProxyTestReport) {
    println!();
    println!("{}", "🧪 Proxy DNS Probe".bold());
    println!("  Target: {}", report.target);
    println!("  URL: {}", report.url);

    println!();
    println!("{}", "Local DNS".bold());
    if report.local_resolve.ok {
        println!(
            "  {} -> {} ({:.0}ms)",
            report.host,
            report.local_resolve.ips.join(", "),
            report.local_resolve.elapsed_ms
        );
    } else {
        println!(
            "  {} -> failed ({:.0}ms)",
            report.host, report.local_resolve.elapsed_ms
        );
    }

    println!();
    println!("{}", "Proxy".bold());
    println!(
        "  Value: {}",
        report.proxy.value.as_deref().unwrap_or("not configured")
    );
    println!("  DNS Mode: {}", report.proxy.dns_mode_hint);
    if let Some(host) = &report.proxy.endpoint_host {
        println!(
            "  Entry: {}:{}",
            host,
            report.proxy.endpoint_port.unwrap_or_default()
        );
    }
    println!(
        "  Route: iface {}, gateway {}",
        report.proxy.route_interface.as_deref().unwrap_or("--"),
        report.proxy.gateway.as_deref().unwrap_or("--")
    );
    println!(
        "  TCP Connect: {}{}",
        if report.proxy.tcp_connect.ok {
            "ok".green().to_string()
        } else {
            "failed".red().to_string()
        },
        report
            .proxy
            .tcp_connect
            .elapsed_ms
            .map(|ms| format!(" ({ms:.0}ms)"))
            .unwrap_or_default()
    );
    if let Some(error) = &report.proxy.tcp_connect.error {
        println!("  TCP Error: {}", error);
    }

    println!();
    println!("{}", "Requests".bold());
    let rows = vec![
        request_row(&report.direct_request),
        request_row(&report.proxy_request),
    ];
    print_table(&["Mode", "Result", "Status", "Time", "Error"], &rows);

    println!();
    println!("{}", "Conclusion".bold());
    for conclusion in &report.conclusions {
        println!("  {}", conclusion);
    }

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn request_row(probe: &RequestProbe) -> Vec<String> {
    vec![
        probe.mode.clone(),
        if !probe.attempted {
            "skipped".to_string()
        } else if probe.ok {
            "ok".green().to_string()
        } else {
            "failed".red().to_string()
        },
        probe
            .status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "--".to_string()),
        probe
            .elapsed_ms
            .map(|ms| format!("{ms:.0}ms"))
            .unwrap_or_else(|| "--".to_string()),
        probe.error.clone().unwrap_or_else(|| "--".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(ok: bool) -> RequestProbe {
        RequestProbe {
            mode: "test".to_string(),
            attempted: true,
            ok,
            status: ok.then_some(200),
            elapsed_ms: Some(1.0),
            error: None,
        }
    }

    #[test]
    fn parses_proxy_endpoint_with_auth_and_default_port() {
        let endpoint = parse_proxy_endpoint("socks5h://user:pass@[::1]").unwrap();
        assert_eq!(endpoint.scheme, "socks5h");
        assert_eq!(endpoint.host, "::1");
        assert_eq!(endpoint.port, 1080);
    }

    #[test]
    fn parses_url_host_without_port() {
        let parsed = parse_url("https://example.com/path").unwrap();
        assert_eq!(parsed.host, "example.com");
    }

    #[test]
    fn conclusion_detects_proxy_dns_when_local_dns_fails() {
        let conclusions = build_conclusions(
            &LocalResolve {
                ok: false,
                ips: Vec::new(),
                elapsed_ms: 1.0,
            },
            &ProxyProbe {
                configured: true,
                value: Some("socks5h://127.0.0.1:1080".to_string()),
                dns_mode_hint: String::new(),
                endpoint_host: None,
                endpoint_port: None,
                route_interface: None,
                gateway: None,
                tcp_connect: TcpProbe {
                    ok: true,
                    elapsed_ms: Some(1.0),
                    error: None,
                },
            },
            &request(false),
            &request(true),
        );
        assert!(conclusions[0].contains("proxy-side DNS is working"));
    }

    #[test]
    fn hint_warns_about_plain_socks5() {
        assert!(proxy_dns_mode_hint("socks5://127.0.0.1:1080").contains("socks5h"));
    }
}
