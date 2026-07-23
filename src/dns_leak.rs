//! DNS leak detection: compare local DNS routing with externally observed resolvers.

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use colored::*;
use serde::{Deserialize, Serialize};
use trust_dns_resolver::TokioAsyncResolver;

use crate::dns_path::{get_dns_servers, DnsServer};
use crate::output::{print_json, OutputMode};
use crate::route_probe::RouteProbe;
use crate::table::print_table;

const MAX_DNS_SERVERS: usize = 16;
const WHOAMI_DOMAIN: &str = "whoami.akamai.net";
const SURFSHARK_DOMAIN_SUFFIX: &str = "ipv4.surfsharkdns.com";
const IP_API_EDNS_URL: &str = "https://edns.ip-api.com/json";
const CLOUDFLARE_TRACE_URL: &str = "https://1.1.1.1/cdn-cgi/trace";
const SURFSHARK_RETRY_DELAY: Duration = Duration::from_millis(150);

static PROBE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize)]
pub struct DnsLeakReport {
    pub tun_mode: bool,
    pub egress_interface: Option<String>,
    pub egress_ip: Option<String>,
    pub proxy: Option<String>,
    pub proxy_dns_mode: Option<String>,
    pub surfshark_probe_count: usize,
    pub ip_api_edns_probe_count: usize,
    pub system_dns_servers: Vec<DnsLeakServer>,
    pub resolver_path: Vec<ResolverPathEntry>,
    pub external_probes: Vec<ExternalProbe>,
    pub resolver_public_ips: Vec<String>,
    pub egress_public_ip: Option<String>,
    pub assessment: String,
    pub risk_level: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DnsLeakServer {
    pub server: String,
    pub configured_interface: Option<String>,
    pub source: String,
    pub route_interface: Option<String>,
    pub gateway: Option<String>,
    pub matches_egress: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ResolverPathEntry {
    pub server: String,
    pub route_interface: Option<String>,
    pub egress_interface: Option<String>,
    pub diverted: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExternalProbe {
    pub service: String,
    pub kind: String,
    pub query_name: Option<String>,
    pub observed_ips: Vec<String>,
    pub resolvers: Vec<ResolverObservation>,
    pub ok: bool,
    pub elapsed_ms: f64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolverObservation {
    pub source: String,
    pub ip: String,
    pub isp: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub country_code: Option<String>,
    pub provider_leak: Option<bool>,
}

#[derive(Debug)]
struct LocalDnsLeakAnalysis {
    tun_mode: bool,
    egress_interface: Option<String>,
    egress_ip: Option<String>,
    system_dns_servers: Vec<DnsLeakServer>,
    resolver_path: Vec<ResolverPathEntry>,
}

#[derive(Debug)]
pub struct DnsLeakQuickCheck {
    pub tun_mode: bool,
    pub total_servers: usize,
    pub known_routes: usize,
    pub diverted_routes: usize,
}

#[derive(Debug, Deserialize)]
struct SurfsharkResolverPayload {
    #[serde(rename = "ISP")]
    isp: Option<String>,
    #[serde(rename = "Country")]
    country: Option<String>,
    #[serde(rename = "City")]
    city: Option<String>,
    #[serde(rename = "IP")]
    ip: Option<String>,
    #[serde(rename = "Leak")]
    leak: Option<bool>,
    #[serde(rename = "CountryCode")]
    country_code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IpApiEdnsResponse {
    dns: IpApiEdnsResolver,
}

#[derive(Debug, Deserialize)]
struct IpApiEdnsResolver {
    geo: String,
    ip: String,
}

pub async fn run(
    proxy: Option<String>,
    no_proxy: bool,
    no_external: bool,
    timeout: u64,
    count: usize,
    mode: OutputMode,
) {
    let timeout = Duration::from_secs(timeout);
    let LocalDnsLeakAnalysis {
        tun_mode,
        egress_interface,
        egress_ip,
        system_dns_servers,
        resolver_path,
    } = collect_local_analysis().await;

    let surfshark_queries = (0..count)
        .map(|_| surfshark_query_name())
        .collect::<Vec<_>>();
    let surfshark_url = format!("https://{}/", surfshark_queries[0]);
    let surfshark_proxy = select_proxy(proxy.as_deref(), no_proxy, &surfshark_url);
    let ip_api_proxy = select_proxy(proxy.as_deref(), no_proxy, IP_API_EDNS_URL);
    let cloudflare_proxy = select_proxy(proxy.as_deref(), no_proxy, CLOUDFLARE_TRACE_URL);
    let proxy_dns_mode = surfshark_proxy.as_deref().map(classify_proxy_dns_mode);
    let proxy_remote_dns = proxy_dns_mode.as_deref() == Some("remote");
    let proxy_url = surfshark_proxy
        .as_deref()
        .map(crate::util::redact_url_credentials);

    let mut external_probes = Vec::new();
    let mut egress_public_ip = None;
    if !no_external {
        let (surfshark, ip_api_edns, akamai, cloudflare) = tokio::join!(
            probe_surfshark_many(surfshark_queries, surfshark_proxy.clone(), timeout),
            probe_ip_api_edns_many(count, ip_api_proxy, timeout),
            probe_akamai_whoami(timeout),
            probe_cloudflare_trace(cloudflare_proxy.as_deref(), timeout),
        );
        egress_public_ip = cloudflare.observed_ips.first().cloned();
        external_probes.extend(surfshark);
        external_probes.extend(ip_api_edns);
        external_probes.push(akamai);
        external_probes.push(cloudflare);
    }

    let resolver_public_ips = collect_resolver_ips(&external_probes);
    let proxy_active = surfshark_proxy.is_some();
    let risk_level = assess(
        tun_mode,
        proxy_active,
        proxy_remote_dns,
        &resolver_path,
        &external_probes,
        no_external,
    );
    let assessment = assessment_message(
        &risk_level,
        tun_mode,
        proxy_active,
        proxy_remote_dns,
        &resolver_path,
        &external_probes,
        no_external,
    );

    let report = DnsLeakReport {
        tun_mode,
        egress_interface,
        egress_ip,
        proxy: proxy_url,
        proxy_dns_mode,
        surfshark_probe_count: count,
        ip_api_edns_probe_count: count,
        system_dns_servers,
        resolver_path,
        external_probes,
        resolver_public_ips,
        egress_public_ip,
        assessment,
        risk_level: risk_level.clone(),
        notes: build_notes(no_external, tun_mode, proxy_active, proxy_remote_dns),
    };

    if risk_level == "high" {
        crate::output::mark_failure();
    }

    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

pub async fn quick_check() -> DnsLeakQuickCheck {
    let analysis = collect_local_analysis().await;
    DnsLeakQuickCheck {
        tun_mode: analysis.tun_mode,
        total_servers: analysis.resolver_path.len(),
        known_routes: analysis
            .resolver_path
            .iter()
            .filter(|entry| entry.diverted.is_some())
            .count(),
        diverted_routes: analysis
            .resolver_path
            .iter()
            .filter(|entry| entry.diverted == Some(true))
            .count(),
    }
}

async fn collect_local_analysis() -> LocalDnsLeakAnalysis {
    let (egress, dns_servers) = collect_local_environment().await;
    let egress_interface = egress.as_ref().map(|egress| egress.interface.clone());
    let egress_ip = egress.as_ref().map(|egress| egress.ip.clone());
    let tun_mode = egress.as_ref().is_some_and(|egress| egress.tun_mode);

    let selected_dns = select_dns_servers(dns_servers);
    let route_results = collect_dns_routes(selected_dns).await;
    let mut system_dns_servers = Vec::new();
    let mut resolver_path = Vec::new();
    for (dns_server, route) in route_results {
        let (route_interface, gateway) = match route {
            Some(route) => (route.interface, route.gateway),
            None => (None, None),
        };
        let matches_egress = if is_local_resolver_hop(&dns_server.server, &route_interface) {
            None
        } else {
            compare_interfaces(&route_interface, &egress_interface)
        };
        system_dns_servers.push(DnsLeakServer {
            server: dns_server.server.clone(),
            configured_interface: dns_server.interface,
            source: dns_server.source,
            route_interface: route_interface.clone(),
            gateway,
            matches_egress,
        });
        resolver_path.push(ResolverPathEntry {
            server: dns_server.server,
            route_interface,
            egress_interface: egress_interface.clone(),
            diverted: matches_egress.map(|matches| !matches),
        });
    }

    LocalDnsLeakAnalysis {
        tun_mode,
        egress_interface,
        egress_ip,
        system_dns_servers,
        resolver_path,
    }
}

async fn collect_local_environment() -> (Option<crate::info::EgressInfo>, Vec<DnsServer>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let interfaces = crate::info::get_all_interfaces();
        let egress = crate::info::collect_egress(&interfaces);
        let dns_servers = get_dns_servers();
        let _ = tx.send((egress, dns_servers));
    });
    rx.await.unwrap_or((None, Vec::new()))
}

fn select_dns_servers(dns_servers: Vec<DnsServer>) -> Vec<DnsServer> {
    let mut seen = HashSet::new();
    dns_servers
        .into_iter()
        .filter(|server| seen.insert(server.server.clone()))
        .take(MAX_DNS_SERVERS)
        .collect()
}

async fn collect_dns_routes(dns_servers: Vec<DnsServer>) -> Vec<(DnsServer, Option<RouteProbe>)> {
    let mut tasks = tokio::task::JoinSet::new();
    for (index, dns_server) in dns_servers.into_iter().enumerate() {
        tasks.spawn(async move {
            let target = dns_server.server.clone();
            let route = route_to_target_async(target).await;
            (index, dns_server, route)
        });
    }

    let mut results = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(result) = result {
            results.push(result);
        }
    }
    results.sort_by_key(|(index, _, _)| *index);
    results
        .into_iter()
        .map(|(_, dns_server, route)| (dns_server, route))
        .collect()
}

async fn route_to_target_async(target: String) -> Option<RouteProbe> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let route = crate::route_probe::route_to_target(&target);
        let _ = tx.send(route);
    });
    rx.await.ok().flatten()
}

