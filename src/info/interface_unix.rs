//! Linux/macOS 网络接口实现。

use std::process::Command;

use super::interface::{classify_interface, InterfaceInfo};

/// 获取所有网络接口信息
pub fn get_all_interfaces() -> Vec<InterfaceInfo> {
    #[cfg(target_os = "linux")]
    {
        get_interfaces_linux()
    }
    #[cfg(target_os = "macos")]
    {
        get_interfaces_macos()
    }
}

/// Linux: 解析 `ip -j addr` 的 JSON 输出
#[cfg(target_os = "linux")]
fn get_interfaces_linux() -> Vec<InterfaceInfo> {
    // 优先尝试 `ip -j addr`（JSON 输出，可靠解析）
    if let Ok(output) = Command::new("ip").args(["-j", "addr"]).output() {
        if let Ok(text) = String::from_utf8(output.stdout) {
            if !text.is_empty() {
                return parse_ip_addr_json(&text);
            }
        }
    }

    // 回退: 解析 `ip addr` 文本输出
    if let Ok(output) = Command::new("ip").arg("addr").output() {
        if let Ok(text) = String::from_utf8(output.stdout) {
            return parse_ip_addr_text(&text);
        }
    }

    Vec::new()
}

/// 解析 `ip -j addr` JSON 输出
#[cfg(target_os = "linux")]
fn parse_ip_addr_json(text: &str) -> Vec<InterfaceInfo> {
    use serde_json::Value;

    let arr: Vec<Value> = match serde_json::from_str(text) {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };

    let mut interfaces = Vec::new();
    for iface in arr {
        let name = iface["ifname"].as_str().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }

        // 跳过 lo 回环（后面单独处理）
        let operstate = iface["operstate"].as_str().unwrap_or("UNKNOWN");
        let is_up = operstate == "UP";

        // 获取 IPv4
        let mut ipv4 = "--".to_string();
        if let Some(addr_info) = iface["addr_info"].as_array() {
            for addr in addr_info {
                if addr["family"].as_str() == Some("inet") {
                    if let Some(ip) = addr["local"].as_str() {
                        ipv4 = ip.to_string();
                        break;
                    }
                }
            }
        }

        let mac = iface["address"].as_str().unwrap_or("--").to_string();
        let desc = iface["alias"].as_str().unwrap_or(&name).to_string();
        let iftype = classify_interface(&desc, &name);

        // 跳过未命名/回环
        if name == "lo" {
            continue;
        }

        interfaces.push(InterfaceInfo {
            name: name.clone(),
            mac,
            ipv4,
            status: if is_up { "Up" } else { "Down" }.to_string(),
            description: desc,
            metric: 0, // Linux 接口跃点需单独查询
            iftype: iftype.to_id(),
            is_virtual: iftype.is_virtual(),
            is_egress: false,
            is_backup: false,
        });
    }

    interfaces
}

