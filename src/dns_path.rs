//! DNS query path inspection: resolver servers, route to resolver, and direct query test.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;
use trust_dns_resolver::config::*;
use trust_dns_resolver::TokioAsyncResolver;

use crate::output::{print_json, OutputMode};
use crate::table::print_table;

const DNS_PATH_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const DNS_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
pub struct DnsServerPath {
    pub server: String,
    pub configured_interface: Option<String>,
    pub route_interface: Option<String>,
    pub gateway: Option<String>,
    pub source: String,
    pub query: Option<DnsServerQuery>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DnsServerQuery {
    pub ok: bool,
    pub elapsed_ms: f64,
    pub ips: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DnsPathReport {
    pub domain: Option<String>,
    pub default_resolve_ips: Vec<String>,
    pub servers: Vec<DnsServerPath>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DnsServer {
    pub server: String,
    pub interface: Option<String>,
    pub source: String,
}

pub async fn run(domain: Option<String>, server: Option<String>, mode: OutputMode) {
    let mut servers = if let Some(server) = server {
        vec![DnsServer {
            server,
            interface: None,
            source: "argument".to_string(),
        }]
    } else {
        get_dns_servers()
    };
    dedup_servers(&mut servers);

    let default_resolve_ips = match domain.as_deref() {
        Some(domain) => crate::util::resolve_host_all(domain)
            .await
            .into_iter()
            .map(|ip| ip.to_string())
            .collect(),
        None => Vec::new(),
    };

    let mut paths = Vec::new();
    for dns_server in servers {
        let route = crate::route_probe::route_to_target(&dns_server.server);
        let (route_interface, gateway) = match route {
            Some(route) => (route.interface, route.gateway),
            None => (None, None),
        };
        let query = match domain.as_deref() {
            Some(domain) => Some(query_via_server(domain, &dns_server.server).await),
            None => None,
        };
        paths.push(DnsServerPath {
            server: dns_server.server,
            configured_interface: dns_server.interface,
            route_interface,
            gateway,
            source: dns_server.source,
            query,
        });
    }

    let report = DnsPathReport {
        domain,
        default_resolve_ips,
        servers: paths,
        notes: vec![
            "This shows the local OS route to each DNS server, not every upstream hop.".to_string(),
            "DoH/DoT, browser DNS, and proxy-side DNS may bypass the OS DNS server list."
                .to_string(),
        ],
    };

    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn print_report(report: &DnsPathReport) {
    println!();
    println!("{}", "🧭 DNS Query Path".bold());
    if let Some(domain) = &report.domain {
        println!("  Domain: {}", domain);
    }
    if !report.default_resolve_ips.is_empty() {
        println!(
            "  Default resolve: {}",
            report.default_resolve_ips.join(", ")
        );
    }

    println!();
    println!("{}", "DNS Servers".bold());
    if report.servers.is_empty() {
        println!("  <none detected>");
    } else {
        let rows = report
            .servers
            .iter()
            .map(|server| {
                let query = server
                    .query
                    .as_ref()
                    .map(format_query)
                    .unwrap_or_else(|| "--".to_string());
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
                    query,
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
                "Query",
            ],
            &rows,
        );
    }

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn format_query(query: &DnsServerQuery) -> String {
    if query.ok {
        format!("{} ({:.0}ms)", query.ips.join(", "), query.elapsed_ms)
    } else {
        format!(
            "failed: {} ({:.0}ms)",
            query.error.as_deref().unwrap_or("unknown"),
            query.elapsed_ms
        )
    }
}

fn dedup_servers(servers: &mut Vec<DnsServer>) {
    let mut seen = HashSet::new();
    servers.retain(|server| seen.insert(server.server.clone()));
}

fn is_placeholder_dns_server(server: &str) -> bool {
    matches!(
        server.trim().to_ascii_lowercase().as_str(),
        "fec0:0:0:ffff::1" | "fec0:0:0:ffff::2" | "fec0:0:0:ffff::3"
    )
}

pub fn get_dns_servers() -> Vec<DnsServer> {
    #[cfg(target_os = "windows")]
    {
        return get_dns_servers_windows();
    }
    #[cfg(target_os = "macos")]
    {
        return get_dns_servers_macos();
    }
    #[cfg(target_os = "linux")]
    {
        return get_dns_servers_linux();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

#[cfg(target_os = "windows")]
fn get_dns_servers_windows() -> Vec<DnsServer> {
    let script = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-DnsClientServerAddress -AddressFamily IPv4, IPv6 -ErrorAction SilentlyContinue | ForEach-Object {
  $iface = $_.InterfaceAlias
  foreach ($server in $_.ServerAddresses) {
    if ($server) { "$iface|$server" }
  }
}
"#;
    let Some(output) = crate::util::powershell_output(script, DNS_PATH_COMMAND_TIMEOUT) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (iface, server) = line.trim().split_once('|')?;
            if server.trim().is_empty() || is_placeholder_dns_server(server) {
                return None;
            }
            Some(DnsServer {
                server: server.trim().to_string(),
                interface: Some(iface.trim().to_string()),
                source: "Get-DnsClientServerAddress".to_string(),
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn get_dns_servers_macos() -> Vec<DnsServer> {
    let Some(output) =
        crate::util::command_output_timeout("scutil", &["--dns"], DNS_PATH_COMMAND_TIMEOUT)
    else {
        return Vec::new();
    };
    let mut servers = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
    {
        if let Some((_, server)) = line.split_once(':') {
            if line.starts_with("nameserver[") && !server.trim().is_empty() {
                servers.push(DnsServer {
                    server: server.trim().to_string(),
                    interface: None,
                    source: "scutil --dns".to_string(),
                });
            }
        }
    }
    servers
}

#[cfg(target_os = "linux")]
fn get_dns_servers_linux() -> Vec<DnsServer> {
    let mut servers = resolvectl_dns_servers();
    if servers.is_empty() {
        servers = resolv_conf_dns_servers();
    }
    servers
}

#[cfg(target_os = "linux")]
fn resolvectl_dns_servers() -> Vec<DnsServer> {
    let Some(output) =
        crate::util::command_output_timeout("resolvectl", &["dns"], DNS_PATH_COMMAND_TIMEOUT)
    else {
        return Vec::new();
    };
    let mut servers = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
    {
        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        let iface = left
            .split('(')
            .nth(1)
            .and_then(|v| v.split(')').next())
            .map(str::to_string);
        for server in right.split_whitespace() {
            servers.push(DnsServer {
                server: server.to_string(),
                interface: iface.clone(),
                source: "resolvectl dns".to_string(),
            });
        }
    }
    servers
}

#[cfg(target_os = "linux")]
fn resolv_conf_dns_servers() -> Vec<DnsServer> {
    let Ok(text) = std::fs::read_to_string("/etc/resolv.conf") else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("nameserver "))
        .map(str::trim)
        .filter(|server| !server.is_empty())
        .map(|server| DnsServer {
            server: server.to_string(),
            interface: None,
            source: "/etc/resolv.conf".to_string(),
        })
        .collect()
}

pub async fn query_via_server(domain: &str, server: &str) -> DnsServerQuery {
    let start = Instant::now();
    let ip = match server.parse::<IpAddr>() {
        Ok(ip) => ip,
        Err(err) => {
            return DnsServerQuery {
                ok: false,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                ips: Vec::new(),
                error: Some(err.to_string()),
            }
        }
    };
    let resolver = resolver_for_server(ip);
    match tokio::time::timeout(DNS_QUERY_TIMEOUT, resolver.lookup_ip(domain)).await {
        Ok(Ok(lookup)) => {
            let ips = lookup.iter().map(|ip| ip.to_string()).collect::<Vec<_>>();
            DnsServerQuery {
                ok: !ips.is_empty(),
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
                ips,
                error: None,
            }
        }
        Ok(Err(err)) => DnsServerQuery {
            ok: false,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            ips: Vec::new(),
            error: Some(err.to_string()),
        },
        Err(_) => DnsServerQuery {
            ok: false,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            ips: Vec::new(),
            error: Some("timeout".to_string()),
        },
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

#[cfg(test)]
mod tests {
    use super::is_placeholder_dns_server;

    #[test]
    fn filters_windows_placeholder_dns_servers() {
        assert!(is_placeholder_dns_server("fec0:0:0:ffff::1"));
        assert!(is_placeholder_dns_server("FEC0:0:0:FFFF::2"));
        assert!(is_placeholder_dns_server("fec0:0:0:ffff::3"));
        assert!(!is_placeholder_dns_server("8.8.8.8"));
        assert!(!is_placeholder_dns_server("2001:4860:4860::8888"));
    }
}
