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

/// 探测方式：优先 ICMP，无权限时回退 TCP。
///
/// 客户端只创建一次，避免每轮探测重复初始化。
enum Prober {
    Icmp(surge_ping::Client),
    Tcp,
}

impl Prober {
    /// ICMP 客户端创建失败（通常是缺少权限）时回退到 TCP。
    fn new() -> Self {
        match surge_ping::Client::new(&surge_ping::ConfigBuilder::default().build()) {
            Ok(client) => Prober::Icmp(client),
            Err(_) => Prober::Tcp,
        }
    }

    fn is_fallback(&self) -> bool {
        matches!(self, Prober::Tcp)
    }

    async fn probe(&self, target: std::net::IpAddr, seq: u32, timeout: Duration) -> ProbeResult {
        match self {
            Prober::Icmp(client) => icmp_probe_once(client, target, seq, timeout).await,
            Prober::Tcp => tcp_probe_once(target, seq, timeout).await,
        }
    }
}

/// 执行 ping 并输出结果。
///
/// `count == 0` 表示持续探测直到 Ctrl-C。
pub async fn run(host: &str, count: u32, timeout: Duration, interval: Duration, mode: OutputMode) {
    // 解析主机
    let target = match crate::util::resolve_host(host).await {
        Some(ip) => ip,
        None => {
            let msg = t("ping.resolve_fail").replace("{0}", host);
            crate::output::mark_failure();
            if mode == OutputMode::Json {
                print_json_error(&msg);
            } else {
                println!("  {}", msg.red());
            }
            return;
        }
    };

    let continuous = count == 0;
    let prober = Prober::new();

    if mode == OutputMode::Table {
        println!();
        println!("{}", t("ping.title").replace("{0}", host).bold());
        println!(
            "  {}",
            t("ping.target")
                .replace("{0}", host)
                .replace("{1}", &target.to_string())
        );
        if prober.is_fallback() {
            println!("  {}", t("ping.icmp_fallback").yellow());
        }
    }

    let mut probes: Vec<ProbeResult> = Vec::new();
    let mut seq = 0u32;
    loop {
        if !continuous && seq >= count {
            break;
        }

        let probe = prober.probe(target, seq, timeout).await;

        // 逐轮输出：此前是「全部探测完再打印」，`--interval` 只延迟了打印。
        match mode {
            OutputMode::Table => print_ping_line(host, &probe),
            // 持续模式下单个 JSON 对象永远不会结束，因此按 NDJSON 逐行输出。
            // 有界模式保持原来的「末尾单个 JSON 对象」契约不变。
            OutputMode::Json if continuous => print_json_line(&probe),
            OutputMode::Json => {}
        }
        probes.push(probe);
        seq += 1;

        let has_next = continuous || seq < count;
        if !has_next {
            break;
        }
        if sleep_or_interrupt(interval, continuous).await {
            break;
        }
    }

    let stats = compute_stats(&probes);
    let output = PingOutput {
        host: host.to_string(),
        target: target.to_string(),
        probes,
        stats,
    };

    if output.stats.received == 0 {
        crate::output::mark_failure();
    }

    match mode {
        // 持续模式已经逐行输出过探测结果，末尾只补一份汇总。
        OutputMode::Json if continuous => print_json(&output.stats),
        OutputMode::Json => print_json(&output),
        OutputMode::Table => print_ping_stats(&output.stats),
    }
}

/// 等待下一轮探测；持续模式下同时监听 Ctrl-C。
///
/// 返回 `true` 表示被中断，应当结束循环并输出汇总。
async fn sleep_or_interrupt(interval: Duration, continuous: bool) -> bool {
    if !continuous {
        tokio::time::sleep(interval).await;
        return false;
    }
    tokio::select! {
        _ = tokio::time::sleep(interval) => false,
        // Ctrl-C 时正常收尾并打印统计，而不是让进程被直接杀掉、丢掉已采集的数据。
        _ = tokio::signal::ctrl_c() => true,
    }
}