fn compare_interfaces(route: &Option<String>, egress: &Option<String>) -> Option<bool> {
    match (route, egress) {
        (Some(route), Some(egress)) => Some(route == egress),
        _ => None,
    }
}

fn is_local_resolver_hop(server: &str, route_interface: &Option<String>) -> bool {
    if server
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
    {
        return true;
    }
    route_interface.as_deref().is_some_and(|interface| {
        let interface = interface.to_ascii_lowercase();
        interface == "lo" || interface == "lo0" || interface.contains("loopback")
    })
}

fn select_proxy(explicit: Option<&str>, no_proxy: bool, target: &str) -> Option<String> {
    if no_proxy {
        None
    } else {
        explicit
            .map(str::to_string)
            .or_else(|| crate::util::get_system_proxy_for_url(target))
    }
}

fn classify_proxy_dns_mode(proxy: &str) -> String {
    let scheme = reqwest::Url::parse(proxy)
        .ok()
        .map(|url| url.scheme().to_ascii_lowercase());
    match scheme.as_deref() {
        Some("http" | "https" | "socks4a" | "socks5h") => "remote".to_string(),
        Some("socks4" | "socks5") => "local".to_string(),
        _ => "unknown".to_string(),
    }
}

async fn probe_surfshark_many(
    query_names: Vec<String>,
    proxy: Option<String>,
    timeout: Duration,
) -> Vec<ExternalProbe> {
    let mut tasks = tokio::task::JoinSet::new();
    for (index, query_name) in query_names.into_iter().enumerate() {
        let proxy = proxy.clone();
        tasks.spawn(async move {
            let url = format!("https://{query_name}/");
            let probe = probe_surfshark(&query_name, &url, proxy.as_deref(), timeout).await;
            (index, probe)
        });
    }

    let mut probes = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(result) = result {
            probes.push(result);
        }
    }
    probes.sort_by_key(|(index, _)| *index);
    probes.into_iter().map(|(_, probe)| probe).collect()
}

