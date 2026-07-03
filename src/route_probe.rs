//! Cross-platform one-target route probing.

use std::time::Duration;

use serde::Serialize;

const ROUTE_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize)]
pub struct RouteProbe {
    pub target: String,
    pub interface: Option<String>,
    pub gateway: Option<String>,
    pub source: String,
}

pub fn route_to_target(target: &str) -> Option<RouteProbe> {
    #[cfg(target_os = "windows")]
    {
        return route_to_target_windows(target);
    }
    #[cfg(target_os = "macos")]
    {
        return route_to_target_macos(target);
    }
    #[cfg(target_os = "linux")]
    {
        return route_to_target_linux(target);
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "windows")]
fn route_to_target_windows(target: &str) -> Option<RouteProbe> {
    let script = format!(
        r#"
$route = Find-NetRoute -RemoteIPAddress "{}" -ErrorAction SilentlyContinue | Sort-Object RouteMetric, InterfaceMetric | Select-Object -First 1
if ($route) {{ "$($route.InterfaceAlias)|$($route.NextHop)" }}
"#,
        target
    );
    let output = crate::util::powershell_output(&script, ROUTE_PROBE_TIMEOUT)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (iface, gateway) = text.trim().split_once('|')?;
    Some(RouteProbe {
        target: target.to_string(),
        interface: non_empty(iface),
        gateway: non_empty(gateway).filter(|gw| gw != "0.0.0.0" && gw != "::"),
        source: "Find-NetRoute".to_string(),
    })
}

#[cfg(target_os = "macos")]
fn route_to_target_macos(target: &str) -> Option<RouteProbe> {
    let output =
        crate::util::command_output_timeout("route", &["-n", "get", target], ROUTE_PROBE_TIMEOUT)?;
    let mut interface = None;
    let mut gateway = None;
    for line in String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
    {
        if let Some(value) = line.strip_prefix("interface:") {
            interface = non_empty(value);
        } else if let Some(value) = line.strip_prefix("gateway:") {
            gateway = non_empty(value);
        }
    }
    Some(RouteProbe {
        target: target.to_string(),
        interface,
        gateway,
        source: "route -n get".to_string(),
    })
}

#[cfg(target_os = "linux")]
fn route_to_target_linux(target: &str) -> Option<RouteProbe> {
    let output =
        crate::util::command_output_timeout("ip", &["route", "get", target], ROUTE_PROBE_TIMEOUT)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let parts = text.split_whitespace().collect::<Vec<_>>();
    let mut interface = None;
    let mut gateway = None;
    for (idx, part) in parts.iter().enumerate() {
        if *part == "dev" {
            interface = parts.get(idx + 1).and_then(|v| non_empty(v));
        } else if *part == "via" {
            gateway = parts.get(idx + 1).and_then(|v| non_empty(v));
        }
    }
    Some(RouteProbe {
        target: target.to_string(),
        interface,
        gateway,
        source: "ip route get".to_string(),
    })
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}
