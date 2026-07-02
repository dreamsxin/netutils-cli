//! 代理检测模块。

use serde::Serialize;
use std::env;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Duration;

#[cfg(any(target_os = "macos", target_os = "linux"))]
const PROXY_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

/// 代理信息条目
#[derive(Debug, Clone, Serialize)]
pub struct ProxyEntry {
    pub ptype: String,
    pub value: String,
}

/// 获取所有代理设置（环境变量 + 平台系统代理）
pub fn get_proxy_info() -> Vec<ProxyEntry> {
    use crate::i18n::t;
    let mut proxies = Vec::new();

    let proxy_vars = [
        ("HTTP_PROXY", "proxy.http"),
        ("HTTPS_PROXY", "proxy.https"),
        ("ALL_PROXY", "proxy.all"),
        ("NO_PROXY", "proxy.no"),
    ];

    for (var, label_key) in &proxy_vars {
        let value = env::var(var)
            .or_else(|_| env::var(var.to_lowercase()))
            .unwrap_or_default();
        if !value.is_empty() {
            proxies.push(ProxyEntry {
                ptype: t(label_key),
                value,
            });
        }
    }

    if proxies.is_empty() {
        proxies.push(ProxyEntry {
            ptype: t("proxy.env"),
            value: t("common.not_set"),
        });
    }

    match get_platform_system_proxy() {
        Some(proxy) => proxies.push(ProxyEntry {
            ptype: t("proxy.system"),
            value: proxy,
        }),
        None => proxies.push(ProxyEntry {
            ptype: t("proxy.system"),
            value: t("proxy.disabled"),
        }),
    }

    proxies
}

pub fn get_platform_system_proxy() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        return get_windows_system_proxy();
    }

    #[cfg(target_os = "macos")]
    {
        return get_macos_system_proxy();
    }

    #[cfg(target_os = "linux")]
    {
        return get_linux_system_proxy();
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "windows")]
pub fn get_windows_system_proxy() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let internet_settings = hkcu
        .open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings",
            KEY_READ,
        )
        .ok()?;

    let proxy_enable: u32 = internet_settings.get_value("ProxyEnable").ok()?;
    if proxy_enable == 1 {
        let proxy_server: String = internet_settings.get_value("ProxyServer").ok()?;
        if proxy_server.is_empty() {
            None
        } else {
            Some(proxy_server)
        }
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
pub fn get_macos_system_proxy() -> Option<String> {
    let output =
        crate::util::command_output_timeout("scutil", &["--proxy"], PROXY_COMMAND_TIMEOUT)?;
    parse_macos_scutil_proxy(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_scutil_proxy(text: &str) -> Option<String> {
    let http_enabled = proxy_flag(text, "HTTPEnable");
    let https_enabled = proxy_flag(text, "HTTPSEnable");
    let socks_enabled = proxy_flag(text, "SOCKSEnable");
    let ftp_enabled = proxy_flag(text, "FTPEnable");
    let pac_enabled = proxy_flag(text, "ProxyAutoConfigEnable");
    let auto_discovery_enabled = proxy_flag(text, "ProxyAutoDiscoveryEnable");
    let exclude_simple = proxy_flag(text, "ExcludeSimpleHostnames");

    let http = if http_enabled {
        proxy_host_port(text, "HTTPProxy", "HTTPPort").map(|addr| format!("http={addr}"))
    } else {
        None
    };
    let https = if https_enabled {
        proxy_host_port(text, "HTTPSProxy", "HTTPSPort").map(|addr| format!("https={addr}"))
    } else {
        None
    };
    let socks = if socks_enabled {
        proxy_host_port(text, "SOCKSProxy", "SOCKSPort")
            .map(|addr| format!("socks=socks5h://{addr}"))
    } else {
        None
    };
    let ftp = if ftp_enabled {
        proxy_host_port(text, "FTPProxy", "FTPPort").map(|addr| format!("ftp={addr}"))
    } else {
        None
    };
    let pac = if pac_enabled {
        proxy_value(text, "ProxyAutoConfigURLString").map(|url| format!("pac={url}"))
    } else {
        None
    };
    let auto_discovery = if auto_discovery_enabled {
        Some("auto-discovery=true".to_string())
    } else {
        None
    };
    let simple_hosts = if exclude_simple {
        Some("exclude-simple-hostnames=true".to_string())
    } else {
        None
    };
    let exceptions = proxy_array_values(text, "ExceptionsList")
        .filter(|items| !items.is_empty())
        .map(|items| format!("bypass={}", items.join(",")));

    join_proxy_parts([
        http,
        https,
        socks,
        ftp,
        pac,
        auto_discovery,
        simple_hosts,
        exceptions,
    ])
}

#[cfg(target_os = "linux")]
pub fn get_linux_system_proxy() -> Option<String> {
    get_gnome_system_proxy()
}

#[cfg(target_os = "linux")]
fn get_gnome_system_proxy() -> Option<String> {
    let mode = gsettings_value("org.gnome.system.proxy", "mode")?;
    if mode == "auto" {
        return gsettings_value("org.gnome.system.proxy", "autoconfig-url")
            .map(|url| format!("pac={url}"));
    }
    if mode != "manual" {
        return None;
    }

    let http_host = gsettings_value("org.gnome.system.proxy.http", "host");
    let http_port = gsettings_value("org.gnome.system.proxy.http", "port");
    let https_host = gsettings_value("org.gnome.system.proxy.https", "host");
    let https_port = gsettings_value("org.gnome.system.proxy.https", "port");
    let socks_host = gsettings_value("org.gnome.system.proxy.socks", "host");
    let socks_port = gsettings_value("org.gnome.system.proxy.socks", "port");

    let http = proxy_part_from_host_port("http", http_host, http_port, false);
    let https = proxy_part_from_host_port("https", https_host, https_port, false);
    let socks = proxy_part_from_host_port("socks", socks_host, socks_port, true);

    join_proxy_parts([http, https, socks])
}

#[cfg(target_os = "linux")]
fn gsettings_value(schema: &str, key: &str) -> Option<String> {
    let output = crate::util::command_output_timeout(
        "gsettings",
        &["get", schema, key],
        PROXY_COMMAND_TIMEOUT,
    )?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let value = value.trim_matches('\'').trim_matches('"').to_string();
    if value.is_empty() || value == "0" {
        None
    } else {
        Some(value)
    }
}

#[cfg(any(target_os = "macos", test))]
fn proxy_flag(text: &str, key: &str) -> bool {
    proxy_value(text, key).as_deref() == Some("1")
}

#[cfg(any(target_os = "macos", test))]
fn proxy_host_port(text: &str, host_key: &str, port_key: &str) -> Option<String> {
    let host = proxy_value(text, host_key)?;
    let port = proxy_value(text, port_key)?;
    if host.is_empty() || port.is_empty() || port == "0" {
        None
    } else {
        Some(format!("{host}:{port}"))
    }
}

#[cfg(any(target_os = "macos", test))]
fn proxy_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                return Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }
    None
}

#[cfg(any(target_os = "macos", test))]
fn proxy_array_values(text: &str, key: &str) -> Option<Vec<String>> {
    let mut values = Vec::new();
    let mut in_array = false;

    for line in text.lines().map(str::trim) {
        if !in_array {
            if line.starts_with(key) && line.contains("<array>") {
                in_array = true;
            }
            continue;
        }

        if line == "}" {
            break;
        }

        if let Some((_, value)) = line.split_once(':') {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                values.push(value.to_string());
            }
        }
    }

    Some(values)
}

