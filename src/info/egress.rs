//! 流量出口检测模块。

use std::net::{IpAddr, UdpSocket};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Duration;

use super::interface::InterfaceInfo;

/// 探测候选目标（避免单一目标被墙导致检测失败）
const PROBE_TARGETS: &[&str] = &[
    "8.8.8.8:80",
    "1.1.1.1:80",
    "114.114.114.114:80",
    "223.5.5.5:80",
];
#[cfg(any(target_os = "macos", target_os = "linux"))]
const EGRESS_ROUTE_TIMEOUT: Duration = Duration::from_secs(3);

/// 通过 UDP 探测实际出口 IP（连接公网地址，不实际发送数据）
///
/// 依次尝试多个探测目标，第一个成功的即为出口 IP
pub fn detect_egress_ip() -> Option<IpAddr> {
    for target in PROBE_TARGETS {
        if let Some(ip) = probe_target(target) {
            return Some(ip);
        }
    }
    None
}

/// 通过系统路由查询实际目标出口接口。
///
/// 这比仅通过 UDP local_addr 匹配接口更适合 TUN 模式：
/// TUN/utun 可能没有可直接匹配的 IPv4 地址，但路由表会明确显示目标流量走哪个接口。
pub fn detect_egress_interface() -> Option<String> {
    for target in ["8.8.8.8", "1.1.1.1", "114.114.114.114", "223.5.5.5"] {
        if let Some(iface) = route_interface_for_target(target) {
            return Some(iface);
        }
    }
    None
}

/// 尝试连接单个探测目标
fn probe_target(target: &str) -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(target).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// 通过实际出口 IP 匹配对应的接口名
pub fn find_egress_interface(egress_ip: &IpAddr, interfaces: &[InterfaceInfo]) -> Option<String> {
    let target = egress_ip.to_string();
    interfaces
        .iter()
        .find(|i| i.ipv4 == target)
        .map(|i| i.name.clone())
}

fn route_interface_for_target(target: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = crate::util::command_output_timeout(
            "route",
            &["-n", "get", target],
            EGRESS_ROUTE_TIMEOUT,
        )?;
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines().map(str::trim) {
            if let Some(v) = line.strip_prefix("interface:") {
                let iface = v.trim();
                if !iface.is_empty() {
                    return Some(iface.to_string());
                }
            }
        }
        return None;
    }

    #[cfg(target_os = "linux")]
    {
        let output = crate::util::command_output_timeout(
            "ip",
            &["route", "get", target],
            EGRESS_ROUTE_TIMEOUT,
        )?;
        let text = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = text.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "dev" && i + 1 < parts.len() {
                return Some(parts[i + 1].to_string());
            }
        }
        return None;
    }

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            r#"
$route = Find-NetRoute -RemoteIPAddress "{}" -ErrorAction SilentlyContinue | Sort-Object RouteMetric, InterfaceMetric | Select-Object -First 1
if ($route) {{ $route.InterfaceAlias }}
"#,
            target
        );
        let output = crate::util::powershell_output(&script, std::time::Duration::from_secs(3))?;
        let iface = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if iface.is_empty() {
            return None;
        }
        return Some(iface);
    }

    #[allow(unreachable_code)]
    None
}
