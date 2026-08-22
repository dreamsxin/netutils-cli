//! 网络连接列表模块：显示当前 TCP/UDP 连接。

#![allow(clippy::items_after_test_module)]

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
    pub is_proxy: bool,
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
    pub proto: Option<String>,
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

// ---------------------------------------------------------------------------
// 以下解析函数与平台无关，始终编译，便于在任意平台编写单元测试。
// （Linux/macOS 的外部命令调用仍在 conn_unix.rs 中，受 cfg 保护。）
// ---------------------------------------------------------------------------

/// 解析 `ss -tunp` 输出。
///
/// `ss -tunp` 表头与典型行（注意第一列是 Netid，而非 State）：
/// ```text
/// Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process
/// tcp   ESTAB  0      0      192.168.1.5:43210 142.250.69.174:443 users:(("chrome",pid=1234,fd=15))
/// udp   UNCONN 0      0      0.0.0.0:53         *:*               users:(("dnsmasq",pid=567,fd=4))
/// ```
/// 老实现误把 `parts[0]`(Netid) 当成 state、`parts[3]`(Send-Q) 当成本地地址，
/// 导致协议/地址/状态全部错位，且 UDP 永远被判成 TCP。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_ss_output(text: &str) -> Vec<ConnectionInfo> {
    let mut connections = Vec::new();

    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }

        // ss 输出: Netid State Recv-Q Send-Q Local Addr:Port Peer Addr:Port [Process...]
        let netid = parts[0].to_lowercase();
        let protocol = if netid == "udp" { "UDP" } else { "TCP" };
        let state = parts[1].to_string();
        let local = parts[4].to_string();
        let remote = parts[5].to_string();

        // 解析进程信息: users:(("name",pid=1234,fd=3))
        let mut pid = 0u32;
        let mut process_name = String::from("N/A");
        if parts.len() > 6 {
            let proc_part = parts[6..].join(" ");
            if let Some(start) = proc_part.find("((\"") {
                let after = &proc_part[start + 3..];
                if let Some(end) = after.find('"') {
                    process_name = after[..end].to_string();
                }
            }
            if let Some(start) = proc_part.find("pid=") {
                let after = &proc_part[start + 4..];
                let num: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(p) = num.parse::<u32>() {
                    pid = p;
                }
            }
        }

        // UDP / 无对端的 remote 地址统一为 *:*
        let remote_addr = if remote == "*" || remote == "*:*" {
            "*:*".to_string()
        } else {
            remote
        };

        // UDP 无连接状态，统一为 *
        let state = if protocol == "UDP" && state.eq_ignore_ascii_case("UNCONN") {
            "*".to_string()
        } else {
            state
        };

        connections.push(ConnectionInfo {
            protocol: protocol.to_string(),
            local_addr: local,
            remote_addr,
            state,
            pid,
            process_name,
            is_proxy: false,
        });
    }

    connections
}

/// 解析 `netstat -tunp` 输出（作为 `ss` 不可用时的回退）。
///
/// ```text
/// Active Internet connections (servers and established)
/// Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name
/// tcp        0      0 192.168.1.5:43210       142.250.69.174:443      ESTABLISHED 1234/chrome
/// udp        0      0 0.0.0.0:53              0.0.0.0:*                           -
/// ```
/// 列序: Proto Recv-Q Send-Q Local Foreign State PID/Program
/// UDP 行无 State 列（该位为空），需特殊处理。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_netstat_output(text: &str) -> Vec<ConnectionInfo> {
    let mut connections = Vec::new();

    for line in text.lines().skip(2) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }

        // Proto Recv-Q Send-Q Local Foreign [State] [PID/Program]
        let proto = parts[0].to_lowercase();
        let protocol = if proto == "udp" { "UDP" } else { "TCP" };
        let local = parts[3].to_string();
        let remote = parts[4].to_string();

        // TCP 行: parts[5]=State, parts[6]=PID/Program
        // UDP 行: parts[5]=PID/Program（无 State），或 parts[5] 为空
        let (state, pid_prog) = if protocol == "UDP" {
            // UDP 无连接状态；PID/Program 在 parts[5]（可能为 "-"）
            ("*".to_string(), parts.get(5).copied().unwrap_or("-"))
        } else {
            (
                parts.get(5).copied().unwrap_or("*").to_string(),
                parts.get(6).copied().unwrap_or("-"),
            )
        };

        let (pid, process_name) = parse_pid_program(pid_prog);
        let remote_addr = if remote == "*" || remote == "*:*" || remote == "0.0.0.0:*" {
            "*:*".to_string()
        } else {
            remote
        };

        connections.push(ConnectionInfo {
            protocol: protocol.to_string(),
            local_addr: local,
            remote_addr,
            state,
            pid,
            process_name,
            is_proxy: false,
        });
    }

    connections
}