#[cfg(target_os = "linux")]
fn proxy_part_from_host_port(
    kind: &str,
    host: Option<String>,
    port: Option<String>,
    socks: bool,
) -> Option<String> {
    let host = host?;
    let port = port?;
    if host.is_empty() || port.is_empty() || port == "0" {
        return None;
    }
    if socks {
        Some(format!("{kind}=socks5h://{host}:{port}"))
    } else {
        Some(format!("{kind}={host}:{port}"))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn join_proxy_parts<const N: usize>(parts: [Option<String>; N]) -> Option<String> {
    let joined = parts.into_iter().flatten().collect::<Vec<_>>().join(";");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_macos_proxy_http_https_socks() {
        let text = r#"
<dictionary> {
  HTTPEnable : 1
  HTTPPort : 7890
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 7891
  HTTPSProxy : proxy.local
  SOCKSEnable : 1
  SOCKSPort : 1080
  SOCKSProxy : 10.0.0.1
  FTPEnable : 1
  FTPPort : 2121
  FTPProxy : ftp.local
  ProxyAutoConfigEnable : 1
  ProxyAutoConfigURLString : http://proxy.local/proxy.pac
  ExcludeSimpleHostnames : 1
  ExceptionsList : <array> {
    0 : *.local
    1 : 169.254/16
  }
}
"#;

        let parsed = parse_macos_scutil_proxy(text).unwrap();
        assert_eq!(
            parsed,
            "http=127.0.0.1:7890;https=proxy.local:7891;socks=socks5h://10.0.0.1:1080;ftp=ftp.local:2121;pac=http://proxy.local/proxy.pac;exclude-simple-hostnames=true;bypass=*.local,169.254/16"
        );
    }

    #[test]
    fn parse_macos_proxy_disabled() {
        let text = r#"
<dictionary> {
  HTTPEnable : 0
  HTTPSEnable : 0
  SOCKSEnable : 0
}
"#;

        assert!(parse_macos_scutil_proxy(text).is_none());
    }
}
