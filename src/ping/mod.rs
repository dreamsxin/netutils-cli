//! Ping 模块：ICMP ping，无权限时回退 TCP ping。

use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;

use crate::i18n::t;
use crate::output::{print_json, print_json_error, OutputMode};
use crate::table::print_table;

/// 单次 ping 结果
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub seq: u32,
    pub success: bool,
    pub rtt_ms: Option<f64>,
    pub error: Option<String>,
}

/// Ping 统计
#[derive(Serialize)]
pub struct PingStats {
    pub sent: usize,
    pub received: usize,
    pub lost: usize,
    pub loss_rate: f64,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub avg_ms: Option<f64>,
}

/// Ping 完整输出
#[derive(Serialize)]
pub struct PingOutput {
    pub host: String,
    pub target: String,
    pub probes: Vec<ProbeResult>,
    pub stats: PingStats,
}

/// 执行 ping 并输出结果
pub async fn run(host: &str, count: u32, mode: OutputMode) {
    // 解析主机
    let target = match crate::util::resolve_host(host).await {
        Some(ip) => ip,
        None => {
            let msg = t("ping.resolve_fail").replace("{0}", host);
            if mode == OutputMode::Json {
                print_json_error(&msg);
            } else {
                println!("  {}", msg.red());
            }
            return;
        }
    };

    // 先尝试 ICMP ping
    let probes = match surge_ping_probe(target, count).await {
        Some(r) => r,
        None => {
            if mode == OutputMode::Table {
                println!("  {}", t("ping.icmp_fallback").yellow());
            }
            tcp_ping_probe(target, count).await
        }
    };

    let stats = compute_stats(&probes);
    let output = PingOutput {
        host: host.to_string(),
        target: target.to_string(),
        probes: probes.clone(),
        stats,
    };

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    // 表格输出
    println!();
    println!("{}", t("ping.title").replace("{0}", host).bold());
    println!("  {}", t("ping.target").replace("{0}", host).replace("{1}", &target.to_string()));

    for probe in &probes {
        print_ping_line(host, probe);
        if probe.seq + 1 < count {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    print_ping_stats(&output.stats);
}

/// 计算统计
fn compute_stats(probes: &[ProbeResult]) -> PingStats {
    let total = probes.len();
    let success = probes.iter().filter(|r| r.success).count();
    let lost = total - success;
    let loss_rate = if total > 0 {
        (lost as f64 / total as f64) * 100.0
    } else {
        0.0
    };
    let rtts: Vec<f64> = probes.iter().filter_map(|r| r.rtt_ms).collect();
    let stats = crate::util::compute_stats(&rtts);
    PingStats {
        sent: total,
        received: success,
        lost,
        loss_rate,
        min_ms: stats.min_ms,
        max_ms: stats.max_ms,
        avg_ms: stats.avg_ms,
    }
}

/// 打印单行 ping 结果
fn print_ping_line(host: &str, result: &ProbeResult) {
    if result.success {
        if let Some(rtt) = result.rtt_ms {
            println!(
                "  {}",
                t("ping.reply")
                    .replace("{0}", &result.seq.to_string())
                    .replace("{1}", host)
                    .replace("{2}", &format!("{:.2}", rtt))
            );
        }
    } else {
        let unknown = t("common.unknown");
        let err = result.error.as_deref().unwrap_or(&unknown);
        println!(
            "  {}",
            t("ping.fail")
                .replace("{0}", &result.seq.to_string())
                .replace("{1}", err)
                .red()
        );
    }
}

/// 打印 ping 统计结果
fn print_ping_stats(stats: &PingStats) {
    println!();
    println!("{}", t("ping.stats").bold());

    let mut rows = Vec::new();
    rows.push(vec![t("ping.sent"), stats.sent.to_string()]);
    rows.push(vec![t("ping.recv"), stats.received.to_string()]);
    rows.push(vec![t("ping.lost"), stats.lost.to_string()]);
    rows.push(vec![t("ping.loss_rate"), format!("{:.1}%", stats.loss_rate)]);

    if let (Some(min), Some(max), Some(avg)) = (stats.min_ms, stats.max_ms, stats.avg_ms) {
        rows.push(vec![t("ping.min"), format!("{:.2}ms", min)]);
        rows.push(vec![t("ping.max"), format!("{:.2}ms", max)]);
        rows.push(vec![t("ping.avg"), format!("{:.2}ms", avg)]);
    }

    let h0 = t("common.metric");
    let h1 = t("proxy.value");
    print_table(&[h0.as_str(), h1.as_str()], &rows);
}

/// ICMP ping（需要权限），返回探测结果
async fn surge_ping_probe(target: std::net::IpAddr, count: u32) -> Option<Vec<ProbeResult>> {
    use surge_ping::{Client, ConfigBuilder, PingIdentifier, PingSequence};

    let client = match Client::new(&ConfigBuilder::default().build()) {
        Ok(c) => c,
        Err(_) => return None,
    };

    let identifier = PingIdentifier(std::process::id() as u16);
    let mut results = Vec::new();

    for seq in 0..count {
        let payload = [0u8; 32];
        let mut pinger = client.pinger(target, identifier).await;
        let result = pinger.ping(PingSequence(seq as u16), &payload).await;

        match result {
            Ok((_, rtt)) => {
                results.push(ProbeResult {
                    seq,
                    success: true,
                    rtt_ms: Some(rtt.as_secs_f64() * 1000.0),
                    error: None,
                });
            }
            Err(e) => {
                results.push(ProbeResult {
                    seq,
                    success: false,
                    rtt_ms: None,
                    error: Some(format!("{}", e)),
                });
            }
        }
    }

    Some(results)
}

/// TCP ping 回退方案（连接 80 端口测延迟）
async fn tcp_ping_probe(target: std::net::IpAddr, count: u32) -> Vec<ProbeResult> {
    use tokio::net::TcpStream;

    let mut results = Vec::new();

    for seq in 0..count {
        let start = Instant::now();
        let addr = format!("{}:80", target);
        let result = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(&addr)).await;

        match result {
            Ok(Ok(_stream)) => {
                let rtt = start.elapsed();
                results.push(ProbeResult {
                    seq,
                    success: true,
                    rtt_ms: Some(rtt.as_secs_f64() * 1000.0),
                    error: None,
                });
            }
            Ok(Err(e)) => {
                results.push(ProbeResult {
                    seq,
                    success: false,
                    rtt_ms: None,
                    error: Some(format!("TCP: {}", e)),
                });
            }
            Err(_) => {
                results.push(ProbeResult {
                    seq,
                    success: false,
                    rtt_ms: None,
                    error: Some(t("ping.timeout")),
                });
            }
        }
    }

    results
}
