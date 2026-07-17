//! 公共工具函数模块。

use serde::Serialize;
use std::collections::HashSet;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 延迟统计
#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub count: usize,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub avg_ms: Option<f64>,
}

/// 解析主机名为 IP 地址（消除各模块重复代码）
pub async fn resolve_host(host: &str) -> Option<IpAddr> {
    resolve_host_all(host).await.into_iter().next()
}

/// 解析主机名为所有可用 IP，优先使用 trust-dns，失败时回退到系统解析。
pub async fn resolve_host_all(host: &str) -> Vec<IpAddr> {
    resolve_host_all_timeout(host, DNS_LOOKUP_TIMEOUT).await
}

/// Resolve through the operating-system resolver with an explicit deadline.
///
/// The blocking OS call runs on a detached thread so a stuck `getaddrinfo`
/// cannot keep the Tokio runtime alive after the deadline expires.
pub async fn resolve_host_all_timeout(host: &str, timeout: Duration) -> Vec<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return vec![ip];
    }

    dedup_ips(resolve_host_system(host, timeout).await)
}

async fn resolve_host_system(host: &str, timeout: Duration) -> Vec<IpAddr> {
    let host = host.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let result = (host.as_str(), 0)
            .to_socket_addrs()
            .map(|addrs| addrs.map(|addr| addr.ip()).collect::<Vec<_>>())
            .unwrap_or_default();
        let _ = tx.send(result);
    });

    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(ips)) => ips,
        Ok(Err(_)) | Err(_) => Vec::new(),
    }
}

fn dedup_ips(ips: Vec<IpAddr>) -> Vec<IpAddr> {
    let mut seen = HashSet::new();
    ips.into_iter().filter(|ip| seen.insert(*ip)).collect()
}

/// 计算 min/max/avg 统计（消除 ping 和 connectivity 的重复）
pub fn compute_stats(rtts: &[f64]) -> Stats {
    if rtts.is_empty() {
        return Stats {
            count: 0,
            min_ms: None,
            max_ms: None,
            avg_ms: None,
        };
    }
    let min = rtts.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = rtts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg = rtts.iter().sum::<f64>() / rtts.len() as f64;
    Stats {
        count: rtts.len(),
        min_ms: Some(min),
        max_ms: Some(max),
        avg_ms: Some(avg),
    }
}

/// 获取系统代理地址（Windows 注册表 或 环境变量）
/// 返回格式化的代理 URL，如 "http://127.0.0.1:7897"
pub fn get_system_proxy_addr() -> Option<String> {
    // 1. 平台系统代理（Windows 注册表、macOS 系统设置、Linux 桌面环境设置等）
    if let Some(proxy) = crate::info::proxy::get_platform_system_proxy() {
        if let Some(addr) = proxy_to_url(&proxy) {
            return Some(addr);
        }
    }

    // 2. 环境变量
    for var in &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                return Some(val);
            }
        }
    }

    None
}

/// Select the effective system/environment proxy for a target URL.
pub fn get_system_proxy_for_url(target: &str) -> Option<String> {
    let normalized = if target.contains("://") {
        target.to_string()
    } else {
        format!("https://{target}")
    };
    let url = reqwest::Url::parse(&normalized).ok()?;
    let host = url.host_str()?;
    if proxy_bypassed(host, url.port_or_known_default(), None) {
        return None;
    }
    let platform_proxy = crate::info::proxy::get_platform_system_proxy();
    let platform_bypass = platform_proxy.as_deref().and_then(|proxy| {
        proxy
            .split(';')
            .find_map(|part| part.trim().strip_prefix("bypass=").map(str::to_string))
    });

    if platform_bypass
        .as_deref()
        .is_some_and(|rules| proxy_bypassed(host, url.port_or_known_default(), Some(rules)))
    {
        return None;
    }

    if let Some(proxy) = platform_proxy {
        if let Some(addr) = proxy_to_url_for_scheme(&proxy, url.scheme()) {
            return Some(addr);
        }
    }

    let vars: &[&str] = if url.scheme().eq_ignore_ascii_case("http") {
        &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
    } else {
        &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
    };
    for var in vars {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn proxy_bypassed(host: &str, port: Option<u16>, platform_rules: Option<&str>) -> bool {
    let env_rules = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .ok();
    env_rules
        .as_deref()
        .into_iter()
        .chain(platform_rules)
        .flat_map(|rules| rules.split(','))
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
        .any(|rule| proxy_rule_matches(rule, host, port))
}

fn proxy_rule_matches(rule: &str, host: &str, port: Option<u16>) -> bool {
    if rule == "*" {
        return true;
    }
    let (rule_host, rule_port) = if rule.parse::<IpAddr>().is_ok() {
        (rule, None)
    } else {
        match rule.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => {
                (host, port.parse::<u16>().ok())
            }
            _ => (rule, None),
        }
    };
    if rule_port.is_some() && rule_port != port {
        return false;
    }
    let rule_host = rule_host
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    host == rule_host || host.ends_with(&format!(".{rule_host}"))
}

fn proxy_to_url_for_scheme(proxy: &str, scheme: &str) -> Option<String> {
    let keys: &[&str] = if scheme.eq_ignore_ascii_case("http") {
        &["http=", "socks="]
    } else {
        &["https=", "http=", "socks="]
    };
    for key in keys {
        for part in proxy.split(';') {
            if let Some(addr) = part.trim().strip_prefix(key) {
                return Some(format_proxy_url(addr));
            }
        }
    }
    (!proxy.contains('=')).then(|| format_proxy_url(proxy))
}

/// Remove credentials from a URL before including it in human or JSON output.
pub fn redact_url_credentials(value: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(value) else {
        return value.to_string();
    };
    if !url.username().is_empty() {
        let _ = url.set_username("***");
    }
    if url.password().is_some() {
        let _ = url.set_password(Some("***"));
    }
    url.to_string()
}

pub fn redact_header_value(name: &str, value: &str) -> String {
    let name = name.to_ascii_lowercase();
    let sensitive = matches!(
        name.as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-api-key" | "api-key"
    ) || name.contains("token")
        || name.contains("secret");
    if sensitive {
        "***".to_string()
    } else {
        value.to_string()
    }
}

pub fn system_root_store() -> rustls::RootCertStore {
    let mut store = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for certificate in native.certs {
        let _ = store.add(certificate);
    }
    if store.is_empty() {
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    store
}

fn proxy_to_url(proxy: &str) -> Option<String> {
    // ProxyServer / scutil / gsettings 统一输出可能是
    // "http=host:port;https=host:port;socks=socks5h://host:port" 或 "host:port"。
    for key in ["https=", "http=", "socks="] {
        for part in proxy.split(';') {
            let part = part.trim();
            if let Some(addr) = part.strip_prefix(key) {
                return Some(format_proxy_url(addr));
            }
        }
    }

    if !proxy.contains('=') {
        Some(format_proxy_url(proxy))
    } else {
        None
    }
}

/// 将代理地址格式化为完整 URL
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn format_proxy_url(addr: &str) -> String {
    if addr.starts_with("http://") || addr.starts_with("https://") || addr.starts_with("socks") {
        addr.to_string()
    } else {
        format!("http://{}", addr)
    }
}

/// 解析端口列表字符串，支持逗号分隔和范围语法
///
/// "80,443,8080" → [80, 443, 8080]
/// "80-90,443"   → [80, 81, ..., 90, 443]
pub fn parse_ports(input: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start.trim().parse::<u16>(), end.trim().parse::<u16>()) {
                if s <= e {
                    for p in s..=e {
                        ports.push(p);
                    }
                }
            }
        } else if let Ok(p) = part.parse::<u16>() {
            ports.push(p);
        }
    }
    ports.sort();
    ports.dedup();
    ports
}

