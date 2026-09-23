//! 端口扫描模块：并发 TCP connect 扫描。

use std::net::{IpAddr, SocketAddr};
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
}

/// 执行端口扫描并输出结果
pub async fn run(host: &str, ports: Option<&[u16]>, concurrency: usize, mode: OutputMode) {
    let concurrency = concurrency.max(1);
    // 扫描窗口从解析之前开始算：多 A 记录域名的解析本身可能是耗时的一段，
    // 把它排除在外会让 started_at 与实际命令起点脱节。
    let started_at = crate::timestamp::now_rfc3339_millis();

    // 解析主机；多 A 记录域名对每个端口尝试多个候选 IP，避免单个后端异常导致误判。
    let targets = crate::util::resolve_host_all(host).await;
    if targets.is_empty() {
        let msg = t1("scan.resolve_fail", host);
        crate::output::mark_failure();
        if mode == OutputMode::Json {
            print_json_error(&msg);
        } else {
            println!("  {}", msg.red());
        }
        return;
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
    let output = ScanOutput {
        host: host.to_string(),
        target: target_label.clone(),
        started_at,
        finished_at: crate::timestamp::now_rfc3339_millis(),
        total_scanned: results.len(),
        open_count,
        results: results.clone(),
    };

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    // 表格输出
    println!();
    println!("{}", t1("scan.title", host).bold());
    println!("  {}", t2("scan.target", host, &target_label));
    println!(
        "  {}",
        t2(
            "scan.info",
            &port_list.len().to_string(),
            &concurrency.to_string()
        )
    );
    println!();

    let open: Vec<&PortResult> = results.iter().filter(|r| r.open).collect();

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
            &open_count.to_string(),
            &results.len().to_string()
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
