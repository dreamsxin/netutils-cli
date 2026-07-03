//! Linux/macOS 网络接口实现。

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Duration;

use super::interface::{classify_interface, InterfaceInfo};

#[cfg(any(target_os = "macos", test))]
use std::collections::HashMap;

#[cfg(any(target_os = "linux", target_os = "macos"))]
const INTERFACE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// 获取所有网络接口信息
#[cfg_attr(
    all(test, not(any(target_os = "linux", target_os = "macos"))),
    allow(dead_code)
)]
pub fn get_all_interfaces() -> Vec<InterfaceInfo> {
    #[cfg(target_os = "linux")]
    {
        get_interfaces_linux()
    }
    #[cfg(target_os = "macos")]
    {
        get_interfaces_macos()
    }
    #[cfg(all(test, not(any(target_os = "linux", target_os = "macos"))))]
    {
        Vec::new()
    }
}

/// Linux: 解析 `ip -j addr` 的 JSON 输出
#[cfg(target_os = "linux")]
fn get_interfaces_linux() -> Vec<InterfaceInfo> {
    // 优先尝试 `ip -j addr`（JSON 输出，可靠解析）
    if let Some(output) =
        crate::util::command_output_timeout("ip", &["-j", "addr"], INTERFACE_COMMAND_TIMEOUT)
    {
        if let Ok(text) = String::from_utf8(output.stdout) {
            if !text.is_empty() {
                return parse_ip_addr_json(&text);
            }
        }
    }

    // 回退: 解析 `ip addr` 文本输出
    if let Some(output) =
        crate::util::command_output_timeout("ip", &["addr"], INTERFACE_COMMAND_TIMEOUT)
    {
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
    let service_order = get_macos_service_order();
    let output =
        match crate::util::command_output_timeout("ifconfig", &[], INTERFACE_COMMAND_TIMEOUT) {
            Some(o) => o,
            None => return Vec::new(),
        };
    let text = String::from_utf8_lossy(&output.stdout);

    parse_macos_ifconfig(&text, &service_order)
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_ifconfig(text: &str, service_order: &HashMap<String, u32>) -> Vec<InterfaceInfo> {
    let mut interfaces = Vec::new();
    let mut current_name = String::new();
    let mut current_mac = String::from("--");
    let mut current_ipv4 = String::from("--");
    let mut is_up = false;

    for raw_line in text.lines() {
        // Only non-indented lines like `en0: flags=...` start a new interface.
        // Indented property lines such as `ether ...`, `media: ...`, or
        // `status: ...` must stay attached to the current interface.
        if raw_line == raw_line.trim_start() {
            if let Some((name, rest)) = raw_line.split_once(": flags=") {
                push_macos_interface(
                    &mut interfaces,
                    &current_name,
                    &current_mac,
                    &current_ipv4,
                    is_up,
                    service_order,
                );
                current_name = name.to_string();
                current_mac = "--".to_string();
                current_ipv4 = "--".to_string();
                is_up = rest.contains("<UP,") || rest.contains("<UP>");
                continue;
            }
        }

        let line = raw_line.trim();
        if line.starts_with("ether ") {
            if let Some(mac) = line.split_whitespace().nth(1) {
                current_mac = mac.to_string();
            }
        } else if line.starts_with("inet ") {
            if let Some(addr) = line.split_whitespace().nth(1) {
                current_ipv4 = addr.to_string();
            }
        }
    }

    push_macos_interface(
        &mut interfaces,
        &current_name,
        &current_mac,
        &current_ipv4,
        is_up,
        service_order,
    );

    interfaces
}

#[cfg(any(target_os = "macos", test))]
fn push_macos_interface(
    interfaces: &mut Vec<InterfaceInfo>,
    name: &str,
    mac: &str,
    ipv4: &str,
    is_up: bool,
    service_order: &HashMap<String, u32>,
) {
    if name.is_empty() || name == "lo0" {
        return;
    }

    let iftype = classify_interface(name, name);
    let metric = service_order.get(name).copied().unwrap_or(0);
    interfaces.push(InterfaceInfo {
        name: name.to_string(),
        mac: mac.to_string(),
        ipv4: ipv4.to_string(),
        status: if is_up { "Up" } else { "Down" }.to_string(),
        description: name.to_string(),
        metric,
        iftype: iftype.to_id(),
        is_virtual: iftype.is_virtual(),
        is_egress: false,
        is_backup: false,
    });
}

#[cfg(target_os = "macos")]
fn get_macos_service_order() -> HashMap<String, u32> {
    let output = match crate::util::command_output_timeout(
        "networksetup",
        &["-listnetworkserviceorder"],
        INTERFACE_COMMAND_TIMEOUT,
    ) {
        Some(o) => o,
        None => return HashMap::new(),
    };
    parse_macos_service_order(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_service_order(text: &str) -> HashMap<String, u32> {
    let mut order_by_device = HashMap::new();
    let mut current_order = None;

    for line in text.lines().map(str::trim) {
        if let Some(order) = parse_service_order_line(line) {
            current_order = Some(order);
            continue;
        }

        if let Some(order) = current_order {
            if let Some(device) = parse_service_device_line(line) {
                order_by_device.insert(device, order);
                current_order = None;
            }
        }
    }

    order_by_device
}

#[cfg(any(target_os = "macos", test))]
fn parse_service_order_line(line: &str) -> Option<u32> {
    let rest = line.strip_prefix('(')?;
    let (num, _) = rest.split_once(')')?;
    num.parse().ok()
}

#[cfg(any(target_os = "macos", test))]
fn parse_service_device_line(line: &str) -> Option<String> {
    let marker = "Device: ";
    let start = line.find(marker)? + marker.len();
    let rest = &line[start..];
    let end = rest.find(')').unwrap_or(rest.len());
    let device = rest[..end].trim();
    if device.is_empty() {
        None
    } else {
        Some(device.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_macos_network_service_order() {
        let text = r#"
An asterisk (*) denotes that a network service is disabled.
(1) Wi-Fi
(Hardware Port: Wi-Fi, Device: en0)
(2) USB 10/100/1000 LAN
(Hardware Port: USB 10/100/1000 LAN, Device: en5)
(3) Thunderbolt Bridge
(Hardware Port: Thunderbolt Bridge, Device: bridge0)
"#;

        let parsed = parse_macos_service_order(text);
        assert_eq!(parsed.get("en0"), Some(&1));
        assert_eq!(parsed.get("en5"), Some(&2));
        assert_eq!(parsed.get("bridge0"), Some(&3));
    }

    #[test]
    fn parses_macos_ifconfig_without_treating_properties_as_interfaces() {
        let text = r#"
lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384
        inet 127.0.0.1 netmask 0xff000000
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
        ether fc:aa:14:00:00:01
        inet6 fe80::1%en0 prefixlen 64 secured scopeid 0xb
        inet 192.168.25.138 netmask 0xffffff00 broadcast 192.168.25.255
        media: autoselect
        status: active
utun89: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1500
        inet 172.18.0.1 --> 172.18.0.1 netmask 0xffffffff
        inet6 fe80::2%utun89 prefixlen 64 scopeid 0x1d
"#;
        let service_order = HashMap::from([("en0".to_string(), 6)]);

        let parsed = parse_macos_ifconfig(text, &service_order);
        let names = parsed
            .iter()
            .map(|iface| iface.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["en0", "utun89"]);
        assert_eq!(parsed[0].mac, "fc:aa:14:00:00:01");
        assert_eq!(parsed[0].ipv4, "192.168.25.138");
        assert_eq!(parsed[0].metric, 6);
        assert_eq!(parsed[1].ipv4, "172.18.0.1");
        assert_eq!(parsed[1].iftype, "tun-tap");
    }
}