/// 输出一行 NDJSON。
fn print_json_line(probe: &ProbeResult) {
    match serde_json::to_string(probe) {
        Ok(line) => println!("{line}"),
        Err(err) => eprintln!("failed to serialize probe: {err}"),
    }
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
    rows.push(vec![
        t("ping.loss_rate"),
        format!("{:.1}%", stats.loss_rate),
    ]);

    if let (Some(min), Some(max), Some(avg)) = (stats.min_ms, stats.max_ms, stats.avg_ms) {
        rows.push(vec![t("ping.min"), format!("{:.2}ms", min)]);
        rows.push(vec![t("ping.max"), format!("{:.2}ms", max)]);
        rows.push(vec![t("ping.avg"), format!("{:.2}ms", avg)]);
    }

    let h0 = t("common.metric");
    let h1 = t("proxy.value");
    print_table(&[h0.as_str(), h1.as_str()], &rows);
}

/// 单次 ICMP 探测
async fn icmp_probe_once(
    client: &surge_ping::Client,
    target: std::net::IpAddr,
    seq: u32,
    timeout: Duration,
) -> ProbeResult {
    use surge_ping::{PingIdentifier, PingSequence};

    let identifier = PingIdentifier(std::process::id() as u16);
    let timeout = timeout.max(Duration::from_millis(1));
    let payload = [0u8; 32];
    let mut pinger = client.pinger(target, identifier).await;

    match tokio::time::timeout(timeout, pinger.ping(PingSequence(seq as u16), &payload)).await {
        Ok(Ok((_, rtt))) => ProbeResult {
            seq,
            success: true,
            rtt_ms: Some(rtt.as_secs_f64() * 1000.0),
            error: None,
        },
        Ok(Err(e)) => ProbeResult {
            seq,
            success: false,
            rtt_ms: None,
            error: Some(format!("{}", e)),
        },
        Err(_) => ProbeResult {
            seq,
            success: false,
            rtt_ms: None,
            error: Some(t("ping.timeout")),
        },
    }
}

/// 单次 TCP 探测（连接 80 端口测延迟）
async fn tcp_probe_once(target: std::net::IpAddr, seq: u32, timeout: Duration) -> ProbeResult {
    use std::net::SocketAddr;
    use tokio::net::TcpStream;

    let start = Instant::now();
    let addr = SocketAddr::new(target, 80);

    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(Ok(_stream)) => ProbeResult {
            seq,
            success: true,
            rtt_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
            error: None,
        },
        Ok(Err(e)) => ProbeResult {
            seq,
            success: false,
            rtt_ms: None,
            error: Some(format!("TCP: {}", e)),
        },
        Err(_) => ProbeResult {
            seq,
            success: false,
            rtt_ms: None,
            error: Some(t("ping.timeout")),
        },
    }
}

/// 批量 ICMP ping（需要权限），返回探测结果；客户端创建失败时返回 None。
///
/// 与 `run` 共用 [`icmp_probe_once`]，避免两处各写一遍探测逻辑。
pub(crate) async fn surge_ping_probe(
    target: std::net::IpAddr,
    count: u32,
    timeout: Duration,
) -> Option<Vec<ProbeResult>> {
    let client = surge_ping::Client::new(&surge_ping::ConfigBuilder::default().build()).ok()?;
    let mut results = Vec::new();
    for seq in 0..count {
        results.push(icmp_probe_once(&client, target, seq, timeout).await);
    }
    Some(results)
}

/// 批量 TCP ping 回退方案
pub(crate) async fn tcp_ping_probe(
    target: std::net::IpAddr,
    count: u32,
    timeout: Duration,
) -> Vec<ProbeResult> {
    let mut results = Vec::new();
    for seq in 0..count {
        results.push(tcp_probe_once(target, seq, timeout).await);
    }
    results
}
