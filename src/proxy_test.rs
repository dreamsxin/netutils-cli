//! Proxy hostname/DNS reachability probe.

use std::collections::BTreeMap;
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
    pub stability: StabilityStats,
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

#[derive(Debug, Clone, Serialize)]
pub struct RequestProbe {
    pub mode: String,
    pub attempted: bool,
    pub ok: bool,
    pub status: Option<u16>,
    pub elapsed_ms: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StabilityStats {
    pub requested: u32,
    pub concurrency: usize,
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub success_rate_pct: f64,
    pub total_elapsed_ms: f64,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub p99_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub assessment: String,
    pub status_counts: BTreeMap<u16, usize>,
    pub error_counts: BTreeMap<String, usize>,
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
    count: u32,
    concurrency: usize,
    mode: OutputMode,
) {
    let count = count.max(1);
    let concurrency = concurrency.max(1).min(count as usize);
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
    let (proxy_request, stability) = match proxy_value.as_deref() {
        Some(proxy_url) => {
            run_proxy_requests(&parsed.normalized, proxy_url, timeout, count, concurrency).await
        }
        None => {
            let probe = RequestProbe {
                mode: "proxy".to_string(),
                attempted: false,
                ok: false,
                status: None,
                elapsed_ms: None,
                error: Some("proxy not configured".to_string()),
            };
            let stats = compute_stability(count, concurrency, 0.0, std::slice::from_ref(&probe));
            (probe, stats)
        }
    };

    let proxy_probe = ProxyProbe {
        configured: proxy_value.is_some(),
        value: proxy_value.as_deref().map(redact_proxy_url),
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
        &stability,
    );
    let report = ProxyTestReport {
        target: target.to_string(),
        url: parsed.normalized,
        host: parsed.host,
        local_resolve,
        proxy: proxy_probe,
        direct_request,
        proxy_request,
        stability,
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
        stability: compute_stability(1, 1, 0.0, &[]),
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
    let client = match build_client(proxy, timeout) {
        Ok(client) => client,
        Err(error) => {
            return RequestProbe {
                mode: mode.to_string(),
                attempted: false,
                ok: false,
                status: None,
                elapsed_ms: None,
                error: Some(error),
            }
        }
    };

    execute_request(&client, url, mode).await
}

fn build_client(proxy: Option<&str>, timeout: Duration) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("netutils proxy-test");
    if let Some(proxy_url) = proxy {
        match reqwest::Proxy::all(proxy_url) {
            Ok(proxy) => builder = builder.proxy(proxy),
            Err(err) => return Err(format!("invalid proxy: {err}")),
        }
    } else {
        builder = builder.no_proxy();
    }

    builder.build().map_err(|err| err.to_string())
}

async fn execute_request(client: &reqwest::Client, url: &str, mode: &str) -> RequestProbe {
    let start = Instant::now();
    match client.get(url).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let ok = response_status_is_reachable(status);
            RequestProbe {
                mode: mode.to_string(),
                attempted: true,
                ok,
                status: Some(status),
                elapsed_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
                error: (!ok).then(|| format!("HTTP {status}")),
            }
        }
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

fn response_status_is_reachable(status: u16) -> bool {
    status != 407 && status < 500
}

async fn run_proxy_requests(
    url: &str,
    proxy: &str,
    timeout: Duration,
    count: u32,
    concurrency: usize,
) -> (RequestProbe, StabilityStats) {
    let concurrency = concurrency.max(1).min(count as usize);
    let client = match build_client(Some(proxy), timeout) {
        Ok(client) => client,
        Err(error) => {
            let probe = RequestProbe {
                mode: "proxy".to_string(),
                attempted: false,
                ok: false,
                status: None,
                elapsed_ms: None,
                error: Some(error),
            };
            let stats = compute_stability(count, concurrency, 0.0, std::slice::from_ref(&probe));
            return (probe, stats);
        }
    };

    let started = Instant::now();
    let mut probes = Vec::with_capacity(count as usize);
    let mut tasks = tokio::task::JoinSet::new();
    let mut scheduled = 0_u32;

    for _ in 0..concurrency {
        spawn_request(&mut tasks, client.clone(), url.to_string());
        scheduled += 1;
    }

    while let Some(result) = tasks.join_next().await {
        probes.push(result.unwrap_or_else(|error| RequestProbe {
            mode: "proxy".to_string(),
            attempted: false,
            ok: false,
            status: None,
            elapsed_ms: None,
            error: Some(format!("request task failed: {error}")),
        }));
        if scheduled < count {
            spawn_request(&mut tasks, client.clone(), url.to_string());
            scheduled += 1;
        }
    }

    let total_elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let stats = compute_stability(count, concurrency, total_elapsed_ms, &probes);
    let representative = probes
        .iter()
        .find(|probe| !probe.ok)
        .or_else(|| probes.first())
        .cloned()
        .unwrap_or_else(|| RequestProbe {
            mode: "proxy".to_string(),
            attempted: false,
            ok: false,
            status: None,
            elapsed_ms: None,
            error: Some("no request result".to_string()),
        });
    (representative, stats)
}

fn spawn_request(
    tasks: &mut tokio::task::JoinSet<RequestProbe>,
    client: reqwest::Client,
    url: String,
) {
    tasks.spawn(async move { execute_request(&client, &url, "proxy").await });
}

fn compute_stability(
    requested: u32,
    concurrency: usize,
    total_elapsed_ms: f64,
    probes: &[RequestProbe],
) -> StabilityStats {
    let attempted = probes.iter().filter(|probe| probe.attempted).count();
    let succeeded = probes.iter().filter(|probe| probe.ok).count();
    let failed = (requested as usize).saturating_sub(succeeded);
    let success_rate_pct = if requested == 0 {
        0.0
    } else {
        succeeded as f64 / requested as f64 * 100.0
    };
    let mut elapsed: Vec<f64> = probes
        .iter()
        .filter(|probe| probe.ok)
        .filter_map(|probe| probe.elapsed_ms)
        .collect();
    elapsed.sort_by(f64::total_cmp);
    let avg_ms = (!elapsed.is_empty()).then(|| elapsed.iter().sum::<f64>() / elapsed.len() as f64);
    let min_ms = elapsed.first().copied();
    let max_ms = elapsed.last().copied();
    let p50_ms = percentile(&elapsed, 0.50);
    let p95_ms = percentile(&elapsed, 0.95);
    let p99_ms = percentile(&elapsed, 0.99);

    let assessment = assess_stability(requested, attempted, success_rate_pct, p50_ms, p95_ms);
    let mut status_counts = BTreeMap::new();
    let mut error_counts = BTreeMap::new();
    for probe in probes {
        if let Some(status) = probe.status {
            *status_counts.entry(status).or_insert(0) += 1;
        }
        if let Some(error) = &probe.error {
            *error_counts.entry(error.clone()).or_insert(0) += 1;
        }
    }

    StabilityStats {
        requested,
        concurrency,
        attempted,
        succeeded,
        failed,
        success_rate_pct,
        total_elapsed_ms,
        min_ms,
        avg_ms,
        p50_ms,
        p95_ms,
        p99_ms,
        max_ms,
        assessment,
        status_counts,
        error_counts,
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (quantile * sorted.len() as f64).ceil() as usize;
    sorted.get(rank.saturating_sub(1)).copied()
}

fn assess_stability(
    requested: u32,
    attempted: usize,
    success_rate_pct: f64,
    p50_ms: Option<f64>,
    p95_ms: Option<f64>,
) -> String {
    if attempted == 0 {
        return "not-run".to_string();
    }
    if requested < 5 {
        return "insufficient-samples".to_string();
    }
    if success_rate_pct < 99.0 {
        return "unstable".to_string();
    }
    if let (Some(p50), Some(p95)) = (p50_ms, p95_ms) {
        if p95 > p50 * 2.0 + 250.0 {
            return "variable-latency".to_string();
        }
    }
    "stable".to_string()
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

fn redact_proxy_url(proxy: &str) -> String {
    let Some((scheme, rest)) = proxy.split_once("://") else {
        return proxy.to_string();
    };
    let Some((_, endpoint)) = rest.rsplit_once('@') else {
        return proxy.to_string();
    };
    format!("{scheme}://***@{endpoint}")
}

fn build_conclusions(
    local: &LocalResolve,
    proxy: &ProxyProbe,
    direct: &RequestProbe,
    proxied: &RequestProbe,
    stability: &StabilityStats,
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
    let proxied_ok = stability.succeeded > 0 || proxied.ok;
    if proxied_ok && !local.ok {
        conclusions.push(
            "Proxy hostname request succeeded while local DNS failed; proxy-side DNS is working for this target."
                .to_string(),
        );
    } else if proxied_ok {
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
    match stability.assessment.as_str() {
        "stable" => conclusions.push(format!(
            "Stability sample is stable: {}/{} succeeded ({:.2}%), P95 {:.0}ms.",
            stability.succeeded,
            stability.requested,
            stability.success_rate_pct,
            stability.p95_ms.unwrap_or_default()
        )),
        "unstable" => conclusions.push(format!(
            "Stability sample is unstable: {}/{} succeeded ({:.2}%).",
            stability.succeeded, stability.requested, stability.success_rate_pct
        )),
        "variable-latency" => conclusions.push(format!(
            "Request success is high, but latency varies significantly: P50 {:.0}ms, P95 {:.0}ms.",
            stability.p50_ms.unwrap_or_default(),
            stability.p95_ms.unwrap_or_default()
        )),
        "insufficient-samples" => conclusions.push(
            "Fewer than 5 proxy requests were sampled; use --count 20 or more for a stability assessment."
                .to_string(),
        ),
        _ => {}
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
    println!("{}", "Stability".bold());
    print_stability(&report.stability);

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

fn print_stability(stats: &StabilityStats) {
    let rows = vec![
        vec!["Assessment".to_string(), stats.assessment.clone()],
        vec!["Requests".to_string(), stats.requested.to_string()],
        vec!["Concurrency".to_string(), stats.concurrency.to_string()],
        vec!["Succeeded".to_string(), stats.succeeded.to_string()],
        vec!["Failed".to_string(), stats.failed.to_string()],
        vec![
            "Success Rate".to_string(),
            format!("{:.2}%", stats.success_rate_pct),
        ],
        vec!["Min".to_string(), format_ms(stats.min_ms)],
        vec!["Average".to_string(), format_ms(stats.avg_ms)],
        vec!["P50".to_string(), format_ms(stats.p50_ms)],
        vec!["P95".to_string(), format_ms(stats.p95_ms)],
        vec!["P99".to_string(), format_ms(stats.p99_ms)],
        vec!["Max".to_string(), format_ms(stats.max_ms)],
        vec![
            "Total Time".to_string(),
            format!("{:.0}ms", stats.total_elapsed_ms),
        ],
        vec![
            "HTTP Statuses".to_string(),
            format_counts(&stats.status_counts),
        ],
        vec!["Errors".to_string(), format_counts(&stats.error_counts)],
    ];
    print_table(&["Metric", "Value"], &rows);
}

fn format_ms(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.0}ms"))
        .unwrap_or_else(|| "--".to_string())
}

fn format_counts<K: std::fmt::Display>(counts: &BTreeMap<K, usize>) -> String {
    if counts.is_empty() {
        return "--".to_string();
    }
    counts
        .iter()
        .map(|(value, count)| format!("{value}: {count}"))
        .collect::<Vec<_>>()
        .join(", ")
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
        let probe = request(true);
        let stability = compute_stability(1, 1, 1.0, std::slice::from_ref(&probe));
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
            &probe,
            &stability,
        );
        assert!(conclusions[0].contains("proxy-side DNS is working"));
    }

    #[test]
    fn hint_warns_about_plain_socks5() {
        assert!(proxy_dns_mode_hint("socks5://127.0.0.1:1080").contains("socks5h"));
    }

    #[test]
    fn redacts_proxy_credentials() {
        assert_eq!(
            redact_proxy_url("socks5://user:secret@example.com:1080"),
            "socks5://***@example.com:1080"
        );
        assert_eq!(
            redact_proxy_url("http://example.com:8080"),
            "http://example.com:8080"
        );
    }

    #[test]
    fn computes_nearest_rank_percentiles() {
        let values = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0];
        assert_eq!(percentile(&values, 0.50), Some(50.0));
        assert_eq!(percentile(&values, 0.95), Some(100.0));
        assert_eq!(percentile(&[], 0.95), None);
    }

    #[test]
    fn stability_requires_enough_samples_and_consistent_latency() {
        assert_eq!(
            assess_stability(1, 1, 100.0, Some(100.0), Some(100.0)),
            "insufficient-samples"
        );
        assert_eq!(
            assess_stability(100, 100, 98.0, Some(100.0), Some(100.0)),
            "unstable"
        );
        assert_eq!(
            assess_stability(100, 100, 100.0, Some(100.0), Some(500.1)),
            "variable-latency"
        );
        assert_eq!(
            assess_stability(100, 100, 99.0, Some(100.0), Some(300.0)),
            "stable"
        );
    }

    #[test]
    fn treats_proxy_auth_and_server_errors_as_failures() {
        assert!(response_status_is_reachable(200));
        assert!(response_status_is_reachable(403));
        assert!(!response_status_is_reachable(407));
        assert!(!response_status_is_reachable(502));
    }
}
