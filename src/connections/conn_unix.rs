//! Linux/macOS 网络连接实现。

use std::process::Command;

use super::ConnectionInfo;

/// 获取所有 TCP/UDP 连接
pub fn get_connections() -> Vec<ConnectionInfo> {
    #[cfg(target_os = "linux")]
    {
        get_connections_linux()
    }
    #[cfg(target_os = "macos")]
    {
        get_connections_macos()
    }
}

/// Linux: 解析 `ss -tunp` 输出
#[cfg(target_os = "linux")]
fn get_connections_linux() -> Vec<ConnectionInfo> {
    let mut connections = Vec::new();

    let output = match Command::new("ss").args(["-tunp"]).output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);

    for line in text.lines().skip(1) {
        // 跳过空行和表头
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }

        // ss 输出: State Recv-Q Send-Q Local Address:Port Peer Address:Port Process
        let state = parts[0].to_string();
        let local = parts[3].to_string();
        let remote = parts[4].to_string();

        // 判断协议
        let protocol = if state.starts_with("UNCONN") {
            "UDP"
        } else {
            "TCP"
        };

        // 解析进程信息: users:(("name",pid=1234,fd=3))
        let mut pid = 0u32;
        let mut process_name = String::from("N/A");
        if parts.len() > 5 {
            let proc_part = parts[5..].join(" ");
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

        // UDP 的 remote 地址
        let remote_addr = if protocol == "UDP" && remote == "*:*" {
            "*:*".to_string()
        } else if remote == "*" {
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
        });
    }

    connections
}

/// macOS: 解析 `lsof -i TCP -i UDP -P -n` 输出
#[cfg(target_os = "macos")]
fn get_connections_macos() -> Vec<ConnectionInfo> {
    let mut connections = Vec::new();

    let output = match Command::new("lsof")
        .args(["-i", "TCP", "-i", "UDP", "-P", "-n"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);

    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }

        // lsof: COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
        let process_name = parts[0].to_string();
        let pid: u32 = parts[1].parse().unwrap_or(0);
        let protocol = if parts[4] == "IPv4" || parts[4] == "IPv6" {
            if parts.len() > 8 && parts[8].starts_with("TCP") {
                "TCP"
            } else {
                "UDP"
            }
        } else {
            continue;
        };

        // NAME 列: "local->remote (STATE)" 或 "local (STATE)" 或 "*:port"
        let name = parts[parts.len() - 1];
        let (local_addr, remote_addr, state) = parse_lsof_name(name, protocol);

        connections.push(ConnectionInfo {
            protocol: protocol.to_string(),
            local_addr,
            remote_addr,
            state,
            pid,
            process_name,
        });
    }

    connections
}

/// 解析 lsof NAME 列
#[cfg(target_os = "macos")]
fn parse_lsof_name(name: &str, protocol: &str) -> (String, String, String) {
    // 格式: "local->remote (STATE)" 或 "local (STATE)" 或 "*:port"
    let (addr_part, state) = if let Some(idx) = name.rfind(" (") {
        let state = name[idx + 2..].trim_end_matches(')').to_string();
        (&name[..idx], state)
    } else {
        (name, "*".to_string())
    };

    if let Some(idx) = addr_part.find("->") {
        let local = addr_part[..idx].to_string();
        let remote = addr_part[idx + 2..].to_string();
        (local, remote, state)
    } else {
        (addr_part.to_string(), "*:*".to_string(), state)
    }
}
