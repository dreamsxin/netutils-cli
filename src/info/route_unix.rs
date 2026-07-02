//! Linux/macOS 路由表实现。

use std::process::Command;

use super::route::RouteEntry;

#[cfg(any(target_os = "macos", test))]
use std::collections::HashMap;

/// 获取默认路由 (网关, 接口名)
pub fn get_default_routes() -> Vec<(String, String)> {
    let mut routes = Vec::new();

    #[cfg(target_os = "linux")]
    {
        // ip route show default
        if let Ok(output) = Command::new("ip")
            .args(["route", "show", "default"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                let mut gw = String::new();
                let mut iface = String::new();
                for (i, &word) in parts.iter().enumerate() {
                    if word == "via" && i + 1 < parts.len() {
                        gw = parts[i + 1].to_string();
                    }
                    if word == "dev" && i + 1 < parts.len() {
                        iface = parts[i + 1].to_string();
                    }
                }
                if !gw.is_empty() && !iface.is_empty() {
                    routes.push((gw, iface));
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // netstat can list multiple default routes; sort them by macOS network service order.
        if let Ok(output) = Command::new("netstat").args(["-rn", "-f", "inet"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut defaults = parse_macos_netstat_routes(&text)
                .into_iter()
                .filter(|r| r.destination == "default")
                .collect::<Vec<_>>();
            let service_order = get_macos_service_order();
            defaults.sort_by_key(|r| service_order.get(&r.interface).copied().unwrap_or(u32::MAX));
            for route in defaults {
                if route.gateway != "--" && route.interface != "--" {
                    routes.push((route.gateway, route.interface));
                }
            }
        }

        // Fallback to the active route if netstat output is unavailable.
        if routes.is_empty() {
            if let Ok(output) = Command::new("route")
                .args(["-n", "get", "default"])
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                let mut gw = String::new();
                let mut iface = String::new();
                for line in text.lines() {
                    let line = line.trim();
                    if let Some(v) = line.strip_prefix("gateway:") {
                        gw = v.trim().to_string();
                    }
                    if let Some(v) = line.strip_prefix("interface:") {
                        iface = v.trim().to_string();
                    }
                }
                if !gw.is_empty() && !iface.is_empty() {
                    routes.push((gw, iface));
                }
            }
        }
    }

    routes
}

/// 获取路由表（最多 20 条）
pub fn get_route_table() -> Vec<RouteEntry> {
    let mut routes = Vec::new();

    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("ip").args(["route", "show"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.is_empty() {
                    continue;
                }
                let dest = parts[0].to_string();
                let mut gw = "--".to_string();
                let mut iface = "--".to_string();
                let mut metric = "0".to_string();
                for (i, &word) in parts.iter().enumerate() {
                    if word == "via" && i + 1 < parts.len() {
                        gw = parts[i + 1].to_string();
                    }
                    if word == "dev" && i + 1 < parts.len() {
                        iface = parts[i + 1].to_string();
                    }
                    if word == "metric" && i + 1 < parts.len() {
                        metric = parts[i + 1].to_string();
                    }
                }
                routes.push(RouteEntry {
                    destination: dest,
                    gateway: gw,
                    interface: iface,
                    metric,
                });
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("netstat").args(["-rn", "-f", "inet"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            routes = parse_macos_netstat_routes(&text);
            let service_order = get_macos_service_order();
            for route in &mut routes {
                route.metric = service_order
                    .get(&route.interface)
                    .map(u32::to_string)
                    .unwrap_or_else(|| "0".to_string());
            }
            routes.sort_by_key(|r| if r.destination == "default" { 0 } else { 1 });
            routes.truncate(20);
        }
    }

    routes
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_netstat_routes(text: &str) -> Vec<RouteEntry> {
    let mut routes = Vec::new();
    let mut in_table = false;

    for line in text.lines().map(str::trim) {
        if line.starts_with("Destination") {
            in_table = true;
            continue;
        }
        if !in_table
            || line.is_empty()
            || line.starts_with("Internet")
            || line.starts_with("Routing")
        {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            routes.push(RouteEntry {
                destination: parts[0].to_string(),
                gateway: if parts[1].starts_with("link#") {
                    "--".to_string()
                } else {
                    parts[1].to_string()
                },
                interface: parts[3].to_string(),
                metric: "0".to_string(),
            });
        }
    }

    routes
}

#[cfg(target_os = "macos")]
fn get_macos_service_order() -> HashMap<String, u32> {
    let output = match Command::new("networksetup")
        .arg("-listnetworkserviceorder")
        .output()
    {
        Ok(o) => o,
        Err(_) => return HashMap::new(),
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
    fn parses_macos_netstat_routes() {
        let text = r#"
Routing tables

Internet:
Destination        Gateway            Flags               Netif Expire
default            192.168.1.1        UGScg                 en0
default            10.8.0.1           UGScI              utun4
127                127.0.0.1          UCS                   lo0
192.168.1/24       link#15            UCS                   en0      !
"#;

        let routes = parse_macos_netstat_routes(text);
        assert_eq!(routes.len(), 4);
        assert_eq!(routes[0].destination, "default");
        assert_eq!(routes[0].gateway, "192.168.1.1");
        assert_eq!(routes[0].interface, "en0");
        assert_eq!(routes[3].gateway, "--");
    }

    #[test]
    fn parses_macos_network_service_order() {
        let text = r#"
An asterisk (*) denotes that a network service is disabled.
(1) Wi-Fi
(Hardware Port: Wi-Fi, Device: en0)
(2) USB 10/100/1000 LAN
(Hardware Port: USB 10/100/1000 LAN, Device: en5)
"#;

        let parsed = parse_macos_service_order(text);
        assert_eq!(parsed.get("en0"), Some(&1));
        assert_eq!(parsed.get("en5"), Some(&2));
    }
}
