//! Traceroute 模块：TTL 递增探测路由路径。

use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;

use crate::i18n::{t, t1, t2};
use crate::output::{print_json, print_json_error, OutputMode};

use socket2::{Domain, Protocol, Socket, Type};

const PROBES_PER_HOP: u32 = 3;
const TIMEOUT: Duration = Duration::from_secs(2);
const PROBE_DEADLINE: Duration = Duration::from_millis(2500);
const MAX_CONSECUTIVE_TIMEOUT_HOPS: u32 = 5;

/// 单次探测结果
#[derive(Serialize, Clone)]
pub struct Probe {
    pub ip: Option<String>,
    pub rtt_ms: Option<f64>,
}

/// 单跳结果
#[derive(Serialize, Clone)]
pub struct Hop {
    pub ttl: u32,
    pub probes: Vec<Probe>,
    pub reached: bool,
}

/// Traceroute 完整输出
#[derive(Serialize)]
pub struct TraceOutput {
    pub host: String,
    pub target: String,
    pub hops: Vec<Hop>,
}

/// 快速收集 traceroute 跳点，每跳只探测一次，适合组合诊断命令。
pub async fn collect_trace_quick(target: IpAddr, max_hops: u32) -> Vec<Hop> {
    collect_trace_with_probe_count(target, max_hops, 1).await
}

async fn collect_trace_with_probe_count(
    target: IpAddr,
    max_hops: u32,
    probes_per_hop: u32,
) -> Vec<Hop> {
    let mut hops = Vec::new();
    let mut consecutive_timeout_hops = 0;

    for ttl in 1..=max_hops {
        let hop = trace_hop_with_probe_count(target, ttl, probes_per_hop).await;
        let reached = hop.reached;
        if hop_all_timed_out(&hop) {
            consecutive_timeout_hops += 1;
        } else {
            consecutive_timeout_hops = 0;
        }
        hops.push(hop);

        if reached || consecutive_timeout_hops >= MAX_CONSECUTIVE_TIMEOUT_HOPS {
            break;
        }
    }

    hops
}

/// 执行 traceroute 并输出结果
pub async fn run(host: &str, max_hops: u32, mode: OutputMode) {
    // 解析主机
    let target = match crate::util::resolve_host(host).await {
        Some(ip) => ip,
        None => {
            let msg = t1("trace.resolve_fail", host);
            crate::output::mark_failure();
            if mode == OutputMode::Json {
                print_json_error(&msg);
            } else {
                println!("  {}", msg.red());
            }
            return;
        }
    };

    let mut hops = Vec::new();
    let mut reached_dest = false;
    let mut stopped_after_timeouts = false;
    let mut consecutive_timeout_hops = 0;

    if mode != OutputMode::Json {
        print_trace_header(host, target, max_hops);
    }

    for ttl in 1..=max_hops {
        let hop = trace_hop(target, ttl).await;
        let is_reached = hop.reached;

        if mode != OutputMode::Json {
            print_hop_row(&hop);
        }

        if hop_all_timed_out(&hop) {
            consecutive_timeout_hops += 1;
        } else {
            consecutive_timeout_hops = 0;
        }

        hops.push(hop);

        if is_reached {
            reached_dest = true;
            break;
        }
        if consecutive_timeout_hops >= MAX_CONSECUTIVE_TIMEOUT_HOPS {
            stopped_after_timeouts = true;
            break;
        }
    }

    let output = TraceOutput {
        host: host.to_string(),
        target: target.to_string(),
        hops: hops.clone(),
    };

    if !reached_dest && output.hops.iter().all(hop_all_timed_out) {
        crate::output::mark_failure();
    }

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    if !reached_dest {
        println!();
        if stopped_after_timeouts {
            println!(
                "  {}",
                t1(
                    "trace.stopped_no_response",
                    &MAX_CONSECUTIVE_TIMEOUT_HOPS.to_string()
                )
                .yellow()
            );
        }
        println!(
            "  {}",
            t1("trace.not_reached", &max_hops.to_string()).yellow()
        );
    }
}