async fn probe_surfshark(
    query_name: &str,
    url: &str,
    proxy: Option<&str>,
    timeout: Duration,
) -> ExternalProbe {
    let service = "surfshark-dns";
    let start = Instant::now();
    let client = match build_http_client(proxy, timeout) {
        Ok(client) => client,
        Err(err) => {
            return failed_probe(
                service,
                "dns-resolver",
                Some(query_name),
                start.elapsed(),
                err.to_string(),
            );
        }
    };

    loop {
        let Some(remaining) = timeout.checked_sub(start.elapsed()) else {
            return failed_probe(
                service,
                "dns-resolver",
                Some(query_name),
                start.elapsed(),
                "timeout".to_string(),
            );
        };
        let request = async {
            let response = client.get(url).send().await?;
            let status = response.status();
            let body = response.text().await?;
            Ok::<_, reqwest::Error>((status, body))
        };
        match tokio::time::timeout(remaining, request).await {
            Ok(Ok((status, body))) if status.is_success() => {
                match parse_surfshark_response(&body) {
                    Ok(resolvers) if !resolvers.is_empty() => {
                        let observed_ips = resolvers.iter().map(|r| r.ip.clone()).collect();
                        return successful_probe(
                            service,
                            "dns-resolver",
                            Some(query_name),
                            observed_ips,
                            resolvers,
                            start.elapsed(),
                        );
                    }
                    Ok(_) if start.elapsed() + SURFSHARK_RETRY_DELAY < timeout => {
                        tokio::time::sleep(SURFSHARK_RETRY_DELAY).await;
                    }
                    Ok(_) => {
                        return failed_probe(
                            service,
                            "dns-resolver",
                            Some(query_name),
                            start.elapsed(),
                            "service returned no resolver observations".to_string(),
                        );
                    }
                    Err(err) => {
                        return failed_probe(
                            service,
                            "dns-resolver",
                            Some(query_name),
                            start.elapsed(),
                            err,
                        );
                    }
                }
            }
            Ok(Ok((status, _)))
                if status.is_server_error()
                    && start.elapsed() + SURFSHARK_RETRY_DELAY < timeout =>
            {
                tokio::time::sleep(SURFSHARK_RETRY_DELAY).await;
            }
            Ok(Ok((status, _))) => {
                return failed_probe(
                    service,
                    "dns-resolver",
                    Some(query_name),
                    start.elapsed(),
                    format!("HTTP {}", status.as_u16()),
                );
            }
            Ok(Err(_)) if start.elapsed() + SURFSHARK_RETRY_DELAY < timeout => {
                tokio::time::sleep(SURFSHARK_RETRY_DELAY).await;
            }
            Ok(Err(err)) => {
                return failed_probe(
                    service,
                    "dns-resolver",
                    Some(query_name),
                    start.elapsed(),
                    err.to_string(),
                );
            }
            Err(_) => {
                return failed_probe(
                    service,
                    "dns-resolver",
                    Some(query_name),
                    start.elapsed(),
                    "timeout".to_string(),
                );
            }
        }
    }
}

async fn probe_ip_api_edns_many(
    count: usize,
    proxy: Option<String>,
    timeout: Duration,
) -> Vec<ExternalProbe> {
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..count {
        let proxy = proxy.clone();
        tasks.spawn(async move {
            let probe = probe_ip_api_edns(proxy.as_deref(), timeout).await;
            (index, probe)
        });
    }

    let mut probes = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(result) = result {
            probes.push(result);
        }
    }
    probes.sort_by_key(|(index, _)| *index);
    probes.into_iter().map(|(_, probe)| probe).collect()
}

async fn probe_ip_api_edns(proxy: Option<&str>, timeout: Duration) -> ExternalProbe {
    let service = "ip-api-edns";
    let start = Instant::now();
    let client = match build_http_client(proxy, timeout) {
        Ok(client) => client,
        Err(err) => {
            return failed_probe(
                service,
                "dns-resolver",
                None,
                start.elapsed(),
                err.to_string(),
            );
        }
    };
    let request = async {
        let response = client.get(IP_API_EDNS_URL).send().await?;
        let final_query = response.url().host_str().map(str::to_string);
        let status = response.status();
        let body = response.text().await?;
        Ok::<_, reqwest::Error>((status, final_query, body))
    };
    match tokio::time::timeout(timeout, request).await {
        Ok(Ok((status, query_name, body))) if status.is_success() => {
            match parse_ip_api_edns_response(&body) {
                Ok(resolver) => successful_probe(
                    service,
                    "dns-resolver",
                    query_name.as_deref(),
                    vec![resolver.ip.clone()],
                    vec![resolver],
                    start.elapsed(),
                ),
                Err(err) => failed_probe(
                    service,
                    "dns-resolver",
                    query_name.as_deref(),
                    start.elapsed(),
                    err,
                ),
            }
        }
        Ok(Ok((status, query_name, _))) => failed_probe(
            service,
            "dns-resolver",
            query_name.as_deref(),
            start.elapsed(),
            format!("HTTP {}", status.as_u16()),
        ),
        Ok(Err(err)) => failed_probe(
            service,
            "dns-resolver",
            err.url().and_then(reqwest::Url::host_str),
            start.elapsed(),
            err.to_string(),
        ),
        Err(_) => failed_probe(
            service,
            "dns-resolver",
            None,
            start.elapsed(),
            "timeout".to_string(),
        ),
    }
}

async fn probe_akamai_whoami(timeout: Duration) -> ExternalProbe {
    let start = Instant::now();
    let resolver = match system_resolver() {
        Ok(resolver) => resolver,
        Err(err) => {
            return failed_probe(
                WHOAMI_DOMAIN,
                "dns-resolver",
                Some(WHOAMI_DOMAIN),
                start.elapsed(),
                err,
            );
        }
    };
    match tokio::time::timeout(timeout, resolver.lookup_ip(WHOAMI_DOMAIN)).await {
        Ok(Ok(lookup)) => {
            let observed_ips = dedup_strings(lookup.iter().map(|ip| ip.to_string()).collect());
            if observed_ips.is_empty() {
                return failed_probe(
                    WHOAMI_DOMAIN,
                    "dns-resolver",
                    Some(WHOAMI_DOMAIN),
                    start.elapsed(),
                    "no resolver IP returned".to_string(),
                );
            }
            let resolvers = observed_ips
                .iter()
                .map(|ip| ResolverObservation {
                    source: WHOAMI_DOMAIN.to_string(),
                    ip: ip.clone(),
                    isp: None,
                    country: None,
                    city: None,
                    country_code: None,
                    provider_leak: None,
                })
                .collect();
            successful_probe(
                WHOAMI_DOMAIN,
                "dns-resolver",
                Some(WHOAMI_DOMAIN),
                observed_ips,
                resolvers,
                start.elapsed(),
            )
        }
        Ok(Err(err)) => failed_probe(
            WHOAMI_DOMAIN,
            "dns-resolver",
            Some(WHOAMI_DOMAIN),
            start.elapsed(),
            err.to_string(),
        ),
        Err(_) => failed_probe(
            WHOAMI_DOMAIN,
            "dns-resolver",
            Some(WHOAMI_DOMAIN),
            start.elapsed(),
            "timeout".to_string(),
        ),
    }
}

