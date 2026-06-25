//! 端口扫描模块：并发 TCP connect 扫描。

use std::net::IpAddr;
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
const MAX_CONCURRENT: usize = 100;

const COMMON_PORTS: &[(u16, &str)] = &[
    (21, "FTP"), (22, "SSH"), (23, "Telnet"), (25, "SMTP"),
    (53, "DNS"), (80, "HTTP"), (110, "POP3"), (143, "IMAP"),
    (443, "HTTPS"), (445, "SMB"), (993, "IMAPS"), (995, "POP3S"),
    (1433, "SQL Server"), (3306, "MySQL"), (3389, "RDP"),
    (5432, "PostgreSQL"), (6379, "Redis"), (8080, "HTTP Alt"),
    (8443, "HTTPS Alt"), (9090, "Prometheus"),
];

/// 单个端口扫描结果
#[derive(Serialize, Clone)]
pub struct PortResult {
    pub port: u16,
    pub open: bool,
    pub service: String,
}

/// 端口扫描完整输出
#[derive(Serialize)]
pub struct ScanOutput {
    pub host: String,
    pub target: String,
    pub total_scanned: usize,
    pub open_count: usize,
    pub results: Vec<PortResult>,
}

/// 执行端口扫描并输出结果
pub async fn run(host: &str, ports: Option<&[u16]>, mode: OutputMode) {
    // 解析主机
    let target = match crate::util::resolve_host(host).await {
        Some(ip) => ip,
        None => {
            let msg = t1("scan.resolve_fail", host);
            if mode == OutputMode::Json {
                print_json_error(&msg);
            } else {
                println!("  {}", msg.red());
            }
            return;
        }
    };

    let port_list: Vec<u16> = match ports {
        Some(p) => p.to_vec(),
        None => COMMON_PORTS.iter().map(|(p, _)| *p).collect(),
    };

    let semaphore = std::sync::Arc::new(Semaphore::new(MAX_CONCURRENT));
    let mut handles = Vec::new();

    for port in &port_list {
        let permit = semaphore.clone();
        let port = *port;
        handles.push(tokio::spawn(async move {
            let _permit = permit.acquire_owned().await.unwrap();
            scan_port(target, port).await
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
        target: target.to_string(),
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
    println!("  {}", t2("scan.target", host, &target.to_string()));
    println!(
        "  {}",
        t2("scan.info", &port_list.len().to_string(), &MAX_CONCURRENT.to_string())
    );
    println!();

    let open: Vec<&PortResult> = results.iter().filter(|r| r.open).collect();

    if open.is_empty() {
        println!("  {}", t("scan.no_open").yellow());
    } else {
        let h_port = t("scan.port");
        let h_state = t("scan.state");
        let h_svc = t("scan.service");
        let headers = [h_port.as_str(), h_state.as_str(), h_svc.as_str()];
        let rows: Vec<Vec<String>> = open
            .iter()
            .map(|r| {
                vec![
                    r.port.to_string(),
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
        t2("scan.done", &open_count.to_string(), &results.len().to_string())
    );
}

/// 扫描单个端口
async fn scan_port(target: IpAddr, port: u16) -> PortResult {
    let addr = format!("{}:{}", target, port);
    let result = timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr)).await;

    let open = result.map(|r| r.is_ok()).unwrap_or(false);
    let service = COMMON_PORTS
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, s)| *s)
        .unwrap_or("unknown");

    PortResult {
        port,
        open,
        service: service.to_string(),
    }
}