#[cfg(target_os = "windows")]
pub fn powershell_output(script: &str, timeout: Duration) -> Option<std::process::Output> {
    command_output_timeout(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
        timeout,
    )
}

pub fn command_output_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Option<std::process::Output> {
    use std::io::Read;
    use std::process::{Command, Output, Stdio};
    use std::time::Instant;

    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Some(Output {
                    status,
                    stdout: stdout_reader.join().unwrap_or_default(),
                    stderr: stderr_reader.join().unwrap_or_default(),
                });
            }
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return None;
            }
            Ok(None) => std::thread::sleep(COMMAND_POLL_INTERVAL),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ports_simple() {
        assert_eq!(parse_ports("80,443,8080"), vec![80, 443, 8080]);
    }

    #[test]
    fn test_parse_ports_range() {
        assert_eq!(parse_ports("80-83"), vec![80, 81, 82, 83]);
    }

    #[test]
    fn test_parse_ports_mixed() {
        assert_eq!(
            parse_ports("80-82,443,8080-8081"),
            vec![80, 81, 82, 443, 8080, 8081]
        );
    }

    #[test]
    fn test_parse_ports_dedup() {
        assert_eq!(parse_ports("80,80,443"), vec![80, 443]);
    }

    #[test]
    fn test_parse_ports_empty() {
        assert_eq!(parse_ports(""), Vec::<u16>::new());
    }

    #[test]
    fn test_parse_ports_spaces() {
        assert_eq!(parse_ports(" 80 , 443 "), vec![80, 443]);
    }

    #[test]
    fn test_compute_stats_empty() {
        let s = compute_stats(&[]);
        assert_eq!(s.count, 0);
        assert!(s.min_ms.is_none());
    }

    #[test]
    fn test_compute_stats_values() {
        let s = compute_stats(&[1.0, 2.0, 3.0]);
        assert_eq!(s.count, 3);
        assert_eq!(s.min_ms, Some(1.0));
        assert_eq!(s.max_ms, Some(3.0));
        assert_eq!(s.avg_ms, Some(2.0));
    }

    #[test]
    fn redacts_url_credentials() {
        assert_eq!(
            redact_url_credentials("socks5h://user:secret@example.com:1080"),
            "socks5h://***:***@example.com:1080"
        );
    }

    #[test]
    fn redacts_sensitive_headers() {
        assert_eq!(redact_header_value("Authorization", "Bearer secret"), "***");
        assert_eq!(
            redact_header_value("Accept", "application/json"),
            "application/json"
        );
    }

    #[test]
    fn matches_proxy_bypass_rules() {
        assert!(proxy_rule_matches(
            ".example.com",
            "api.example.com",
            Some(443)
        ));
        assert!(proxy_rule_matches("localhost", "localhost", Some(80)));
        assert!(proxy_rule_matches(
            "example.com:443",
            "example.com",
            Some(443)
        ));
        assert!(!proxy_rule_matches(
            "example.com:80",
            "example.com",
            Some(443)
        ));
    }

    #[test]
    fn selects_proxy_by_target_scheme() {
        let value = "http=127.0.0.1:8080;https=127.0.0.1:8443;socks=socks5h://127.0.0.1:1080";
        assert_eq!(
            proxy_to_url_for_scheme(value, "http").as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(
            proxy_to_url_for_scheme(value, "https").as_deref(),
            Some("http://127.0.0.1:8443")
        );
    }
}