async fn probe_cloudflare_trace(proxy: Option<&str>, timeout: Duration) -> ExternalProbe {
    let service = "cloudflare-trace";
    let start = Instant::now();
    let client = match build_http_client(proxy, timeout) {
        Ok(client) => client,
        Err(err) => {
            return failed_probe(
                service,
                "http-egress",
                None,
                start.elapsed(),
                err.to_string(),
            );
        }
    };
    let request = async {
        let response = client.get(CLOUDFLARE_TRACE_URL).send().await?;
        let status = response.status();
        let body = response.text().await?;
        Ok::<_, reqwest::Error>((status, body))
    };
    match tokio::time::timeout(timeout, request).await {
        Ok(Ok((status, body))) if status.is_success() => match parse_trace_ip(&body) {
            Some(ip) => successful_probe(
                service,
                "http-egress",
                None,
                vec![ip],
                Vec::new(),
                start.elapsed(),
            ),
            None => failed_probe(
                service,
                "http-egress",
                None,
                start.elapsed(),
                "response did not contain an egress IP".to_string(),
            ),
        },
        Ok(Ok((status, _))) => failed_probe(
            service,
            "http-egress",
            None,
            start.elapsed(),
            format!("HTTP {}", status.as_u16()),
        ),
        Ok(Err(err)) => failed_probe(
            service,
            "http-egress",
            None,
            start.elapsed(),
            err.to_string(),
        ),
        Err(_) => failed_probe(
            service,
            "http-egress",
            None,
            start.elapsed(),
            "timeout".to_string(),
        ),
    }
}

fn build_http_client(
    proxy: Option<&str>,
    timeout: Duration,
) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(3)))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("netutils dns-leak");
    if let Some(proxy_url) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy_url)?);
    } else {
        builder = builder.no_proxy();
    }
    builder.build()
}

fn system_resolver() -> Result<TokioAsyncResolver, String> {
    TokioAsyncResolver::tokio_from_system_conf()
        .map_err(|err| format!("failed to load system resolver configuration: {err}"))
}

fn surfshark_query_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = u128::from(PROBE_COUNTER.fetch_add(1, Ordering::Relaxed));
    let process = u128::from(std::process::id());
    let token = nanos ^ (process << 64) ^ counter;
    format!("{token:032x}.{SURFSHARK_DOMAIN_SUFFIX}")
}

fn parse_surfshark_response(body: &str) -> Result<Vec<ResolverObservation>, String> {
    let payload: BTreeMap<String, SurfsharkResolverPayload> =
        serde_json::from_str(body).map_err(|err| format!("invalid Surfshark response: {err}"))?;
    let mut resolvers = Vec::new();
    for (key, value) in payload {
        let ip = value.ip.as_deref().unwrap_or(&key);
        if ip.parse::<IpAddr>().is_err() {
            continue;
        }
        resolvers.push(ResolverObservation {
            source: "surfshark-dns".to_string(),
            ip: ip.to_string(),
            isp: non_empty(value.isp),
            country: non_empty(value.country),
            city: non_empty(value.city),
            country_code: non_empty(value.country_code),
            provider_leak: value.leak,
        });
    }
    Ok(resolvers)
}

fn parse_ip_api_edns_response(body: &str) -> Result<ResolverObservation, String> {
    let payload: IpApiEdnsResponse =
        serde_json::from_str(body).map_err(|err| format!("invalid ip-api response: {err}"))?;
    if payload.dns.ip.parse::<IpAddr>().is_err() {
        return Err("ip-api response contained an invalid resolver IP".to_string());
    }
    let (country, isp) = payload
        .dns
        .geo
        .split_once(" - ")
        .map(|(country, isp)| (display_value(country), display_value(isp)))
        .unwrap_or_else(|| (display_value(&payload.dns.geo), None));
    Ok(ResolverObservation {
        source: "ip-api-edns".to_string(),
        ip: payload.dns.ip,
        isp: isp.map(str::to_string),
        country: country.map(str::to_string),
        city: None,
        country_code: None,
        provider_leak: None,
    })
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn parse_trace_ip(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let value = line.trim().strip_prefix("ip=")?.trim();
        value.parse::<IpAddr>().ok().map(|_| value.to_string())
    })
}

fn successful_probe(
    service: &str,
    kind: &str,
    query_name: Option<&str>,
    observed_ips: Vec<String>,
    resolvers: Vec<ResolverObservation>,
    elapsed: Duration,
) -> ExternalProbe {
    ExternalProbe {
        service: service.to_string(),
        kind: kind.to_string(),
        query_name: query_name.map(str::to_string),
        observed_ips,
        resolvers,
        ok: true,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        error: None,
    }
}

fn failed_probe(
    service: &str,
    kind: &str,
    query_name: Option<&str>,
    elapsed: Duration,
    error: String,
) -> ExternalProbe {
    ExternalProbe {
        service: service.to_string(),
        kind: kind.to_string(),
        query_name: query_name.map(str::to_string),
        observed_ips: Vec::new(),
        resolvers: Vec::new(),
        ok: false,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        error: Some(error),
    }
}

