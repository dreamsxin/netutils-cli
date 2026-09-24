//! 端口扫描模块：并发 TCP connect 扫描。

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use colored::*;
use serde::Serialize;

use crate::i18n::{t, t1, t2};
use crate::output::{print_json, print_json_error, OutputMode};
use crate::table::print_table;

use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::time::timeout;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

const COMMON_PORTS: &[(u16, &str)] = &[
    (21, "FTP"),
    (22, "SSH"),
    (23, "Telnet"),
    (25, "SMTP"),
    (53, "DNS"),
    (80, "HTTP"),
    (110, "POP3"),
    (143, "IMAP"),
    (443, "HTTPS"),
    (445, "SMB"),
    (993, "IMAPS"),
    (995, "POP3S"),
    (1433, "SQL Server"),
    (3306, "MySQL"),
    (3389, "RDP"),
    (5432, "PostgreSQL"),
    (6379, "Redis"),
    (8080, "HTTP Alt"),
    (8443, "HTTPS Alt"),
    (9090, "Prometheus"),
];

/// 单个端口扫描结果
#[derive(Serialize, Clone)]
pub struct PortResult {
    pub port: u16,
    /// 该端口探测**发起**时刻（RFC 3339 UTC）
    pub ts: String,
    pub open: bool,
    pub ip: Option<String>,
    pub service: String,
}

