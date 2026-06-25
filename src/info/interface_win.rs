//! Windows 网络接口实现（PowerShell Get-NetAdapter）。

use std::process::Command;

use super::interface::{classify_interface, InterfaceInfo};

/// 从 PowerShell 获取所有网络接口信息（含接口跃点）
pub fn get_all_interfaces() -> Vec<InterfaceInfo> {
    let mut interfaces = Vec::new();

    let ps_script = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-NetAdapter | ForEach-Object {
    $adapter = $_
    $ip = Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue | Select-Object -First 1
    $metric = Get-NetIPInterface -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue | Select-Object -First 1
    [PSCustomObject]@{
        Name = $adapter.InterfaceAlias
        MAC = $adapter.MacAddress
        IPv4 = if ($ip) { $ip.IPAddress } else { "--" }
        Status = $adapter.Status
        Type = $adapter.InterfaceDescription
        Index = $adapter.ifIndex
        Metric = if ($metric) { $metric.InterfaceMetric } else { 0 }
    }
} | ForEach-Object { "$($_.Name)|$($_.MAC)|$($_.IPv4)|$($_.Status)|$($_.Type)|$($_.Index)|$($_.Metric)" }
"#;

    if let Ok(output) = Command::new("powershell").args(["-Command", ps_script]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.splitn(7, '|').collect();
            if parts.len() >= 7 {
                let iftype = classify_interface(parts[4], parts[0]);
                interfaces.push(InterfaceInfo {
                    name: parts[0].trim().to_string(),
                    mac: parts[1].trim().to_string(),
                    ipv4: parts[2].trim().to_string(),
                    status: parts[3].trim().to_string(),
                    description: parts[4].trim().to_string(),
                    metric: parts[6].trim().parse().unwrap_or(0u32),
                    iftype: iftype.to_id(),
                    is_virtual: iftype.is_virtual(),
                    is_egress: false,
                    is_backup: false,
                });
            }
        }
    }

    interfaces
}
