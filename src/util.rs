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
    if let Ok(ip) = host.parse::<IpAddr>() {
        return vec![ip];
    }

    let mut ips = resolve_host_trust_dns(host).await;
    if ips.is_empty() {
        ips = resolve_host_system(host).await;
    }

    dedup_ips(ips)
}

async fn resolve_host_trust_dns(host: &str) -> Vec<IpAddr> {
    use trust_dns_resolver::config::*;
    use trust_dns_resolver::TokioAsyncResolver;

    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());
    match tokio::time::timeout(DNS_LOOKUP_TIMEOUT, resolver.lookup_ip(host)).await {
        Ok(Ok(ips)) => ips.iter().collect(),
        Ok(Err(_)) | Err(_) => Vec::new(),
    }
}

async fn resolve_host_system(host: &str) -> Vec<IpAddr> {
    let host = host.to_string();
    let lookup = tokio::time::timeout(
        DNS_LOOKUP_TIMEOUT,
        tokio::task::spawn_blocking(move || (host.as_str(), 0).to_socket_addrs()),
    )
    .await;

    match lookup {
        Ok(Ok(Ok(addrs))) => addrs.map(|addr| addr.ip()).collect(),
        Ok(Ok(Err(_))) | Ok(Err(_)) | Err(_) => Vec::new(),
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
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(COMMAND_POLL_INTERVAL),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
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
}