fn dedup_strings(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn collect_resolver_ips(probes: &[ExternalProbe]) -> Vec<String> {
    dedup_strings(
        probes
            .iter()
            .filter(|probe| probe.kind == "dns-resolver")
            .flat_map(|probe| probe.observed_ips.iter().cloned())
            .collect(),
    )
}

fn surfshark_leak_signal(probes: &[ExternalProbe]) -> Option<bool> {
    let mut found = false;
    for leak in probes
        .iter()
        .filter(|probe| probe.service == "surfshark-dns")
        .flat_map(|probe| probe.resolvers.iter())
        .filter_map(|resolver| resolver.provider_leak)
    {
        found = true;
        if leak {
            return Some(true);
        }
    }
    found.then_some(false)
}

fn is_remote_dns_probe(probe: &ExternalProbe) -> bool {
    matches!(probe.service.as_str(), "surfshark-dns" | "ip-api-edns")
}

fn remote_dns_resolvers(probes: &[ExternalProbe]) -> Vec<&ResolverObservation> {
    probes
        .iter()
        .filter(|probe| is_remote_dns_probe(probe) && probe.ok)
        .flat_map(|probe| probe.resolvers.iter())
        .collect()
}

fn remote_dns_location_divergence(probes: &[ExternalProbe]) -> bool {
    let resolvers = remote_dns_resolvers(probes);
    if resolvers.len() < 2 {
        return false;
    }

    let mut countries = HashSet::new();
    let mut cities_by_country: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for resolver in resolvers {
        let Some(country) = resolver_country_key(resolver) else {
            continue;
        };
        countries.insert(country.clone());
        if let Some(city) = resolver.city.as_deref().and_then(normalized_value) {
            cities_by_country.entry(country).or_default().insert(city);
        }
    }

    countries.len() > 1 || cities_by_country.values().any(|cities| cities.len() > 1)
}

fn remote_dns_locations_complete(probes: &[ExternalProbe]) -> bool {
    let resolvers = remote_dns_resolvers(probes);
    !resolvers.is_empty()
        && resolvers
            .iter()
            .all(|resolver| resolver_country_key(resolver).is_some())
}

fn remote_dns_probes_complete(probes: &[ExternalProbe]) -> bool {
    let remote_probes = probes
        .iter()
        .filter(|probe| is_remote_dns_probe(probe))
        .collect::<Vec<_>>();
    let has_surfshark = remote_probes
        .iter()
        .any(|probe| probe.service == "surfshark-dns");
    let has_ip_api = remote_probes
        .iter()
        .any(|probe| probe.service == "ip-api-edns");
    has_surfshark
        && has_ip_api
        && remote_probes
            .iter()
            .all(|probe| probe.ok && !probe.observed_ips.is_empty())
}

fn remote_dns_location_labels(probes: &[ExternalProbe]) -> Vec<String> {
    let mut labels = HashSet::new();
    for resolver in remote_dns_resolvers(probes) {
        let country = resolver
            .country
            .as_deref()
            .or(resolver.country_code.as_deref())
            .and_then(display_value);
        let city = resolver.city.as_deref().and_then(display_value);
        let label = match (city, country) {
            (Some(city), Some(country)) if city.eq_ignore_ascii_case(country) => {
                country.to_string()
            }
            (Some(city), Some(country)) => format!("{city}, {country}"),
            (None, Some(country)) => country.to_string(),
            (Some(city), None) => city.to_string(),
            (None, None) => continue,
        };
        labels.insert(label);
    }
    let mut labels = labels.into_iter().collect::<Vec<_>>();
    labels.sort();
    labels
}

fn resolver_country_key(resolver: &ResolverObservation) -> Option<String> {
    resolver
        .country_code
        .as_deref()
        .or(resolver.country.as_deref())
        .and_then(normalized_value)
}

fn normalized_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_ascii_lowercase())
}

