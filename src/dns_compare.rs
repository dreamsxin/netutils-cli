//! Compare system DNS resolution with direct queries against DNS servers.

use std::collections::{HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;
use trust_dns_resolver::config::*;
use trust_dns_resolver::proto::rr::RData;
use trust_dns_resolver::TokioAsyncResolver;

use crate::dns_path::get_dns_servers;
use crate::output::{print_json, OutputMode};
use crate::table::print_table;

const DNS_COMPARE_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize)]
pub struct DnsCompareReport {
    pub domain: String,
    pub default_a: DnsFamilyResult,
    pub default_aaaa: DnsFamilyResult,
    pub default_path_a: DefaultResolverPath,
    pub default_path_aaaa: DefaultResolverPath,
    pub results: Vec<DnsCompareRow>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DefaultResolverPath {
    pub family: String,
    pub status: String,
    pub matches: Vec<DefaultResolverMatch>,
}

#[derive(Debug, Serialize)]
pub struct DefaultResolverMatch {
    pub server: String,
    pub route_interface: Option<String>,
    pub gateway: Option<String>,
    pub source: String,
    pub elapsed_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct DnsCompareRow {
    pub source: String,
    pub server: String,
    pub route_interface: Option<String>,
    pub gateway: Option<String>,
    pub a: DnsFamilyResult,
    pub aaaa: DnsFamilyResult,
    pub a_diff: String,
    pub aaaa_diff: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DnsFamilyResult {
    pub family: String,
    pub ok: bool,
    pub elapsed_ms: f64,
    pub values: Vec<String>,
    pub answer_count: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DnsFamily {
    A,
    Aaaa,
}

impl DnsFamily {
    fn label(self) -> &'static str {
        match self {
            DnsFamily::A => "A",
            DnsFamily::Aaaa => "AAAA",
        }
    }

    fn record_type(self) -> trust_dns_resolver::proto::rr::RecordType {
        match self {
            DnsFamily::A => trust_dns_resolver::proto::rr::RecordType::A,
            DnsFamily::Aaaa => trust_dns_resolver::proto::rr::RecordType::AAAA,
        }
    }
}

pub async fn run(domain: &str, servers: Vec<String>, mode: OutputMode) {
    let (default_a, default_aaaa) = tokio::join!(
        query_default_family(domain, DnsFamily::A),
        query_default_family(domain, DnsFamily::Aaaa)
    );

    let mut candidates = VecDeque::new();
    for server in servers {
        candidates.push_back((server, "argument".to_string()));
    }
    if candidates.is_empty() {
        for server in get_dns_servers() {
            candidates.push_back((server.server, server.source));
        }
    }

    let default_a_set = value_set(&default_a.values);
    let default_aaaa_set = value_set(&default_aaaa.values);

    let mut seen = HashSet::new();
    let mut queries = tokio::task::JoinSet::new();
    let mut index = 0usize;
    while let Some((server, source)) = candidates.pop_front() {
        if !seen.insert(server.clone()) {
            continue;
        }
        if index >= 16 {
            break;
        }
        let domain = domain.to_string();
        let task_index = index;
        queries.spawn(async move {
            let (a, aaaa) = tokio::join!(
                query_via_server_family(&domain, &server, DnsFamily::A),
                query_via_server_family(&domain, &server, DnsFamily::Aaaa)
            );
            let route_server = server.clone();
            let route = tokio::task::spawn_blocking(move || {
                crate::route_probe::route_to_target(&route_server)
            })
            .await
            .ok()
            .flatten();
            (task_index, server, source, a, aaaa, route)
        });
        index += 1;
    }

    let mut completed = Vec::new();
    while let Some(result) = queries.join_next().await {
        if let Ok(result) = result {
            completed.push(result);
        }
    }
    completed.sort_by_key(|result| result.0);

    let mut results = Vec::new();
    for (_, server, source, a, aaaa, route) in completed {
        let (route_interface, gateway) = match route {
            Some(route) => (route.interface, route.gateway),
            None => (None, None),
        };
        results.push(DnsCompareRow {
            source,
            server,
            route_interface,
            gateway,
            a_diff: diff_against_default(&a, &default_a_set),
            aaaa_diff: diff_against_default(&aaaa, &default_aaaa_set),
            a,
            aaaa,
        });
    }

    let report = DnsCompareReport {
        domain: domain.to_string(),
        default_path_a: infer_default_path(&results, DnsFamily::A),
        default_path_aaaa: infer_default_path(&results, DnsFamily::Aaaa),
        default_a,
        default_aaaa,
        results,
        notes: vec![
            "Default resolve uses the operating-system resolver; direct rows query a specific DNS server over UDP/53.".to_string(),
            "A and AAAA are compared separately because IPv4 and IPv6 answers often differ in CDN, split DNS, ECS, proxy DNS, or local cache behavior.".to_string(),
        ],
    };

    if !report.default_a.ok && !report.default_aaaa.ok {
        crate::output::mark_failure();
    }

    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

async fn query_default_family(domain: &str, family: DnsFamily) -> DnsFamilyResult {
    query_system_family(domain, family).await
}

async fn query_system_family(domain: &str, family: DnsFamily) -> DnsFamilyResult {
    let start = Instant::now();
    let host = domain.to_string();
    let lookup = tokio::time::timeout(
        DNS_COMPARE_QUERY_TIMEOUT,
        tokio::task::spawn_blocking(move || (host.as_str(), 0).to_socket_addrs()),
    )
    .await;

    let values = match lookup {
        Ok(Ok(Ok(addrs))) => dedup_values(
            addrs
                .filter_map(|addr| match (family, addr.ip()) {
                    (DnsFamily::A, IpAddr::V4(ip)) => Some(ip.to_string()),
                    (DnsFamily::Aaaa, IpAddr::V6(ip)) => Some(ip.to_string()),
                    _ => None,
                })
                .collect(),
        ),
        Ok(Ok(Err(err))) => {
            return failed_family_result(family, start.elapsed(), err.to_string());
        }
        Ok(Err(err)) => {
            return failed_family_result(family, start.elapsed(), err.to_string());
        }
        Err(_) => {
            return failed_family_result(family, start.elapsed(), "timeout".to_string());
        }
    };

    ok_family_result(family, start.elapsed(), values)
}

async fn query_via_server_family(domain: &str, server: &str, family: DnsFamily) -> DnsFamilyResult {
    let start = Instant::now();
    let ip = match server.parse::<IpAddr>() {
        Ok(ip) => ip,
        Err(err) => {
            return failed_family_result(family, start.elapsed(), err.to_string());
        }
    };
    let resolver = resolver_for_server(ip);
    query_with_resolver(&resolver, domain, family).await
}

async fn query_with_resolver(
    resolver: &TokioAsyncResolver,
    domain: &str,
    family: DnsFamily,
) -> DnsFamilyResult {
    let start = Instant::now();
    match tokio::time::timeout(
        DNS_COMPARE_QUERY_TIMEOUT,
        resolver.lookup(domain, family.record_type()),
    )
    .await
    {
        Ok(Ok(lookup)) => {
            let values = dedup_values(
                lookup
                    .record_iter()
                    .filter_map(|record| match record.data() {
                        Some(RData::A(addr)) if family == DnsFamily::A => Some(addr.0.to_string()),
                        Some(RData::AAAA(addr)) if family == DnsFamily::Aaaa => {
                            Some(addr.0.to_string())
                        }
                        _ => None,
                    })
                    .collect(),
            );
            ok_family_result(family, start.elapsed(), values)
        }
        Ok(Err(err)) => failed_family_result(family, start.elapsed(), err.to_string()),
        Err(_) => failed_family_result(family, start.elapsed(), "timeout".to_string()),
    }
}

fn resolver_for_server(server: IpAddr) -> TokioAsyncResolver {
    let name_server = NameServerConfig {
        socket_addr: SocketAddr::new(server, 53),
        protocol: Protocol::Udp,
        tls_dns_name: None,
        trust_negative_responses: false,
        bind_addr: None,
    };
    let config = ResolverConfig::from_parts(None, vec![], vec![name_server]);
    TokioAsyncResolver::tokio(config, ResolverOpts::default())
}

fn ok_family_result(family: DnsFamily, elapsed: Duration, values: Vec<String>) -> DnsFamilyResult {
    DnsFamilyResult {
        family: family.label().to_string(),
        ok: true,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        answer_count: values.len(),
        values,
        error: None,
    }
}

fn failed_family_result(family: DnsFamily, elapsed: Duration, error: String) -> DnsFamilyResult {
    DnsFamilyResult {
        family: family.label().to_string(),
        ok: false,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        values: Vec::new(),
        answer_count: 0,
        error: Some(error),
    }
}

fn diff_against_default(result: &DnsFamilyResult, default_set: &HashSet<String>) -> String {
    let current_set = value_set(&result.values);
    if !result.ok {
        "failed".to_string()
    } else if current_set == *default_set && !current_set.is_empty() {
        "same".to_string()
    } else if current_set == *default_set {
        "same-empty".to_string()
    } else if current_set.is_empty() {
        "empty".to_string()
    } else {
        "different".to_string()
    }
}

fn value_set(values: &[String]) -> HashSet<String> {
    values.iter().cloned().collect()
}

fn dedup_values(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn infer_default_path(results: &[DnsCompareRow], family: DnsFamily) -> DefaultResolverPath {
    let matches = results
        .iter()
        .filter(|row| {
            let (result, diff) = match family {
                DnsFamily::A => (&row.a, row.a_diff.as_str()),
                DnsFamily::Aaaa => (&row.aaaa, row.aaaa_diff.as_str()),
            };
            result.ok && matches!(diff, "same" | "same-empty")
        })
        .map(|row| {
            let elapsed_ms = match family {
                DnsFamily::A => row.a.elapsed_ms,
                DnsFamily::Aaaa => row.aaaa.elapsed_ms,
            };
            DefaultResolverMatch {
                server: row.server.clone(),
                route_interface: row.route_interface.clone(),
                gateway: row.gateway.clone(),
                source: row.source.clone(),
                elapsed_ms,
            }
        })
        .collect::<Vec<_>>();

    let status = match matches.len() {
        0 => "unmatched",
        1 => "unique",
        _ => "ambiguous",
    }
    .to_string();

    DefaultResolverPath {
        family: family.label().to_string(),
        status,
        matches,
    }
}

fn print_report(report: &DnsCompareReport) {
    println!();
    println!("{}", "🧪 DNS Compare".bold());
    println!("  Domain: {}", report.domain);

    println!();
    println!("{}", "Default Resolve".bold());
    println!("  A: {}", format_family_summary(&report.default_a));
    println!("  AAAA: {}", format_family_summary(&report.default_aaaa));

    println!();
    println!("{}", "Default Resolver Path".bold());
    print_default_path("A", &report.default_path_a);
    print_default_path("AAAA", &report.default_path_aaaa);

    println!();
    println!("{}", "Direct DNS Queries".bold());
    if report.results.is_empty() {
        println!("  <no DNS servers detected>");
    } else {
        let rows = report
            .results
            .iter()
            .map(|row| {
                vec![
                    row.server.clone(),
                    row.route_interface
                        .clone()
                        .unwrap_or_else(|| "--".to_string()),
                    row.gateway.clone().unwrap_or_else(|| "--".to_string()),
                    row.source.clone(),
                    colored_diff(&row.a_diff),
                    format_family_cell(&row.a),
                    colored_diff(&row.aaaa_diff),
                    format_family_cell(&row.aaaa),
                ]
            })
            .collect::<Vec<_>>();
        print_table(
            &[
                "Server",
                "Route Iface",
                "Gateway",
                "Source",
                "A Diff",
                "A",
                "AAAA Diff",
                "AAAA",
            ],
            &rows,
        );
    }

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn print_default_path(label: &str, path: &DefaultResolverPath) {
    match path.status.as_str() {
        "unique" => {
            let matched = &path.matches[0];
            println!("  {}: {}", label, format_match(matched));
        }
        "ambiguous" => {
            println!("  {}: multiple matching DNS servers", label);
            for matched in &path.matches {
                println!("    {}", format_match(matched));
            }
        }
        _ => {
            println!(
                "  {}: unable to map the default resolve to a single configured DNS server",
                label
            );
        }
    }
}

fn format_match(matched: &DefaultResolverMatch) -> String {
    format!(
        "{} (iface {}, gateway {}, {}, {:.0}ms)",
        matched.server,
        matched.route_interface.as_deref().unwrap_or("--"),
        matched.gateway.as_deref().unwrap_or("--"),
        matched.source,
        matched.elapsed_ms
    )
}

fn format_family_summary(result: &DnsFamilyResult) -> String {
    if result.ok {
        if result.values.is_empty() {
            format!("<empty> ({:.0}ms)", result.elapsed_ms)
        } else {
            format!(
                "{} [{}] ({:.0}ms)",
                result.values.join(", "),
                result.answer_count,
                result.elapsed_ms
            )
        }
    } else {
        format!(
            "failed: {} ({:.0}ms)",
            result.error.as_deref().unwrap_or("unknown"),
            result.elapsed_ms
        )
    }
}

fn format_family_cell(result: &DnsFamilyResult) -> String {
    if result.ok {
        if result.values.is_empty() {
            format!("<empty> ({:.0}ms)", result.elapsed_ms)
        } else {
            format!(
                "{} [{}] ({:.0}ms)",
                result.values.join(", "),
                result.answer_count,
                result.elapsed_ms
            )
        }
    } else {
        format!(
            "failed: {} ({:.0}ms)",
            result.error.as_deref().unwrap_or("unknown"),
            result.elapsed_ms
        )
    }
}

fn colored_diff(diff: &str) -> String {
    match diff {
        "same" | "same-empty" => diff.green().to_string(),
        "different" => diff.yellow().to_string(),
        "failed" => diff.red().to_string(),
        _ => diff.normal().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        diff_against_default, infer_default_path, value_set, DefaultResolverPath, DnsCompareRow,
        DnsFamily, DnsFamilyResult,
    };

    fn family_result(family: DnsFamily, ok: bool, values: &[&str]) -> DnsFamilyResult {
        DnsFamilyResult {
            family: family.label().to_string(),
            ok,
            elapsed_ms: 12.0,
            values: values.iter().map(|value| value.to_string()).collect(),
            answer_count: values.len(),
            error: if ok { None } else { Some("failed".to_string()) },
        }
    }

    fn row(
        server: &str,
        a_diff: &str,
        aaaa_diff: &str,
        a: DnsFamilyResult,
        aaaa: DnsFamilyResult,
    ) -> DnsCompareRow {
        DnsCompareRow {
            source: "test".to_string(),
            server: server.to_string(),
            route_interface: Some("eth0".to_string()),
            gateway: Some("192.168.1.1".to_string()),
            a,
            aaaa,
            a_diff: a_diff.to_string(),
            aaaa_diff: aaaa_diff.to_string(),
        }
    }

    fn infer(results: &[DnsCompareRow], family: DnsFamily) -> DefaultResolverPath {
        infer_default_path(results, family)
    }

    #[test]
    fn infers_unique_default_path_per_family() {
        let results = vec![
            row(
                "8.8.8.8",
                "same",
                "different",
                family_result(DnsFamily::A, true, &["1.1.1.1"]),
                family_result(DnsFamily::Aaaa, true, &["2001::1"]),
            ),
            row(
                "1.1.1.1",
                "different",
                "same",
                family_result(DnsFamily::A, true, &["2.2.2.2"]),
                family_result(DnsFamily::Aaaa, true, &["2001::2"]),
            ),
        ];
        let inferred_a = infer(&results, DnsFamily::A);
        let inferred_aaaa = infer(&results, DnsFamily::Aaaa);
        assert_eq!(inferred_a.status, "unique");
        assert_eq!(inferred_a.matches[0].server, "8.8.8.8");
        assert_eq!(inferred_aaaa.status, "unique");
        assert_eq!(inferred_aaaa.matches[0].server, "1.1.1.1");
    }

    #[test]
    fn infers_ambiguous_default_path() {
        let results = vec![
            row(
                "8.8.8.8",
                "same",
                "failed",
                family_result(DnsFamily::A, true, &["1.1.1.1"]),
                family_result(DnsFamily::Aaaa, false, &[]),
            ),
            row(
                "1.1.1.1",
                "same",
                "failed",
                family_result(DnsFamily::A, true, &["1.1.1.1"]),
                family_result(DnsFamily::Aaaa, false, &[]),
            ),
        ];
        let inferred = infer(&results, DnsFamily::A);
        assert_eq!(inferred.status, "ambiguous");
        assert_eq!(inferred.matches.len(), 2);
    }

    #[test]
    fn infers_unmatched_default_path() {
        let results = vec![row(
            "8.8.8.8",
            "different",
            "failed",
            family_result(DnsFamily::A, true, &["2.2.2.2"]),
            family_result(DnsFamily::Aaaa, false, &[]),
        )];
        let inferred = infer(&results, DnsFamily::A);
        assert_eq!(inferred.status, "unmatched");
        assert!(inferred.matches.is_empty());
    }

    #[test]
    fn computes_same_empty_diff() {
        let result = family_result(DnsFamily::Aaaa, true, &[]);
        let diff = diff_against_default(&result, &value_set(&[]));
        assert_eq!(diff, "same-empty");
    }
}
