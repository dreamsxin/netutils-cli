//! Linux/macOS 网络连接实现。
//!
//! 仅包含平台相关的外部命令调用；解析逻辑（`parse_ss_output` /
//! `parse_lsof_output`）位于 [`super`]，无条件编译，便于在任意平台编写单元测试。

use std::time::Duration;

#[cfg(target_os = "macos")]
use super::parse_lsof_output;
#[cfg(target_os = "linux")]
use super::{parse_netstat_output, parse_ss_output};
use super::{ConnectionInfo, ConnectionReadError, ConnectionReadIssue, ConnectionSnapshot};

const CONNECTION_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

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
    #[cfg(all(test, not(any(target_os = "linux", target_os = "macos"))))]
    {
        Vec::new()
    }
}

pub fn collect_connections(port: u16) -> Result<ConnectionSnapshot, ConnectionReadError> {
    #[cfg(target_os = "linux")]
    {
        collect_connections_linux(port)
    }
    #[cfg(target_os = "macos")]
    {
        collect_connections_macos(port)
    }
    #[cfg(all(test, not(any(target_os = "linux", target_os = "macos"))))]
    {
        Ok(ConnectionSnapshot {
            connections: Vec::new(),
            issue: None,
        })
    }
}

/// Linux: 优先解析 `ss -tunp`，不可用或无输出时回退到 `netstat -tunp`。
#[cfg(target_os = "linux")]
fn collect_connections_linux(port: u16) -> Result<ConnectionSnapshot, ConnectionReadError> {
    let mut permission_error = false;
    let mut successful_empty = false;
    let filter = format!("sport = :{port}");

    if let Some(output) =
        crate::util::command_output_timeout("ss", &["-tunap", &filter], CONNECTION_COMMAND_TIMEOUT)
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let connections = filter_local_port(parse_ss_output(&text), port);
        if output.status.success() {
            if !connections.is_empty() {
                return Ok(ConnectionSnapshot {
                    connections,
                    issue: None,
                });
            }
            successful_empty = true;
        } else {
            permission_error |= has_permission_error(&output.stderr);
            if !connections.is_empty() {
                return Ok(ConnectionSnapshot {
                    connections,
                    issue: Some(ConnectionReadIssue {
                        code: if permission_error {
                            "permission_denied"
                        } else {
                            "collector_incomplete"
                        },
                    }),
                });
            }
        }
    }

    if let Some(output) =
        crate::util::command_output_timeout("netstat", &["-tunap"], CONNECTION_COMMAND_TIMEOUT)
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let connections = filter_local_port(parse_netstat_output(&text), port);
        if output.status.success() {
            return Ok(ConnectionSnapshot {
                connections,
                issue: permission_error.then_some(ConnectionReadIssue {
                    code: "permission_denied",
                }),
            });
        }
        permission_error |= has_permission_error(&output.stderr);
        if !connections.is_empty() {
            return Ok(ConnectionSnapshot {
                connections,
                issue: Some(ConnectionReadIssue {
                    code: if permission_error {
                        "permission_denied"
                    } else {
                        "collector_incomplete"
                    },
                }),
            });
        }
    }

    if successful_empty {
        return Ok(ConnectionSnapshot {
            connections: Vec::new(),
            issue: None,
        });
    }

    Err(ConnectionReadError {
        code: if permission_error {
            "permission_denied"
        } else {
            "collector_unavailable"
        },
    })
}

#[cfg(target_os = "linux")]
fn get_connections_linux() -> Vec<ConnectionInfo> {
    if let Some(output) =
        crate::util::command_output_timeout("ss", &["-tunp"], CONNECTION_COMMAND_TIMEOUT)
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let connections = parse_ss_output(&text);
        if !connections.is_empty() {
            return connections;
        }
    }
    if let Some(output) =
        crate::util::command_output_timeout("netstat", &["-tunp"], CONNECTION_COMMAND_TIMEOUT)
    {
        let text = String::from_utf8_lossy(&output.stdout);
        return parse_netstat_output(&text);
    }
    Vec::new()
}

/// macOS: 解析 `lsof -i TCP -i UDP -P -n` 输出
#[cfg(target_os = "macos")]
fn collect_connections_macos(port: u16) -> Result<ConnectionSnapshot, ConnectionReadError> {
    let port_filter = format!(":{port}");
    let output = match crate::util::command_output_timeout(
        "lsof",
        &["-i", &port_filter, "-P", "-n"],
        CONNECTION_COMMAND_TIMEOUT,
    ) {
        Some(o) => o,
        None => {
            return Err(ConnectionReadError {
                code: "collector_unavailable",
            })
        }
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let connections = filter_local_port(parse_lsof_output(&text), port);
    if output.status.success() {
        return Ok(ConnectionSnapshot {
            connections,
            issue: None,
        });
    }

    let permission_error = has_permission_error(&output.stderr);
    if !connections.is_empty() {
        return Ok(ConnectionSnapshot {
            connections,
            issue: Some(ConnectionReadIssue {
                code: if permission_error {
                    "permission_denied"
                } else {
                    "collector_incomplete"
                },
            }),
        });
    }

    // lsof 返回 1 且没有权限错误时表示没有匹配的网络端点。
    if output.status.code() == Some(1) && !permission_error {
        return Ok(ConnectionSnapshot {
            connections,
            issue: None,
        });
    }

    Err(ConnectionReadError {
        code: if permission_error {
            "permission_denied"
        } else {
            "collector_unavailable"
        },
    })
}

#[cfg(target_os = "macos")]
fn get_connections_macos() -> Vec<ConnectionInfo> {
    let output = match crate::util::command_output_timeout(
        "lsof",
        &["-i", "TCP", "-i", "UDP", "-P", "-n"],
        CONNECTION_COMMAND_TIMEOUT,
    ) {
        Some(output) => output,
        None => return Vec::new(),
    };
    parse_lsof_output(&String::from_utf8_lossy(&output.stdout))
}

fn has_permission_error(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    stderr.contains("permission denied")
        || stderr.contains("operation not permitted")
        || stderr.contains("access is denied")
}

fn filter_local_port(mut connections: Vec<ConnectionInfo>, port: u16) -> Vec<ConnectionInfo> {
    connections.retain(|connection| {
        connection
            .local_addr
            .rsplit_once(':')
            .and_then(|(_, value)| value.parse::<u16>().ok())
            == Some(port)
    });
    connections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_only_exact_local_ports() {
        let connections = vec![
            ConnectionInfo {
                protocol: "TCP".to_string(),
                local_addr: "127.0.0.1:80".to_string(),
                remote_addr: "*:*".to_string(),
                state: "LISTEN".to_string(),
                pid: 1,
                process_name: "web".to_string(),
                is_proxy: false,
            },
            ConnectionInfo {
                protocol: "TCP".to_string(),
                local_addr: "[::1]:8080".to_string(),
                remote_addr: "*:*".to_string(),
                state: "LISTEN".to_string(),
                pid: 2,
                process_name: "api".to_string(),
                is_proxy: false,
            },
        ];

        assert_eq!(filter_local_port(connections, 80).len(), 1);
    }
}