fn display_value(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn assess(
    tun_mode: bool,
    proxy_active: bool,
    proxy_remote_dns: bool,
    resolver_path: &[ResolverPathEntry],
    external_probes: &[ExternalProbe],
    no_external: bool,
) -> String {
    let protected_mode = tun_mode || proxy_active;
    let diverted_count = resolver_path
        .iter()
        .filter(|entry| entry.diverted == Some(true))
        .count();
    let known_count = resolver_path
        .iter()
        .filter(|entry| entry.diverted.is_some())
        .count();
    let surfshark_leak = surfshark_leak_signal(external_probes);
    let remote_location_divergence =
        proxy_remote_dns && remote_dns_location_divergence(external_probes);

    if remote_location_divergence {
        return "high".to_string();
    }
    if proxy_remote_dns {
        if remote_dns_probes_complete(external_probes)
            && remote_dns_locations_complete(external_probes)
        {
            return "none".to_string();
        }
        return "low".to_string();
    }

    if protected_mode && surfshark_leak == Some(true) && diverted_count > 0 {
        return "high".to_string();
    }
    if protected_mode
        && (surfshark_leak == Some(true) || (diverted_count > 0 && surfshark_leak.is_none()))
    {
        return "medium".to_string();
    }
    if protected_mode && surfshark_leak == Some(false) && diverted_count > 0 {
        return "low".to_string();
    }
    if protected_mode && (no_external || surfshark_leak.is_none()) {
        return "low".to_string();
    }
    if !protected_mode && surfshark_leak == Some(true) {
        return "low".to_string();
    }
    if !protected_mode && diverted_count > 0 {
        return "low".to_string();
    }
    if resolver_path.is_empty() || known_count == 0 {
        return "low".to_string();
    }
    if !no_external {
        let resolver_probe_ok = external_probes.iter().any(|probe| {
            probe.kind == "dns-resolver" && probe.ok && !probe.observed_ips.is_empty()
        });
        if !resolver_probe_ok {
            return "low".to_string();
        }
    }
    "none".to_string()
}

fn assessment_message(
    risk_level: &str,
    tun_mode: bool,
    proxy_active: bool,
    proxy_remote_dns: bool,
    resolver_path: &[ResolverPathEntry],
    external_probes: &[ExternalProbe],
    no_external: bool,
) -> String {
    let diverted_servers = resolver_path
        .iter()
        .filter(|entry| entry.diverted == Some(true))
        .map(|entry| entry.server.as_str())
        .collect::<Vec<_>>();
    let provider_signal = surfshark_leak_signal(external_probes);
    let locations = remote_dns_location_labels(external_probes);
    let location_text = if locations.is_empty() {
        "unknown".to_string()
    } else {
        locations.join("; ")
    };
    match risk_level {
        "high" if proxy_remote_dns && remote_dns_location_divergence(external_probes) => format!(
            "Remote DNS leak detected: the proxy returned resolver IPs in different geographic locations ({location_text})."
        ),
        "high" => format!(
            "Likely DNS leak: Surfshark flagged the observed resolver path and DNS server(s) {} route outside the selected egress interface.",
            diverted_servers.join(", ")
        ),
        "medium" if provider_signal == Some(true) => {
            "Possible DNS leak: Surfshark flagged at least one observed resolver while VPN/TUN/proxy protection is active. Review the resolver ISP and location below."
                .to_string()
        }
        "medium" => format!(
            "Possible DNS path diversion: protected mode is active and DNS server(s) {} use a different local interface, but no external resolver confirmation was available.",
            diverted_servers.join(", ")
        ),
        "low" if provider_signal == Some(false) && !diverted_servers.is_empty() => {
            "Local DNS routes differ from the selected egress, but Surfshark did not flag the externally observed resolver path. Treat the local entries as configuration candidates, not proof of a leak."
                .to_string()
        }
        "low" if !tun_mode && !proxy_active && !diverted_servers.is_empty() => {
            "DNS servers are configured on multiple local interfaces. No VPN/TUN/proxy protection was detected, so this is informational rather than a confirmed leak."
                .to_string()
        }
        "low" if provider_signal == Some(true) && !tun_mode && !proxy_active => {
            "Surfshark exposed the current resolver path and returned Leak=true, but no VPN/TUN/proxy protection was detected. This is a privacy observation, not proof that a protected tunnel leaked."
                .to_string()
        }
        "low" => {
            if proxy_remote_dns {
                "Remote DNS is active, but one or more resolver locations are missing, so geographic consistency cannot be verified."
                    .to_string()
            } else {
                "DNS leak status is inconclusive because resolver, route, or external probe evidence is incomplete."
                    .to_string()
            }
        }
        _ if no_external => {
            "No local DNS route mismatch was detected. External resolver observation was skipped."
                .to_string()
        }
        _ if proxy_remote_dns => format!(
            "Remote DNS is working: all observed proxy resolver IPs are in the same geographic location ({location_text})."
        ),
        _ => "No DNS leak evidence was detected on the observed resolver and egress paths."
            .to_string(),
    }
}

fn build_notes(
    no_external: bool,
    tun_mode: bool,
    proxy_active: bool,
    proxy_remote_dns: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    if tun_mode {
        notes.push(
            "TUN/VPN mode is active; resolver observations are evaluated against the selected egress path."
                .to_string(),
        );
    }
    if proxy_active {
        notes.push(
            "Surfshark and ip-api-edns hostname requests use the selected proxy, so HTTP and socks5h proxies can expose proxy-side DNS behavior."
                .to_string(),
        );
    }
    if proxy_remote_dns {
        notes.push(
            "Surfshark and ip-api-edns resolver samples are combined; different countries or known cities are classified as a DNS leak."
                .to_string(),
        );
        notes.push(
            "For remote-DNS proxies, Surfshark's provider-defined Leak flag is displayed but does not override geographic consistency."
                .to_string(),
        );
    }
    if no_external {
        notes.push(
            "External probes were skipped (--no-external); local DNS routes alone cannot prove which resolver handled a query."
                .to_string(),
        );
    } else {
        notes.push(
            "Random *.ipv4.surfsharkdns.com hostnames and ip-api-edns redirect tokens report resolver IP and location."
                .to_string(),
        );
        notes.push(
            "whoami.akamai.net A records provide a secondary resolver observation; cloudflare-trace reports only the HTTP egress IP."
                .to_string(),
        );
    }
    notes.push(
        "System DNS lists may include inactive, scoped, split-DNS, or local stub resolvers; a different interface is a candidate signal, not proof by itself."
            .to_string(),
    );
    notes.push(
        "Browser DoH/DoT and application-specific resolvers can differ from this command's resolver path."
            .to_string(),
    );
    notes
}

fn print_report(report: &DnsLeakReport) {
    println!();
    println!("{}", "DNS Leak Test".bold());

    println!();
    println!("{}", "Environment".bold());
    println!(
        "  Egress Interface: {}",
        report.egress_interface.as_deref().unwrap_or("--").green()
    );
    println!(
        "  Egress IP:        {}",
        report.egress_ip.as_deref().unwrap_or("--").yellow()
    );
    println!(
        "  TUN/VPN Mode:     {}",
        if report.tun_mode {
            "yes".green().bold().to_string()
        } else {
            "no".normal().to_string()
        }
    );
    println!(
        "  Proxy:            {}",
        report.proxy.as_deref().unwrap_or("none")
    );
    println!(
        "  Proxy DNS Mode:   {}",
        report.proxy_dns_mode.as_deref().unwrap_or("--")
    );
    println!("  Surfshark Probes: {}", report.surfshark_probe_count);
    println!("  ip-api-edns Probes: {}", report.ip_api_edns_probe_count);

    println!();
    println!("{}", "System DNS Servers".bold());
    if report.system_dns_servers.is_empty() {
        println!("  <none detected>");
    } else {
        let rows = report
            .system_dns_servers
            .iter()
            .map(|server| {
                let status = match server.matches_egress {
                    Some(true) => "match".green().to_string(),
                    Some(false) => "different".yellow().to_string(),
                    None => "unknown".dimmed().to_string(),
                };
                vec![
                    server.server.clone(),
                    server
                        .configured_interface
                        .clone()
                        .unwrap_or_else(|| "--".to_string()),
                    server
                        .route_interface
                        .clone()
                        .unwrap_or_else(|| "--".to_string()),
                    server.gateway.clone().unwrap_or_else(|| "--".to_string()),
                    server.source.clone(),
                    status,
                ]
            })
            .collect::<Vec<_>>();
        print_table(
            &[
                "Server",
                "Configured Iface",
                "Route Iface",
                "Gateway",
                "Source",
                "Egress",
            ],
            &rows,
        );
    }

    if !report.external_probes.is_empty() {
        println!();
        println!("{}", "External Probes".bold());
        let rows = report
            .external_probes
            .iter()
            .map(|probe| {
                let status = if probe.ok {
                    "ok".green().to_string()
                } else {
                    "failed".red().to_string()
                };
                vec![
                    probe.service.clone(),
                    probe
                        .query_name
                        .as_deref()
                        .map(query_token)
                        .unwrap_or_else(|| "--".to_string()),
                    probe.kind.clone(),
                    value_or_dash(&probe.observed_ips.join(", ")),
                    status,
                    format!("{:.0}ms", probe.elapsed_ms),
                    probe.error.clone().unwrap_or_else(|| "--".to_string()),
                ]
            })
            .collect::<Vec<_>>();
        print_table(
            &[
                "Service",
                "Query Token",
                "Kind",
                "Observed IPs",
                "Status",
                "Latency",
                "Error",
            ],
            &rows,
        );

        let resolver_rows = report
            .external_probes
            .iter()
            .flat_map(|probe| probe.resolvers.iter())
            .map(|resolver| {
                let location = [resolver.city.as_deref(), resolver.country.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ");
                let provider_leak = match resolver.provider_leak {
                    Some(true) => "yes".red().to_string(),
                    Some(false) => "no".green().to_string(),
                    None => "--".to_string(),
                };
                vec![
                    resolver.source.clone(),
                    resolver.ip.clone(),
                    resolver.isp.clone().unwrap_or_else(|| "--".to_string()),
                    value_or_dash(&location),
                    provider_leak,
                ]
            })
            .collect::<Vec<_>>();
        if !resolver_rows.is_empty() {
            println!();
            println!("{}", "Observed DNS Resolvers".bold());
            print_table(
                &["Source", "Resolver IP", "ISP", "Location", "Provider Leak"],
                &resolver_rows,
            );
        }

        if let Some(ip) = &report.egress_public_ip {
            println!();
            println!("  Public Egress IP: {}", ip.yellow());
        }
    }

    println!();
    println!("{}", "Assessment".bold());
    let risk_colored = match report.risk_level.as_str() {
        "high" => report.risk_level.red().bold().to_string(),
        "medium" | "low" => report.risk_level.yellow().to_string(),
        _ => report.risk_level.green().to_string(),
    };
    println!("  Risk Level: {}", risk_colored);
    println!("  {}", report.assessment);

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn query_token(query_name: &str) -> String {
    query_name
        .split('.')
        .next()
        .map(|token| token.chars().take(12).collect())
        .unwrap_or_else(|| "--".to_string())
}

fn value_or_dash(value: &str) -> String {
    if value.is_empty() {
        "--".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_entry(server: &str, route: Option<&str>, egress: Option<&str>) -> ResolverPathEntry {
        ResolverPathEntry {
            server: server.to_string(),
            route_interface: route.map(str::to_string),
            egress_interface: egress.map(str::to_string),
            diverted: match (route, egress) {
                (Some(route), Some(egress)) => Some(route != egress),
                _ => None,
            },
        }
    }

    fn surfshark_probe(leak: bool) -> ExternalProbe {
        successful_probe(
            "surfshark-dns",
            "dns-resolver",
            Some("token.ipv4.surfsharkdns.com"),
            vec!["203.0.113.10".to_string()],
            vec![ResolverObservation {
                source: "surfshark-dns".to_string(),
                ip: "203.0.113.10".to_string(),
                isp: Some("Example ISP".to_string()),
                country: Some("Example".to_string()),
                city: Some("Example City".to_string()),
                country_code: Some("EX".to_string()),
                provider_leak: Some(leak),
            }],
            Duration::from_millis(10),
        )
    }

    #[test]
    fn assess_high_when_protected_provider_flag_and_diverted_route() {
        let paths = vec![path_entry("8.8.8.8", Some("eth0"), Some("tun0"))];
        assert_eq!(
            assess(true, false, false, &paths, &[surfshark_probe(true)], false,),
            "high"
        );
    }

    #[test]
    fn assess_none_when_remote_proxy_resolver_location_is_consistent() {
        let paths = vec![path_entry("8.8.8.8", Some("eth0"), Some("eth0"))];
        let probes = vec![surfshark_probe(true), ip_api_probe("EX", "203.0.113.20")];
        assert_eq!(assess(false, true, true, &paths, &probes, false), "none");
    }

    #[test]
    fn assess_medium_when_tun_route_differs_and_external_skipped() {
        let paths = vec![path_entry("8.8.8.8", Some("eth0"), Some("tun0"))];
        assert_eq!(assess(true, false, false, &paths, &[], true), "medium");
    }

    #[test]
    fn assess_low_for_multi_interface_dns_without_protected_mode() {
        let paths = vec![path_entry("8.8.8.8", Some("eth0"), Some("wifi0"))];
        assert_eq!(assess(false, false, false, &paths, &[], true), "low");
    }

    #[test]
    fn assess_low_for_provider_flag_without_protected_mode() {
        let paths = vec![path_entry("8.8.8.8", Some("eth0"), Some("eth0"))];
        assert_eq!(
            assess(false, false, false, &paths, &[surfshark_probe(true)], false,),
            "low"
        );
    }

    #[test]
    fn assess_none_when_surfshark_does_not_flag_resolver() {
        let paths = vec![path_entry("8.8.8.8", Some("tun0"), Some("tun0"))];
        assert_eq!(
            assess(true, false, false, &paths, &[surfshark_probe(false)], false,),
            "none"
        );
    }

    #[test]
    fn assess_low_when_route_evidence_is_unknown() {
        let paths = vec![path_entry("8.8.8.8", None, Some("tun0"))];
        assert_eq!(
            assess(true, false, false, &paths, &[surfshark_probe(false)], false,),
            "low"
        );
    }

    #[test]
    fn assess_low_when_protected_surfshark_probe_is_unavailable() {
        let paths = vec![path_entry("8.8.8.8", Some("tun0"), Some("tun0"))];
        assert_eq!(assess(true, false, false, &paths, &[], false), "low");
        assert_eq!(assess(true, false, false, &paths, &[], true), "low");
    }

    #[test]
    fn remote_proxy_resolvers_in_different_countries_are_high_risk() {
        let probes = vec![
            surfshark_probe_for_locations(&[("203.0.113.10", "HK", "Hong Kong")]),
            ip_api_probe("Taiwan", "203.0.113.11"),
        ];
        assert!(remote_dns_location_divergence(&probes));
        assert_eq!(assess(false, true, true, &[], &probes, false), "high");
    }

    #[test]
    fn remote_proxy_resolvers_in_different_cities_are_high_risk() {
        let probe = surfshark_probe_for_locations(&[
            ("203.0.113.10", "US", "Los Angeles"),
            ("203.0.113.11", "US", "New York"),
        ]);
        assert!(remote_dns_location_divergence(std::slice::from_ref(&probe)));
        assert_eq!(assess(false, true, true, &[], &[probe], false), "high");
    }

    #[test]
    fn multiple_remote_proxy_resolvers_in_same_city_are_normal() {
        let probes = vec![
            surfshark_probe_for_locations(&[
                ("203.0.113.10", "HK", "Hong Kong"),
                ("203.0.113.11", "HK", "Hong Kong"),
            ]),
            ip_api_probe("HK", "203.0.113.12"),
        ];
        assert!(!remote_dns_location_divergence(&probes));
        assert_eq!(assess(false, true, true, &[], &probes, false), "none");
    }

    #[test]
    fn partial_remote_proxy_probe_failure_is_inconclusive() {
        let success = surfshark_probe_for_locations(&[("203.0.113.10", "HK", "Hong Kong")]);
        let failed = failed_probe(
            "ip-api-edns",
            "dns-resolver",
            Some("failed.edns.ip-api.com"),
            Duration::from_millis(10),
            "timeout".to_string(),
        );
        let probes = vec![success, failed];
        assert!(!remote_dns_probes_complete(&probes));
        assert_eq!(assess(false, true, true, &[], &probes, false), "low");
    }

    #[test]
    fn classifies_proxy_dns_modes() {
        assert_eq!(
            classify_proxy_dns_mode("socks5h://127.0.0.1:1080"),
            "remote"
        );
        assert_eq!(classify_proxy_dns_mode("http://127.0.0.1:7890"), "remote");
        assert_eq!(classify_proxy_dns_mode("socks5://127.0.0.1:1080"), "local");
    }

    fn surfshark_probe_for_locations(locations: &[(&str, &str, &str)]) -> ExternalProbe {
        let resolvers = locations
            .iter()
            .map(|(ip, country_code, city)| ResolverObservation {
                source: "surfshark-dns".to_string(),
                ip: (*ip).to_string(),
                isp: Some("Example ISP".to_string()),
                country: Some((*country_code).to_string()),
                city: Some((*city).to_string()),
                country_code: Some((*country_code).to_string()),
                provider_leak: Some(true),
            })
            .collect::<Vec<_>>();
        successful_probe(
            "surfshark-dns",
            "dns-resolver",
            Some("token.ipv4.surfsharkdns.com"),
            resolvers
                .iter()
                .map(|resolver| resolver.ip.clone())
                .collect(),
            resolvers,
            Duration::from_millis(10),
        )
    }

    fn ip_api_probe(country: &str, ip: &str) -> ExternalProbe {
        successful_probe(
            "ip-api-edns",
            "dns-resolver",
            Some("token.edns.ip-api.com"),
            vec![ip.to_string()],
            vec![ResolverObservation {
                source: "ip-api-edns".to_string(),
                ip: ip.to_string(),
                isp: Some("Example ISP".to_string()),
                country: Some(country.to_string()),
                city: None,
                country_code: None,
                provider_leak: None,
            }],
            Duration::from_millis(10),
        )
    }

    #[test]
    fn parses_surfshark_resolver_response() {
        let body = r#"{
            "202.101.173.180": {
                "ISP": "China Telecom",
                "Country": "China",
                "City": "Hangzhou",
                "IP": "202.101.173.180",
                "Leak": true,
                "CountryCode": "CN"
            }
        }"#;
        let resolvers = parse_surfshark_response(body).unwrap();
        assert_eq!(resolvers.len(), 1);
        assert_eq!(resolvers[0].ip, "202.101.173.180");
        assert_eq!(resolvers[0].isp.as_deref(), Some("China Telecom"));
        assert_eq!(resolvers[0].provider_leak, Some(true));
    }

    #[test]
    fn ignores_invalid_surfshark_resolver_keys() {
        let body = r#"{"not-an-ip":{"Leak":true}}"#;
        assert!(parse_surfshark_response(body).unwrap().is_empty());
    }

    #[test]
    fn parses_ip_api_edns_response() {
        let body = r#"{
            "dns": {
                "geo": "Hong Kong - Google LLC",
                "ip": "172.253.4.29"
            }
        }"#;
        let resolver = parse_ip_api_edns_response(body).unwrap();
        assert_eq!(resolver.ip, "172.253.4.29");
        assert_eq!(resolver.country.as_deref(), Some("Hong Kong"));
        assert_eq!(resolver.isp.as_deref(), Some("Google LLC"));
        assert_eq!(resolver.city, None);
    }

    #[test]
    fn surfshark_query_names_are_unique_and_bounded() {
        let first = surfshark_query_name();
        let second = surfshark_query_name();
        assert_ne!(first, second);
        assert!(first.ends_with(SURFSHARK_DOMAIN_SUFFIX));
        assert!(first.split('.').next().unwrap().len() <= 63);
    }

    #[test]
    fn parses_cloudflare_trace_ip() {
        let body = "fl=123f\nh=1.1.1.1\nip=203.0.113.5\nts=123\nvisit_scheme=https\n";
        assert_eq!(parse_trace_ip(body), Some("203.0.113.5".to_string()));
    }

    #[test]
    fn parse_trace_ip_returns_none_for_invalid_value() {
        assert_eq!(parse_trace_ip("ip=not-an-ip\n"), None);
    }

    #[test]
    fn comparison_preserves_unknown_state() {
        assert_eq!(compare_interfaces(&None, &Some("tun0".to_string())), None);
        assert_eq!(
            compare_interfaces(&Some("tun0".to_string()), &Some("tun0".to_string())),
            Some(true)
        );
    }

    #[test]
    fn local_resolver_hops_are_not_treated_as_diverted() {
        assert!(is_local_resolver_hop(
            "127.0.0.1",
            &Some("Loopback Pseudo-Interface 1".to_string())
        ));
        assert!(is_local_resolver_hop(
            "10.255.255.254",
            &Some("lo".to_string())
        ));
        assert!(!is_local_resolver_hop(
            "192.168.1.1",
            &Some("eth0".to_string())
        ));
    }
}
