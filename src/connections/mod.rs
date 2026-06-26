//! 网络连接列表模块：显示当前 TCP/UDP 连接。

use serde::Serialize;

use crate::i18n::t;
use crate::output::{print_json, OutputMode};
use crate::table::print_table;

/// 连接信息
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub protocol: String,
    pub local_addr: String,
    pub remote_addr: String,
    pub state: String,
    pub pid: u32,
    pub process_name: String,
}

/// 连接列表完整输出
#[derive(Serialize)]
pub struct ConnectionsOutput {
    pub connections: Vec<ConnectionInfo>,
    pub total: usize,
    pub tcp_count: usize,
    pub udp_count: usize,
}

/// 过滤条件
pub struct ConnFilter {
    pub state: Option<String>,
    pub port: Option<u16>,
    pub process: Option<String>,
}

// 平台分发
#[cfg(target_os = "windows")]
mod conn_win;
#[cfg(target_os = "windows")]
use conn_win::get_connections;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod conn_unix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use conn_unix::get_connections;

/// 执行连接列表命令
pub fn run(filter: ConnFilter, mode: OutputMode) {
    let mut connections = get_connections();

    // 应用过滤
    if let Some(ref state) = filter.state {
        let state_upper = state.to_uppercase();
        connections.retain(|c| c.state.to_uppercase() == state_upper);
    }
    if let Some(port) = filter.port {
        connections.retain(|c| {
            c.local_addr.ends_with(&format!(":{}", port))
                || c.remote_addr.ends_with(&format!(":{}", port))
        });
    }
    if let Some(ref process) = filter.process {
        let process_lower = process.to_lowercase();
        connections.retain(|c| c.process_name.to_lowercase().contains(&process_lower));
    }

    let tcp_count = connections.iter().filter(|c| c.protocol == "TCP").count();
    let udp_count = connections.iter().filter(|c| c.protocol == "UDP").count();
    let total = connections.len();

    let output = ConnectionsOutput {
        connections: connections.clone(),
        total,
        tcp_count,
        udp_count,
    };

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    // 表格输出
    println!();
    println!("{}", t("connections.title").bold());

    if connections.is_empty() {
        println!("  {}", t("connections.no_result").yellow());
    } else {
        let h_proto = t("connections.proto");
        let h_local = t("connections.local");
        let h_remote = t("connections.remote");
        let h_state = t("connections.state");
        let h_pid = t("connections.pid");
        let h_process = t("connections.process");

        let headers = [
            h_proto.as_str(),
            h_local.as_str(),
            h_remote.as_str(),
            h_state.as_str(),
            h_pid.as_str(),
            h_process.as_str(),
        ];

        let rows: Vec<Vec<String>> = connections
            .iter()
            .map(|c| {
                vec![
                    c.protocol.clone(),
                    c.local_addr.clone(),
                    c.remote_addr.clone(),
                    c.state.clone(),
                    c.pid.to_string(),
                    c.process_name.clone(),
                ]
            })
            .collect();

        print_table(&headers, &rows);
    }

    println!();
    println!(
        "  {}",
        t("connections.summary")
            .replace("{0}", &total.to_string())
            .replace("{1}", &tcp_count.to_string())
            .replace("{2}", &udp_count.to_string())
    );

    // 权限提示
    println!();
    println!("  {}", t("connections.no_admin").dimmed());
}

use colored::Colorize;
