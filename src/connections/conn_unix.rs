//! Linux/macOS 网络连接实现。
//!
//! 仅包含平台相关的外部命令调用；解析逻辑（`parse_ss_output` /
//! `parse_lsof_output`）位于 [`super`]，无条件编译，便于在任意平台编写单元测试。

use std::time::Duration;

#[cfg(target_os = "macos")]
use super::parse_lsof_output;
use super::ConnectionInfo;
#[cfg(target_os = "linux")]
use super::{parse_netstat_output, parse_ss_output};

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

/// Linux: 优先解析 `ss -tunp`，不可用或无输出时回退到 `netstat -tunp`。
#[cfg(target_os = "linux")]
fn get_connections_linux() -> Vec<ConnectionInfo> {
    // 优先 ss（更现代，输出更易解析）
    if let Some(output) =
        crate::util::command_output_timeout("ss", &["-tunp"], CONNECTION_COMMAND_TIMEOUT)
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let conns = parse_ss_output(&text);
        if !conns.is_empty() {
            return conns;
        }
    }

    // 回退 netstat（ss 未安装或无连接时）
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
fn get_connections_macos() -> Vec<ConnectionInfo> {
    let output = match crate::util::command_output_timeout(
        "lsof",
        &["-i", "TCP", "-i", "UDP", "-P", "-n"],
        CONNECTION_COMMAND_TIMEOUT,
    ) {
        Some(o) => o,
        None => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    parse_lsof_output(&text)
}