/// 解析 `ip addr` 文本输出（回退方案）
#[cfg(target_os = "linux")]
fn parse_ip_addr_text(text: &str) -> Vec<InterfaceInfo> {
    let mut interfaces = Vec::new();
    let mut current_name = String::new();
    let mut current_mac = String::from("--");
    let mut current_ipv4 = String::from("--");
    let mut current_status = "Down".to_string();
    let mut current_desc = String::new();
    let mut is_up = false;

    for line in text.lines() {
        let line = line.trim();
        // 接口行: "2: eth0: <BROADCAST,MULTICAST,UP,...>"
        if line.len() > 2 && line.chars().nth(1) == Some(':') {
            // 保存前一个接口
            if !current_name.is_empty() && current_name != "lo" {
                let iftype = classify_interface(&current_desc, &current_name);
                interfaces.push(InterfaceInfo {
                    name: current_name.clone(),
                    mac: current_mac.clone(),
                    ipv4: current_ipv4.clone(),
                    status: if is_up { "Up" } else { "Down" }.to_string(),
                    description: current_desc.clone(),
                    metric: 0,
                    iftype: iftype.to_id(),
                    is_virtual: iftype.is_virtual(),
                    is_egress: false,
                    is_backup: false,
                });
            }

            // 解析新接口
            let parts: Vec<&str> = line.splitn(3, ':').collect();
            current_name = parts.get(1).map(|s| s.trim()).unwrap_or("").to_string();
            current_mac = "--".to_string();
            current_ipv4 = "--".to_string();
            current_desc = current_name.clone();
            is_up = line.contains("UP");
        } else if line.starts_with("link/") {
            // link/ether aa:bb:cc:dd:ee:ff
            if let Some(mac) = line.split_whitespace().nth(1) {
                current_mac = mac.to_string();
            }
        } else if line.starts_with("inet ") {
            // inet 192.168.1.100/24
            if let Some(addr) = line.split_whitespace().nth(1) {
                if let Some(ip) = addr.split('/').next() {
                    current_ipv4 = ip.to_string();
                }
            }
        }
    }

    // 保存最后一个接口
    if !current_name.is_empty() && current_name != "lo" {
        let iftype = classify_interface(&current_desc, &current_name);
        interfaces.push(InterfaceInfo {
            name: current_name,
            mac: current_mac,
            ipv4: current_ipv4,
            status: if is_up { "Up" } else { "Down" }.to_string(),
            description: current_desc,
            metric: 0,
            iftype: iftype.to_id(),
            is_virtual: iftype.is_virtual(),
            is_egress: false,
            is_backup: false,
        });
    }

    interfaces
}

/// macOS: 解析 `ifconfig` 输出
#[cfg(target_os = "macos")]
fn get_interfaces_macos() -> Vec<InterfaceInfo> {
    let output = match Command::new("ifconfig").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);

    let mut interfaces = Vec::new();
    let mut current_name = String::new();
    let mut current_mac = String::from("--");
    let mut current_ipv4 = String::from("--");
    let mut current_status = "Down".to_string();
    let mut is_up = false;

    for line in text.lines() {
        let line = line.trim();
        // 接口行: "en0: flags=..."
        if !line.is_empty() && line.chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) && line.contains(':') {
            // 保存前一个
            if !current_name.is_empty() && current_name != "lo0" {
                let iftype = classify_interface(&current_name, &current_name);
                interfaces.push(InterfaceInfo {
                    name: current_name.clone(),
                    mac: current_mac.clone(),
                    ipv4: current_ipv4.clone(),
                    status: if is_up { "Up" } else { "Down" }.to_string(),
                    description: current_name.clone(),
                    metric: 0,
                    iftype: iftype.to_id(),
                    is_virtual: iftype.is_virtual(),
                    is_egress: false,
                    is_backup: false,
                });
            }
            let name = line.split(':').next().unwrap_or("").to_string();
            current_name = name;
            current_mac = "--".to_string();
            current_ipv4 = "--".to_string();
            is_up = line.contains("UP");
        } else if line.starts_with("ether ") {
            if let Some(mac) = line.split_whitespace().nth(1) {
                current_mac = mac.to_string();
            }
        } else if line.starts_with("inet ") {
            if let Some(addr) = line.split_whitespace().nth(1) {
                current_ipv4 = addr.to_string();
            }
        }
    }

    // 最后一个
    if !current_name.is_empty() && current_name != "lo0" {
        let iftype = classify_interface(&current_name, &current_name);
        interfaces.push(InterfaceInfo {
            name: current_name,
            mac: current_mac,
            ipv4: current_ipv4,
            status: if is_up { "Up" } else { "Down" }.to_string(),
            description: current_name,
            metric: 0,
            iftype: iftype.to_id(),
            is_virtual: iftype.is_virtual(),
            is_egress: false,
            is_backup: false,
        });
    }

    interfaces
}
