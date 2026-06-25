//! Traceroute 模块：TTL 递增探测路由路径。

use std::mem::MaybeUninit;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;

use crate::i18n::{t, t1, t2};
use crate::output::{print_json, print_json_error, OutputMode};
use crate::table::print_table;

use socket2::{Domain, Protocol, Socket, Type};

const MAX_HOPS: u32 = 30;
const PROBES_PER_HOP: u32 = 3;
const TIMEOUT: Duration = Duration::from_secs(2);

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

/// 执行 traceroute 并输出结果
pub async fn run(host: &str, mode: OutputMode) {
    // 解析主机
    let target = match crate::util::resolve_host(host).await {
        Some(ip) => ip,
        None => {
            let msg = t1("trace.resolve_fail", host);
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

    for ttl in 1..=MAX_HOPS {
        let hop = trace_hop(target, ttl).await;
        let is_reached = hop.reached;
        hops.push(hop);

        if is_reached {
            reached_dest = true;
            break;
        }
    }

    let output = TraceOutput {
        host: host.to_string(),
        target: target.to_string(),
        hops: hops.clone(),
    };

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    // 表格输出
    println!();
    println!("{}", t1("trace.title", host).bold());
    println!("  {}", t2("trace.target", host, &target.to_string()));
    println!("  {}", t1("trace.max_hops", &MAX_HOPS.to_string()));
    println!();

    let h_hop = t("trace.hop");
    let h_ip = t("trace.ip");
    let h_p1 = t1("trace.probe", "1");
    let h_p2 = t1("trace.probe", "2");
    let h_p3 = t1("trace.probe", "3");
    let headers = [h_hop.as_str(), h_ip.as_str(), h_p1.as_str(), h_p2.as_str(), h_p3.as_str()];

    let rows: Vec<Vec<String>> = hops
        .iter()
        .map(|hop| {
            let mut row = vec![hop.ttl.to_string()];

            let ip_str = hop
                .probes
                .iter()
                .find_map(|p| p.ip.as_ref().map(|ip| ip.clone()))
                .unwrap_or_else(|| "*".to_string());
            row.push(ip_str);

            for i in 0..PROBES_PER_HOP as usize {
                if let Some(Some(rtt)) = hop.probes.get(i).map(|p| p.rtt_ms) {
                    row.push(format!("{:.2}ms", rtt));
                } else {
                    row.push("*".to_string());
                }
            }

            row
        })
        .collect();

    print_table(&headers, &rows);

    if !reached_dest {
        println!();
        println!("  {}", t1("trace.not_reached", &MAX_HOPS.to_string()).yellow());
    }
}

/// 探测单跳
async fn trace_hop(target: IpAddr, ttl: u32) -> Hop {
    let mut probes = Vec::new();
    let mut reached = false;

    for probe_seq in 0..PROBES_PER_HOP {
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
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)).ok()?;
    socket.set_ttl_v4(ttl).ok()?;
    socket.set_read_timeout(Some(TIMEOUT)).ok()?;

    let ident = (std::process::id() & 0xFFFF) as u16;
    let seq = (probe_seq + ttl * 10) as u16;
    let packet = build_icmp_echo_request(ident, seq);

    let start = Instant::now();
    let dest = SocketAddr::new(IpAddr::V4(target), 0);
    socket.send_to(&packet, &dest.into()).ok()?;

    let mut buf = [MaybeUninit::new(0); 1024];
    loop {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                let from_ip = from.as_socket().map(|s| s.ip())?;
                let data: &[u8] =
                    unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, len) };
                if parse_icmp_response(data, ident, seq).is_some() {
                    return Some((from_ip, start.elapsed()));
                }
            }
            Err(_) => return None,
        }
    }
}

/// 构造 ICMP Echo Request 包
fn build_icmp_echo_request(ident: u16, seq: u16) -> Vec<u8> {
    let mut packet = vec![0u8; 8 + 32];
    packet[0] = 8;
    packet[1] = 0;
    packet[4] = (ident >> 8) as u8;
    packet[5] = (ident & 0xFF) as u8;
    packet[6] = (seq >> 8) as u8;
    packet[7] = (seq & 0xFF) as u8;
    for i in 0..32 {
        packet[8 + i] = i as u8;
    }
    let checksum = icmp_checksum(&packet);
    packet[2] = (checksum >> 8) as u8;
    packet[3] = (checksum & 0xFF) as u8;
    packet
}

fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        let word = ((data[i] as u32) << 8) | (data[i + 1] as u32);
        sum += word;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn parse_icmp_response(buf: &[u8], ident: u16, seq: u16) -> Option<()> {
    if buf.len() < 20 {
        return None;
    }
    let ihl = ((buf[0] & 0x0F) * 4) as usize;
    if buf.len() < ihl + 8 {
        return None;
    }
    let icmp_type = buf[ihl];
    match icmp_type {
        // Echo Reply (type 0) — 验证 ident 和 seq 匹配
        0 => {
            let recv_ident = u16::from_be_bytes([buf[ihl + 4], buf[ihl + 5]]);
            let recv_seq = u16::from_be_bytes([buf[ihl + 6], buf[ihl + 7]]);
            if recv_ident == ident && recv_seq == seq {
                Some(())
            } else {
                None
            }
        }
        // Time Exceeded (type 11) — 包含原始 IP+ICMP 头，验证原始 ident/seq
        11 => {
            // 原始 IP 头在 ICMP 数据部分（offset ihl+8）
            let inner_start = ihl + 8;
            if buf.len() < inner_start + 20 + 8 {
                return Some(()); // 无法解析，仍然接受
            }
            let inner_ihl = ((buf[inner_start] & 0x0F) * 4) as usize;
            let icmp_offset = inner_start + inner_ihl;
            if buf.len() < icmp_offset + 8 {
                return Some(());
            }
            let orig_type = buf[icmp_offset];
            if orig_type != 8 {
                return None; // 不是 Echo Request
            }
            let orig_ident = u16::from_be_bytes([buf[icmp_offset + 4], buf[icmp_offset + 5]]);
            let orig_seq = u16::from_be_bytes([buf[icmp_offset + 6], buf[icmp_offset + 7]]);
            if orig_ident == ident && orig_seq == seq {
                Some(())
            } else {
                None
            }
        }
        _ => None,
    }
}
