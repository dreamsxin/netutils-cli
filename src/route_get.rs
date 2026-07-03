//! Explain how the local routing table selects an egress path for a target.

use std::net::IpAddr;

use colored::*;
use serde::Serialize;

use crate::info::interface::{classify_interface, InterfaceInfo};
use crate::output::{print_json, print_json_error, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
pub struct RouteGetReport {
    pub target: String,
    pub resolved_ips: Vec<String>,
    pub routes: Vec<RouteGetRow>,
    pub trace: Vec<RouteGetTraceHop>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RouteGetRow {
    pub ip: String,
    pub interface: Option<String>,
    pub gateway: Option<String>,
    pub source: Option<String>,
    pub interface_ip: Option<String>,
    pub interface_type: Option<String>,
    pub tun_mode: bool,
}

#[derive(Debug, Serialize)]
pub struct RouteGetTraceHop {
    pub ttl: u32,
    pub ip: Option<String>,
    pub rtt_ms: Option<f64>,
}

pub async fn run(target: &str, max_hops: u32, no_trace: bool, mode: OutputMode) {
    let ips = crate::util::resolve_host_all(target).await;
    if ips.is_empty() {
        let msg = format!("resolve failed: {}", target);
        if mode == OutputMode::Json {
            print_json_error(&msg);
        } else {
            println!("  {}", msg.red());
        }
        return;
    }

    let interfaces = crate::info::collect_interfaces();
    let routes = ips
        .iter()
        .copied()
        .take(12)
        .map(|ip| route_row(ip, &interfaces))
        .collect::<Vec<_>>();
    let trace = if no_trace {
        Vec::new()
    } else {
        trace_for(ips[0], max_hops).await
    };

    let report = RouteGetReport {
        target: target.to_string(),
        resolved_ips: ips.iter().map(IpAddr::to_string).collect(),
        routes,
        trace,
        notes: vec![
            "Route selection is the local kernel decision for the resolved IP, before any remote proxy hop.".to_string(),
            "In TUN mode, the selected interface is usually a utun/tun/tap/WireGuard-like interface even when the physical uplink is Wi-Fi/Ethernet.".to_string(),
        ],
    };

    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn route_row(ip: IpAddr, interfaces: &[InterfaceInfo]) -> RouteGetRow {
    let probe = crate::route_probe::route_to_target(&ip.to_string());
    let (interface, gateway, source) = match probe {
        Some(probe) => (probe.interface, probe.gateway, Some(probe.source)),
        None => (None, None, None),
    };
    let iface = interface
        .as_ref()
        .and_then(|name| interfaces.iter().find(|iface| iface.name == *name));
    let interface_type = iface.map(|iface| iface.iftype.clone());
    let tun_mode = iface
        .map(|iface| classify_interface(&iface.description, &iface.name).is_tun_like())
        .unwrap_or_else(|| {
            interface
                .as_deref()
                .map(|name| classify_interface(name, name).is_tun_like())
                .unwrap_or(false)
        });

    RouteGetRow {
        ip: ip.to_string(),
        interface,
        gateway,
        source,
        interface_ip: iface.map(|iface| iface.ipv4.clone()),
        interface_type,
        tun_mode,
    }
}

async fn trace_for(target: IpAddr, max_hops: u32) -> Vec<RouteGetTraceHop> {
    crate::traceroute::collect_trace_quick(target, max_hops)
        .await
        .into_iter()
        .map(|hop| {
            let probe = hop.probes.iter().find(|probe| probe.ip.is_some());
            RouteGetTraceHop {
                ttl: hop.ttl,
                ip: probe.and_then(|probe| probe.ip.clone()),
                rtt_ms: probe.and_then(|probe| probe.rtt_ms),
            }
        })
        .collect()
}

fn print_report(report: &RouteGetReport) {
    println!();
    println!("{}", "🧭 Route Get".bold());
    println!("  Target: {}", report.target);
    println!("  Resolved: {}", report.resolved_ips.join(", "));

    println!();
    println!("{}", "Kernel Route Decision".bold());
    let rows = report
        .routes
        .iter()
        .map(|route| {
            vec![
                route.ip.clone(),
                route.interface.clone().unwrap_or_else(|| "--".to_string()),
                route.gateway.clone().unwrap_or_else(|| "--".to_string()),
                route
                    .interface_ip
                    .clone()
                    .unwrap_or_else(|| "--".to_string()),
                route
                    .interface_type
                    .clone()
                    .unwrap_or_else(|| "--".to_string()),
                if route.tun_mode {
                    "yes".green().to_string()
                } else {
                    "no".normal().to_string()
                },
                route.source.clone().unwrap_or_else(|| "--".to_string()),
            ]
        })
        .collect::<Vec<_>>();
    print_table(
        &[
            "IP",
            "Interface",
            "Gateway",
            "Iface IP",
            "Type",
            "TUN",
            "Source",
        ],
        &rows,
    );

    if !report.trace.is_empty() {
        println!();
        println!("{}", "Quick Trace".bold());
        let rows = report
            .trace
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
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}