/// 解析 netstat 的 "PID/Program name" 字段（如 `1234/chrome`、`-`、`1234`）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_pid_program(s: &str) -> (u32, String) {
    if s == "-" || s.is_empty() {
        return (0, String::from("N/A"));
    }
    if let Some(idx) = s.find('/') {
        let pid = s[..idx].parse::<u32>().unwrap_or(0);
        let name = s[idx + 1..].to_string();
        (pid, name)
    } else {
        (s.parse::<u32>().unwrap_or(0), String::from("N/A"))
    }
}
///
/// `lsof` 固定前缀列：`COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME`，
/// 其中 NAME 字段含空格（如 `TCP 1.2.3.4:443->5.6.7.8:80 (ESTABLISHED)`）。
/// 老实现按 whitespace 切分后只取 `parts[len-1]`，结果只拿到 `(ESTABLISHED)`，
/// 丢失了本地/远端地址与协议。
///
/// 正确做法：前 8 列按位置取，剩余部分合并为 NAME。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_lsof_output(text: &str) -> Vec<ConnectionInfo> {
    let mut connections = Vec::new();

    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // 前 8 列固定，第 9 列起为 NAME（含空格，整体保留）
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }

        let process_name = parts[0].to_string();
        let pid: u32 = parts[1].parse().unwrap_or(0);
        // NAME = parts[8..] 合并
        let name = parts[8..].join(" ");

        // NAME 形如 "TCP local->remote (STATE)" / "UDP local" / "TCP *:443 (LISTEN)"
        let (protocol, local_addr, remote_addr, state) = match parse_lsof_name(&name) {
            Some(v) => v,
            None => continue,
        };

        connections.push(ConnectionInfo {
            protocol,
            local_addr,
            remote_addr,
            state,
            pid,
            process_name,
            is_proxy: false,
        });
    }

    connections
}

