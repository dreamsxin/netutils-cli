//! Compare system DNS resolution with direct queries against DNS servers.

use std::collections::{HashSet, VecDeque};

use colored::*;
use serde::Serialize;

use crate::dns_path::{get_dns_servers, query_via_server, DnsServerQuery};
use crate::output::{print_json, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
pub struct DnsCompareReport {
    pub domain: String,
    pub default_ips: Vec<String>,
    pub results: Vec<DnsCompareRow>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DnsCompareRow {
    pub source: String,
    pub server: String,
    pub route_interface: Option<String>,
    pub gateway: Option<String>,
    pub ok: bool,
    pub elapsed_ms: f64,
    pub ips: Vec<String>,
    pub diff: String,
    pub error: Option<String>,
}

pub async fn run(domain: &str, servers: Vec<String>, mode: OutputMode) {
    let default_ips = crate::util::resolve_host_all(domain)
        .await
        .into_iter()
        .map(|ip| ip.to_string())
        .collect::<Vec<_>>();
    let default_set = ip_set(&default_ips);

    let mut candidates = VecDeque::new();
    for server in servers {
        candidates.push_back((server, "argument".to_string()));
    }
    if candidates.is_empty() {
        for server in get_dns_servers() {
            candidates.push_back((server.server, server.source));
        }
    }

    let mut seen = HashSet::new();
    let mut results = Vec::new();
    while let Some((server, source)) = candidates.pop_front() {
        if !seen.insert(server.clone()) {
            continue;
        }
        let query = query_via_server(domain, &server).await;
        let route = crate::route_probe::route_to_target(&server);
        results.push(row_from_query(
            source,
            server,
            route.and_then(|route| Some((route.interface, route.gateway))),
            query,
            &default_set,
        ));
    }

    let report = DnsCompareReport {
        domain: domain.to_string(),
        default_ips,
        results,
        notes: vec![
            "Default resolve uses the resolver path chosen by the OS/runtime; direct rows query a specific DNS server over UDP/53.".to_string(),
            "Different answers can be normal for CDN, split DNS, ECS, proxy DNS, or stale local cache.".to_string(),
        ],
    };

    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn row_from_query(
    source: String,
    server: String,
    route: Option<(Option<String>, Option<String>)>,
    query: DnsServerQuery,
    default_set: &HashSet<String>,
) -> DnsCompareRow {
    let current_set = ip_set(&query.ips);
    let diff = if !query.ok {
        "failed".to_string()
    } else if current_set == *default_set {
        "same".to_string()
    } else if current_set.is_empty() {
        "empty".to_string()
    } else {
        "different".to_string()
    };
    let (route_interface, gateway) = route.unwrap_or((None, None));
    DnsCompareRow {
        source,
        server,
        route_interface,
        gateway,
        ok: query.ok,
        elapsed_ms: query.elapsed_ms,
        ips: query.ips,
        diff,
        error: query.error,
    }
}

fn ip_set(ips: &[String]) -> HashSet<String> {
    ips.iter().cloned().collect()
}

fn print_report(report: &DnsCompareReport) {
    println!();
    println!("{}", "🧪 DNS Compare".bold());
    println!("  Domain: {}", report.domain);
    if report.default_ips.is_empty() {
        println!("  Default resolve: {}", "<failed>".red());
    } else {
        println!("  Default resolve: {}", report.default_ips.join(", "));
    }

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
                    format!("{:.0}ms", row.elapsed_ms),
                    colored_diff(&row.diff),
                    if row.ok {
                        row.ips.join(", ")
                    } else {
                        format!("failed: {}", row.error.as_deref().unwrap_or("unknown"))
                    },
                ]
            })
            .collect::<Vec<_>>();
        print_table(
            &[
                "Server",
                "Route Iface",
                "Gateway",
                "Source",
                "RTT",
                "Diff",
                "IPs",
            ],
            &rows,
        );
    }

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn colored_diff(diff: &str) -> String {
    match diff {
        "same" => diff.green().to_string(),
        "different" => diff.yellow().to_string(),
        "failed" => diff.red().to_string(),
        _ => diff.normal().to_string(),
    }
}
