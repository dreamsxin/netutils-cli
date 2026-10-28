//! Windows 网络连接实现（PowerShell Get-NetTCPConnection）。

use std::time::Duration;

use super::{ConnectionInfo, ConnectionReadError, ConnectionReadIssue, ConnectionSnapshot};

const POWERSHELL_TIMEOUT: Duration = Duration::from_secs(5);
const PROCESS_QUERY_TIMEOUT: Duration = Duration::from_secs(15);

/// 获取所有 TCP/UDP 连接
pub fn get_connections() -> Vec<ConnectionInfo> {
    let ps_script = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-NetTCPConnection -ErrorAction SilentlyContinue | ForEach-Object {
    $proc = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
    $procName = if ($proc) { $proc.ProcessName } else { 'N/A' }
    "$($_.LocalAddress)|$($_.LocalPort)|$($_.RemoteAddress)|$($_.RemotePort)|$($_.State)|$($_.OwningProcess)|$procName|TCP"
}
Get-NetUDPEndpoint -ErrorAction SilentlyContinue | ForEach-Object {
    $proc = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
    $procName = if ($proc) { $proc.ProcessName } else { 'N/A' }
    "$($_.LocalAddress)|$($_.LocalPort)|*|*|*|$($_.OwningProcess)|$procName|UDP"
}
"#;
    crate::util::powershell_output(ps_script, POWERSHELL_TIMEOUT)
        .map(|output| parse_connections(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

pub fn collect_connections(port: u16) -> Result<ConnectionSnapshot, ConnectionReadError> {
    let mut collection_failed = false;

    let mut ps_script = format!("$port = {port}\n");
    ps_script.push_str(
        r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

# TCP 连接
try {
    Get-NetTCPConnection -LocalPort $port -ErrorAction Stop | ForEach-Object {
        $proc = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
        $procName = if ($proc) { $proc.ProcessName } else { 'N/A' }
        "$($_.LocalAddress)|$($_.LocalPort)|$($_.RemoteAddress)|$($_.RemotePort)|$($_.State)|$($_.OwningProcess)|$procName|TCP"
    }
} catch {
    if ($_.FullyQualifiedErrorId -notlike 'CmdletizationQuery_NotFound_LocalPort,*') {
        "ERROR|tcp|$($_.FullyQualifiedErrorId)"
    }
}

# UDP 端点
try {
    Get-NetUDPEndpoint -LocalPort $port -ErrorAction Stop | ForEach-Object {
        $proc = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
        $procName = if ($proc) { $proc.ProcessName } else { 'N/A' }
        "$($_.LocalAddress)|$($_.LocalPort)|*|*|*|$($_.OwningProcess)|$procName|UDP"
    }
} catch {
    if ($_.FullyQualifiedErrorId -notlike 'CmdletizationQuery_NotFound_LocalPort,*') {
        "ERROR|udp|$($_.FullyQualifiedErrorId)"
    }
}
Write-Output "DONE"
 "#,
    );

    let output = crate::util::powershell_output(&ps_script, PROCESS_QUERY_TIMEOUT).ok_or(
        ConnectionReadError {
            code: "collector_unavailable",
        },
    )?;
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    let text = String::from_utf8_lossy(&output.stdout);
    let permission_error = stderr.contains("access is denied")
        || stderr.contains("permission denied")
        || stderr.contains("unauthorizedaccess")
        || text.to_ascii_lowercase().contains("accessdenied")
        || text.to_ascii_lowercase().contains("unauthorizedaccess");
    for line in text.lines() {
        if line.trim().starts_with("ERROR|") {
            collection_failed = true;
        }
    }
    let mut connections = parse_connections(&text);
    connections.retain(|connection| local_port(&connection.local_addr) == Some(port));

    collection_failed |= !output.status.success();
    if collection_failed && connections.is_empty() {
        return Err(ConnectionReadError {
            code: if permission_error {
                "permission_denied"
            } else {
                "collector_unavailable"
            },
        });
    }

    Ok(ConnectionSnapshot {
        connections,
        issue: collection_failed.then_some(ConnectionReadIssue {
            code: if permission_error {
                "permission_denied"
            } else {
                "collector_incomplete"
            },
        }),
    })
}

fn parse_connections(text: &str) -> Vec<ConnectionInfo> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("ERROR|") {
                return None;
            }
            let parts: Vec<&str> = line.splitn(8, '|').collect();
            if parts.len() < 8 {
                return None;
            }
            let local_addr = format!("{}:{}", parts[0], parts[1]);
            let remote_addr = if parts[2] == "*" || parts[3] == "*" {
                "*:*".to_string()
            } else {
                format!("{}:{}", parts[2], parts[3])
            };
            let state = if parts[4] == "*" {
                "*".to_string()
            } else {
                parts[4].to_string()
            };
            Some(ConnectionInfo {
                protocol: parts[7].to_string(),
                local_addr,
                remote_addr,
                state,
                pid: parts[5].parse().unwrap_or(0),
                process_name: parts[6].to_string(),
                is_proxy: false,
            })
        })
        .collect()
}

fn local_port(address: &str) -> Option<u16> {
    address.rsplit_once(':')?.1.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tcp_and_udp_endpoint_records() {
        let parsed = parse_connections(
            "127.0.0.1|8080|0.0.0.0|0|Listen|42|server|TCP\n127.0.0.1|53|*|*|*|7|dns|UDP\nERROR|tcp|denied",
        );

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].local_addr, "127.0.0.1:8080");
        assert_eq!(parsed[0].pid, 42);
        assert_eq!(parsed[1].protocol, "UDP");
        assert_eq!(parsed[1].remote_addr, "*:*");
        assert_eq!(parsed[1].pid, 7);
    }

    #[test]
    fn local_port_parser_matches_ipv6_and_rejects_suffix_matches() {
        assert_eq!(local_port("[::1]:80"), Some(80));
        assert_eq!(local_port("0.0.0.0:8080"), Some(8080));
        assert_ne!(local_port("127.0.0.1:8080"), Some(80));
    }
}
