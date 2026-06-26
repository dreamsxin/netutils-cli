//! Windows 网络连接实现（PowerShell Get-NetTCPConnection）。

use std::process::Command;

use super::ConnectionInfo;

/// 获取所有 TCP/UDP 连接
pub fn get_connections() -> Vec<ConnectionInfo> {
    let mut connections = Vec::new();

    let ps_script = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

# TCP 连接
Get-NetTCPConnection -ErrorAction SilentlyContinue | ForEach-Object {
    $proc = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
    $procName = if ($proc) { $proc.ProcessName } else { 'N/A' }
    "$($_.LocalAddress)|$($_.LocalPort)|$($_.RemoteAddress)|$($_.RemotePort)|$($_.State)|$($_.OwningProcess)|$procName|TCP"
}

# UDP 端点
Get-NetUDPEndpoint -ErrorAction SilentlyContinue | ForEach-Object {
    $proc = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
    $procName = if ($proc) { $proc.ProcessName } else { 'N/A' }
    "$($_.LocalAddress)|$($_.LocalPort)|*|*|*|$($_.OwningProcess)|$procName|UDP"
}
"#;

    if let Ok(output) = Command::new("powershell").args(["-Command", ps_script]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.splitn(8, '|').collect();
            if parts.len() < 8 {
                continue;
            }

            let local_addr = format!("{}:{}", parts[0], parts[1]);
            let remote_addr = if parts[2] == "*" || parts[3] == "*" {
                "*:*".to_string()
            } else {
                format!("{}:{}", parts[2], parts[3])
            };
            let state = if parts[4] == "*" { "*".to_string() } else { parts[4].to_string() };
            let pid: u32 = parts[5].parse().unwrap_or(0);
            let process_name = parts[6].to_string();
            let protocol = parts[7].to_string();

            connections.push(ConnectionInfo {
                protocol,
                local_addr,
                remote_addr,
                state,
                pid,
                process_name,
            });
        }
    }

    connections
}
