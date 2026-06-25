//! Linux/macOS 路由表实现。

use std::process::Command;

use super::route::RouteEntry;

/// 获取默认路由 (网关, 接口名)
pub fn get_default_routes() -> Vec<(String, String)> {
    let mut routes = Vec::new();

    #[cfg(target_os = "linux")]
    {
        // ip route show default
        if let Ok(output) = Command::new("ip").args(["route", "show", "default"]).output() {
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
        // route -n get default
        if let Ok(output) = Command::new("route").args(["-n", "get", "default"]).output() {
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
        if let Ok(output) = Command::new("netstat").args(["-rn"]).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut in_table = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with("Destination") {
                    in_table = true;
                    continue;
                }
                if !in_table || line.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    routes.push(RouteEntry {
                        destination: parts[0].to_string(),
                        gateway: parts[1].to_string(),
                        interface: parts[3].to_string(),
                        metric: "0".to_string(),
                    });
                }
                if routes.len() >= 20 {
                    break;
                }
            }
        }
    }

    routes
}
