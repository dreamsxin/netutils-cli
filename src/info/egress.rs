//! 流量出口检测模块。

use std::net::{IpAddr, UdpSocket};

use super::interface::InterfaceInfo;

/// 探测候选目标（避免单一目标被墙导致检测失败）
const PROBE_TARGETS: &[&str] = &[
    "8.8.8.8:80",
    "1.1.1.1:80",
    "114.114.114.114:80",
    "223.5.5.5:80",
];

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
