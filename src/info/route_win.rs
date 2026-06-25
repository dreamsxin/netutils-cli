//! Windows 路由表实现（PowerShell Get-NetRoute）。

use std::process::Command;

use super::route::RouteEntry;

/// 从路由表获取所有默认路由 (网关, 接口名)
pub fn get_default_routes() -> Vec<(String, String)> {
    let mut routes = Vec::new();

    let ps_script = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-NetRoute -DestinationPrefix "0.0.0.0/0" -ErrorAction SilentlyContinue | Sort-Object RouteMetric | ForEach-Object { "$($_.NextHop)|$($_.InterfaceAlias)" }
"#;

    if let Ok(output) = Command::new("powershell").args(["-Command", ps_script]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if let Some((gw, iface)) = line.split_once('|') {
                routes.push((gw.trim().to_string(), iface.trim().to_string()));
            }
        }
    }

    routes
}

/// 获取路由表（过滤掉多播/广播，默认路由优先，最多 20 条）
pub fn get_route_table() -> Vec<RouteEntry> {
    let mut routes = Vec::new();

    let ps_script = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-NetRoute -ErrorAction SilentlyContinue | Where-Object {
    $_.DestinationPrefix -notlike 'ff*' -and $_.DestinationPrefix -notlike '224*' -and $_.DestinationPrefix -notlike '255*'
} | Sort-Object RouteMetric | Select-Object -First 20 | ForEach-Object {
    "$($_.DestinationPrefix)|$($_.NextHop)|$($_.InterfaceAlias)|$($_.RouteMetric)"
}
"#;

    if let Ok(output) = Command::new("powershell").args(["-Command", ps_script]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.splitn(4, '|').collect();
            if parts.len() >= 4 {
                routes.push(RouteEntry {
                    destination: parts[0].trim().to_string(),
                    gateway: if parts[1] == "0.0.0.0" || parts[1] == "::" {
                        "--".to_string()
                    } else {
                        parts[1].trim().to_string()
                    },
                    interface: parts[2].trim().to_string(),
                    metric: parts[3].trim().to_string(),
                });
            }
        }
    }

    routes
}