/// 端口扫描完整输出
#[derive(Serialize)]
pub struct ScanOutput {
    pub host: String,
    pub target: String,
    /// 本次扫描覆盖的时间窗口（RFC 3339 UTC）
    pub started_at: String,
    pub finished_at: String,
    pub total_scanned: usize,
    pub open_count: usize,
    pub results: Vec<PortResult>,
    /// 主机本身无法扫描时的原因（如解析失败）；正常扫描时不出现
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 批量扫描结果。独立类型，单主机的 JSON 结构因此不会被批量改动波及。
#[derive(Serialize)]
pub struct ScanBatchOutput {
    pub mode: String,
    pub started_at: String,
    pub finished_at: String,
    pub stats: ScanBatchStats,
    /// 被 Ctrl-C 打断时为 true：此时 results 只覆盖已派发的主机
    pub interrupted: bool,
    pub results: Vec<ScanOutput>,
}

#[derive(Serialize)]
pub struct ScanBatchStats {
    pub hosts: usize,
    pub with_open: usize,
    pub without_open: usize,
}

/// 执行端口扫描并输出结果
pub async fn run(host: &str, ports: Option<&[u16]>, concurrency: usize, mode: OutputMode) {
    match probe_host(host, ports, concurrency).await {
        Ok(output) => render(&output, concurrency, mode),
        Err(msg) => report_host_error(&msg, mode),
    }
}

/// 批量扫描：跑完清单里的每台主机，共用同一份端口集合。
///
/// `parallel == 1` 时串行，每台主机的完整表格实时打出。`parallel > 1` 时并发，
/// 单台主机的表格会交错到不可读，因此改为**每台完成即打印一行**带计数器的
/// 结果——降粒度，而不是缓冲到最后（见 spec R9）。
pub async fn run_batch(
    hosts: &[String],
    ports: Option<&[u16]>,
    concurrency: usize,
    parallel: usize,
    mode: OutputMode,
) {
    let started_at = crate::timestamp::now_rfc3339_millis();
    let total = hosts.len();

    if mode == OutputMode::Table {
        println!();
        println!("{}", t1("scan.batch_title", &total.to_string()).bold());
    }

    let (results, interrupted) = if parallel > 1 {
        let shared_ports = ports.map(|p| Arc::new(p.to_vec()));
        let done = Arc::new(AtomicUsize::new(0));
        let run = crate::batch::run_targets(hosts.to_vec(), parallel, move |_, host| {
            let shared_ports = shared_ports.clone();
            let done = done.clone();
            async move {
                let slice = shared_ports.as_ref().map(|p| p.as_slice());
                let output = normalize(&host, probe_host(&host, slice, concurrency).await);
                if mode == OutputMode::Table {
                    let seq = done.fetch_add(1, Ordering::Relaxed) + 1;
                    print_progress_line(seq, total, &output);
                }
                output
            }
        })
        .await;
        (run.results, run.interrupted)
    } else {
        let mut results = Vec::with_capacity(total);
        for (i, host) in hosts.iter().enumerate() {
            // 分段标识带序号：长清单跑到一半时要能看出进行到哪台主机了
            if mode == OutputMode::Table {
                println!();
                println!("  [{}/{}] {}", i + 1, total, host.bold());
            }
            let output = normalize(host, probe_host(host, ports, concurrency).await);
            if mode == OutputMode::Table {
                match &output.error {
                    Some(msg) => println!("  {}", msg.red()),
                    None => render(&output, concurrency, mode),
                }
            }
            results.push(output);
        }
        (results, false)
    };

    let with_open = results.iter().filter(|o| o.open_count > 0).count();
    let scanned = results.len();
    let output = ScanBatchOutput {
        mode: "batch".to_string(),
        started_at,
        finished_at: crate::timestamp::now_rfc3339_millis(),
        stats: ScanBatchStats {
            hosts: scanned,
            with_open,
            without_open: scanned - with_open,
        },
        interrupted,
        results,
    };

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    println!();
    println!(
        "  {}",
        t1("scan.batch_summary", &scanned.to_string())
            .replace("{1}", &with_open.to_string())
            .bold()
    );
}

/// 并发模式下每台主机完成时的一行结果。
fn print_progress_line(seq: usize, total: usize, output: &ScanOutput) {
    let width = total.to_string().len();
    match &output.error {
        Some(msg) => println!(
            "  [{:>width$}/{}] {:<28} {}",
            seq,
            total,
            output.host,
            msg.red(),
            width = width
        ),
        None => println!(
            "  [{:>width$}/{}] {:<28} {}",
            seq,
            total,
            output.host,
            t2(
                "scan.done",
                &output.open_count.to_string(),
                &output.total_scanned.to_string()
            ),
            width = width
        ),
    }
}

/// 把探测结果收敛成一条必定存在的结果：解析失败的主机也要占位，
/// 否则结果数与清单行数对不上。
fn normalize(host: &str, result: Result<ScanOutput, String>) -> ScanOutput {
    match result {
        Ok(output) => output,
        Err(msg) => {
            crate::output::mark_failure();
            failed_output(host, msg)
        }
    }
}

/// 解析失败的主机的占位结果。
fn failed_output(host: &str, error: String) -> ScanOutput {
    ScanOutput {
        host: host.to_string(),
        target: String::new(),
        started_at: crate::timestamp::now_rfc3339_millis(),
        finished_at: crate::timestamp::now_rfc3339_millis(),
        total_scanned: 0,
        open_count: 0,
        results: Vec::new(),
        error: Some(error),
    }
}

fn report_host_error(msg: &str, mode: OutputMode) {
    crate::output::mark_failure();
    if mode == OutputMode::Json {
        print_json_error(msg);
    } else {
        println!("  {}", msg.red());
    }
}

/// 扫描单台主机：只探测，不渲染。解析失败时返回 `Err(消息)`。
async fn probe_host(
    host: &str,
    ports: Option<&[u16]>,
    concurrency: usize,
) -> Result<ScanOutput, String> {
    let concurrency = concurrency.max(1);
    // 扫描窗口从解析之前开始算：多 A 记录域名的解析本身可能是耗时的一段，
    // 把它排除在外会让 started_at 与实际命令起点脱节。
    let started_at = crate::timestamp::now_rfc3339_millis();

    // 解析主机；多 A 记录域名对每个端口尝试多个候选 IP，避免单个后端异常导致误判。
    let targets = crate::util::resolve_host_all(host).await;
    if targets.is_empty() {
        return Err(t1("scan.resolve_fail", host));
    }
    let target_label = targets
        .iter()
        .map(IpAddr::to_string)
        .collect::<Vec<_>>()
        .join(", ");

    let port_list: Vec<u16> = match ports {
        Some(p) => p.to_vec(),
        None => COMMON_PORTS.iter().map(|(p, _)| *p).collect(),
    };

    let semaphore = std::sync::Arc::new(Semaphore::new(concurrency));
    let mut handles = Vec::new();

    for port in &port_list {
        let permit = semaphore.clone();
        let port = *port;
        let targets = targets.clone();
        handles.push(tokio::spawn(async move {
            let _permit = permit.acquire_owned().await.unwrap();
            scan_port(&targets, port).await
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        if let Ok(result) = handle.await {
            results.push(result);
        }
    }

    results.sort_by_key(|r| r.port);

    let open_count = results.iter().filter(|r| r.open).count();
    Ok(ScanOutput {
        host: host.to_string(),
        target: target_label,
        started_at,
        finished_at: crate::timestamp::now_rfc3339_millis(),
        total_scanned: results.len(),
        open_count,
        results,
        error: None,
    })
}

/// 渲染扫描结果。
fn render(output: &ScanOutput, concurrency: usize, mode: OutputMode) {
    if mode == OutputMode::Json {
        print_json(output);
        return;
    }

    // 表格输出
    println!();
    println!("{}", t1("scan.title", &output.host).bold());
    println!("  {}", t2("scan.target", &output.host, &output.target));
    println!(
        "  {}",
        t2(
            "scan.info",
            &output.total_scanned.to_string(),
            &concurrency.to_string()
        )
    );
    println!();

    let open: Vec<&PortResult> = output.results.iter().filter(|r| r.open).collect();

    if open.is_empty() {
        println!("  {}", t("scan.no_open").yellow());
    } else {
        let h_port = t("scan.port");
        let h_ip = t("trace.ip");
        let h_state = t("scan.state");
        let h_svc = t("scan.service");
        let headers = [
            h_port.as_str(),
            h_ip.as_str(),
            h_state.as_str(),
            h_svc.as_str(),
        ];
        let rows: Vec<Vec<String>> = open
            .iter()
            .map(|r| {
                vec![
                    r.port.to_string(),
                    r.ip.clone().unwrap_or_else(|| "--".to_string()),
                    "open".green().to_string(),
                    r.service.to_string(),
                ]
            })
            .collect();
        print_table(&headers, &rows);
    }

    println!();
    println!(
        "  {}",
        t2(
            "scan.done",
            &output.open_count.to_string(),
            &output.total_scanned.to_string()
        )
    );
}

/// 扫描单个端口
async fn scan_port(targets: &[IpAddr], port: u16) -> PortResult {
    let service = COMMON_PORTS
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, s)| *s)
        .unwrap_or("unknown");

    // 时间戳取自第一次尝试之前：多候选 IP 会串行重试，取结束时刻就无法反映
    // 该端口实际是什么时候开始探的。
    let ts = crate::timestamp::now_rfc3339_millis();

    for target in targets.iter().copied().take(8) {
        let addr = SocketAddr::new(target, port);
        let result = timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await;
        if result.map(|r| r.is_ok()).unwrap_or(false) {
            return PortResult {
                port,
                ts,
                open: true,
                ip: Some(target.to_string()),
                service: service.to_string(),
            };
        }
    }

    PortResult {
        port,
        ts,
        open: false,
        ip: None,
        service: service.to_string(),
    }
}