/// 解析 lsof NAME 列，返回 (protocol, local, remote, state)。
///
/// 格式举例：
/// - `TCP 192.168.1.5:43210->142.250.69.174:443 (ESTABLISHED)`
/// - `TCP *:22 (LISTEN)`
/// - `UDP 0.0.0.0:53`
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_lsof_name(name: &str) -> Option<(String, String, String, String)> {
    let mut rest = name.trim();

    // 协议前缀
    let (protocol, stripped) = match rest.strip_prefix("TCP") {
        Some(stripped) => ("TCP", stripped),
        None => ("UDP", rest.strip_prefix("UDP")?),
    };
    rest = stripped.trim_start();

    // 拆出末尾的 (STATE)
    let (addr_part, state) = if let Some(idx) = rest.rfind(" (") {
        let s = rest[idx + 2..].trim_end_matches(')').to_string();
        (rest[..idx].trim().to_string(), s)
    } else {
        (rest.trim().to_string(), "*".to_string())
    };

    // local->remote
    let (local_addr, remote_addr) = if let Some(idx) = addr_part.find("->") {
        (
            addr_part[..idx].trim().to_string(),
            addr_part[idx + 2..].trim().to_string(),
        )
    } else {
        (addr_part, "*:*".to_string())
    };

    Some((protocol.to_string(), local_addr, remote_addr, state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ss_parses_tcp_established_with_process() {
        let input = "\
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process
tcp   ESTAB  0      0      192.168.1.5:43210 142.250.69.174:443 users:((\"chrome\",pid=1234,fd=15))
";
        let conns = parse_ss_output(input);
        assert_eq!(conns.len(), 1);
        let c = &conns[0];
        assert_eq!(c.protocol, "TCP");
        assert_eq!(c.local_addr, "192.168.1.5:43210");
        assert_eq!(c.remote_addr, "142.250.69.174:443");
        assert_eq!(c.state, "ESTAB");
        assert_eq!(c.pid, 1234);
        assert_eq!(c.process_name, "chrome");
    }

    #[test]
    fn ss_parses_udp_unconn() {
        // 关键：UDP 行的 Netid 是 udp，不能被误判成 TCP
        let input = "\
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process
udp   UNCONN 0      0      0.0.0.0:53         *:*               users:((\"dnsmasq\",pid=567,fd=4))
";
        let conns = parse_ss_output(input);
        assert_eq!(conns.len(), 1);
        let c = &conns[0];
        assert_eq!(c.protocol, "UDP");
        assert_eq!(c.local_addr, "0.0.0.0:53");
        assert_eq!(c.remote_addr, "*:*");
        assert_eq!(c.state, "*");
        assert_eq!(c.pid, 567);
        assert_eq!(c.process_name, "dnsmasq");
    }

    #[test]
    fn ss_skips_header_and_blank_lines() {
        let input = "\
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process

tcp   LISTEN 0      0      0.0.0.0:22         *:*               users:((\"sshd\",pid=890,fd=3))
";
        let conns = parse_ss_output(input);
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].protocol, "TCP");
        assert_eq!(conns[0].state, "LISTEN");
        assert_eq!(conns[0].process_name, "sshd");
    }

    #[test]
    fn netstat_parses_tcp_established() {
        // Proto Recv-Q Send-Q Local Foreign State PID/Program
        let input = "\
Active Internet connections (servers and established)
Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name
tcp        0      0 192.168.1.5:43210       142.250.69.174:443      ESTABLISHED 1234/chrome
";
        let conns = parse_netstat_output(input);
        assert_eq!(conns.len(), 1);
        let c = &conns[0];
        assert_eq!(c.protocol, "TCP");
        assert_eq!(c.local_addr, "192.168.1.5:43210");
        assert_eq!(c.remote_addr, "142.250.69.174:443");
        assert_eq!(c.state, "ESTABLISHED");
        assert_eq!(c.pid, 1234);
        assert_eq!(c.process_name, "chrome");
    }

    #[test]
    fn netstat_parses_udp_no_state() {
        // UDP 行无 State 列，PID/Program 紧跟 Foreign 之后（可能为 "-"）
        let input = "\
Active Internet connections (servers and established)
Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name
udp        0      0 0.0.0.0:53              0.0.0.0:*                           -
udp        0      0 0.0.0.0:68              0.0.0.0:*                           567/dhclient
";
        let conns = parse_netstat_output(input);
        assert_eq!(conns.len(), 2);
        let c0 = &conns[0];
        assert_eq!(c0.protocol, "UDP");
        assert_eq!(c0.local_addr, "0.0.0.0:53");
        assert_eq!(c0.remote_addr, "*:*");
        assert_eq!(c0.state, "*");
        assert_eq!(c0.pid, 0);
        assert_eq!(c0.process_name, "N/A");
        let c1 = &conns[1];
        assert_eq!(c1.pid, 567);
        assert_eq!(c1.process_name, "dhclient");
    }

    #[test]
    fn netstat_parses_tcp_listen_wildcard() {
        let input = "\
Active Internet connections (servers and established)
Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name
tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      890/sshd
";
        let conns = parse_netstat_output(input);
        assert_eq!(conns.len(), 1);
        let c = &conns[0];
        assert_eq!(c.protocol, "TCP");
        assert_eq!(c.local_addr, "0.0.0.0:22");
        assert_eq!(c.remote_addr, "*:*");
        assert_eq!(c.state, "LISTEN");
        assert_eq!(c.pid, 890);
        assert_eq!(c.process_name, "sshd");
    }

    #[test]
    fn lsof_parses_tcp_established() {
        // NAME 含空格：老实现会切散。
        // 列序: COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
        //       NAME = "TCP local->remote (STATE)"
        let input = "\
COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME
chrome   1234 root   15u  IPv4 0x12345      0t0  67890 TCP 192.168.1.5:43210->142.250.69.174:443 (ESTABLISHED)
";
        let conns = parse_lsof_output(input);
        assert_eq!(conns.len(), 1);
        let c = &conns[0];
        assert_eq!(c.protocol, "TCP");
        assert_eq!(c.local_addr, "192.168.1.5:43210");
        assert_eq!(c.remote_addr, "142.250.69.174:443");
        assert_eq!(c.state, "ESTABLISHED");
        assert_eq!(c.pid, 1234);
        assert_eq!(c.process_name, "chrome");
    }

    #[test]
    fn lsof_parses_tcp_listen_wildcard() {
        let input = "\
COMMAND PID USER FD   TYPE DEVICE SIZE/OFF NODE NAME
sshd    890 root 3u  IPv4 0x10000      0t0  50000 TCP *:22 (LISTEN)
";
        let conns = parse_lsof_output(input);
        assert_eq!(conns.len(), 1);
        let c = &conns[0];
        assert_eq!(c.protocol, "TCP");
        assert_eq!(c.local_addr, "*:22");
        assert_eq!(c.remote_addr, "*:*");
        assert_eq!(c.state, "LISTEN");
    }

    #[test]
    fn lsof_parses_udp_no_state() {
        let input = "\
COMMAND  PID USER FD   TYPE DEVICE SIZE/OFF NODE NAME
dnsmasq  567 root 4u  IPv4 0x20000      0t0  60000 UDP 0.0.0.0:53
";
        let conns = parse_lsof_output(input);
        assert_eq!(conns.len(), 1);
        let c = &conns[0];
        assert_eq!(c.protocol, "UDP");
        assert_eq!(c.local_addr, "0.0.0.0:53");
        assert_eq!(c.remote_addr, "*:*");
        assert_eq!(c.state, "*");
    }

    #[test]
    fn lsof_ignores_non_ip_lines() {
        // TYPE 非 IPv4/IPv6 的行（如 Unix socket）应被忽略：
        // 其 NAME 不以 TCP/UDP 开头，parse_lsof_name 返回 None
        let input = "\
COMMAND PID USER FD   TYPE  DEVICE SIZE/OFF NODE NAME
systemd 1   root 9u  unix  0x123         0t0  30000 /run/systemd/socket type=STREAM
sshd    890 root 3u  IPv4  0x10000       0t0  50000 TCP *:22 (LISTEN)
";
        let conns = parse_lsof_output(input);
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].protocol, "TCP");
    }
}

/// 从系统代理地址中提取端口号
fn detect_proxy_port() -> Option<u16> {
    let proxy_addr = crate::util::get_system_proxy_addr()?;
    // 从 "http://127.0.0.1:7897" 提取端口
    proxy_addr.rsplit(':').next()?.parse().ok()
}

/// 执行连接列表命令
pub fn run(filter: ConnFilter, mode: OutputMode) {
    let mut connections = get_connections();

    // 检测代理相关连接
    let proxy_port = detect_proxy_port();
    let proxy_keywords = [
        "mihomo",
        "clash",
        "v2ray",
        "xray",
        "sing-box",
        "trojan",
        "ssr",
        "shadowsocks",
    ];
    for c in &mut connections {
        // 进程名匹配代理工具
        let proc_lower = c.process_name.to_lowercase();
        if proxy_keywords.iter().any(|kw| proc_lower.contains(kw)) {
            c.is_proxy = true;
            continue;
        }
        // 本地端口匹配代理端口
        if let Some(port) = proxy_port {
            if c.local_addr.ends_with(&format!(":{}", port)) {
                c.is_proxy = true;
            }
        }
    }

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
    if let Some(ref proto) = filter.proto {
        let proto_upper = proto.to_uppercase();
        connections.retain(|c| c.protocol == proto_upper);
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
        let h_proxy = t("connections.proxy_col");

        let headers = [
            h_proto.as_str(),
            h_local.as_str(),
            h_remote.as_str(),
            h_state.as_str(),
            h_pid.as_str(),
            h_process.as_str(),
            h_proxy.as_str(),
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
                    if c.is_proxy {
                        "代理".to_string()
                    } else {
                        "".to_string()
                    },
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