fn print_trace_header(host: &str, target: IpAddr, max_hops: u32) {
    println!();
    println!("{}", t1("trace.title", host).bold());
    println!("  {}", t2("trace.target", host, &target.to_string()));
    println!("  {}", t1("trace.max_hops", &max_hops.to_string()));
    println!();

    let h_hop = t("trace.hop");
    let h_ip = t("trace.ip");
    let h_p1 = t1("trace.probe", "1");
    let h_p2 = t1("trace.probe", "2");
    let h_p3 = t1("trace.probe", "3");
    println!(
        "{:<5} {:<40} {:>12} {:>12} {:>12}",
        h_hop, h_ip, h_p1, h_p2, h_p3
    );
    println!("{:-<5} {:-<40} {:-<12} {:-<12} {:-<12}", "", "", "", "", "");
    let _ = io::stdout().flush();
}

fn print_hop_row(hop: &Hop) {
    let ip_str = hop
        .probes
        .iter()
        .find_map(|p| p.ip.clone())
        .unwrap_or_else(|| "*".to_string());
    let mut probe_cells = Vec::new();
    for i in 0..PROBES_PER_HOP as usize {
        let cell = if let Some(Some(rtt)) = hop.probes.get(i).map(|p| p.rtt_ms) {
            format!("{:.2}ms", rtt)
        } else {
            "*".to_string()
        };
        probe_cells.push(cell);
    }

    println!(
        "{:<5} {:<40} {:>12} {:>12} {:>12}",
        hop.ttl, ip_str, probe_cells[0], probe_cells[1], probe_cells[2]
    );
    let _ = io::stdout().flush();
}

fn hop_all_timed_out(hop: &Hop) -> bool {
    hop.probes
        .iter()
        .all(|probe| probe.ip.is_none() && probe.rtt_ms.is_none())
}

/// 探测单跳
async fn trace_hop(target: IpAddr, ttl: u32) -> Hop {
    trace_hop_with_probe_count(target, ttl, PROBES_PER_HOP).await
}

async fn trace_hop_with_probe_count(target: IpAddr, ttl: u32, probes_per_hop: u32) -> Hop {
    let mut probes = Vec::new();
    let mut reached = false;

    for probe_seq in 0..probes_per_hop {
        match send_probe(target, ttl, probe_seq).await {
            Some((ip, rtt)) => {
                if ip == target {
                    reached = true;
                }
                probes.push(Probe {
                    ip: Some(ip.to_string()),
                    rtt_ms: Some(rtt.as_secs_f64() * 1000.0),
                });
            }
            None => probes.push(Probe {
                ip: None,
                rtt_ms: None,
            }),
        }
    }

    Hop {
        ttl,
        probes,
        reached,
    }
}

/// 发送单个 ICMP 探测包并等待响应
async fn send_probe(target: IpAddr, ttl: u32, probe_seq: u32) -> Option<(IpAddr, Duration)> {
    match target {
        IpAddr::V4(addr) => send_probe_v4(addr, ttl, probe_seq).await,
        IpAddr::V6(_) => None,
    }
}

/// IPv4 ICMP 探测
async fn send_probe_v4(target: Ipv4Addr, ttl: u32, probe_seq: u32) -> Option<(IpAddr, Duration)> {
    tokio::time::timeout(
        PROBE_DEADLINE,
        tokio::task::spawn_blocking(move || send_probe_v4_blocking(target, ttl, probe_seq)),
    )
    .await
    .ok()?
    .ok()?
}

fn send_probe_v4_blocking(
    target: Ipv4Addr,
    ttl: u32,
    probe_seq: u32,
) -> Option<(IpAddr, Duration)> {
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)).ok()?;
    socket.set_ttl_v4(ttl).ok()?;
    socket.set_read_timeout(Some(TIMEOUT)).ok()?;

    let ident = (std::process::id() & 0xFFFF) as u16;
    let seq = (probe_seq + ttl * 10) as u16;
    let packet = crate::icmp::build_icmp_echo_request(ident, seq);

    let start = Instant::now();
    let dest = SocketAddr::new(IpAddr::V4(target), 0);
    socket.send_to(&packet, &dest.into()).ok()?;

    let mut buf = [MaybeUninit::new(0); 1024];
    while start.elapsed() < TIMEOUT {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                let from_ip = from.as_socket().map(|s| s.ip())?;
                let data: &[u8] =
                    unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, len) };
                if crate::icmp::parse_icmp_response(data, ident, seq).is_some() {
                    return Some((from_ip, start.elapsed()));
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }

    None
}
