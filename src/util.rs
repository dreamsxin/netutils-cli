//! 公共工具函数模块。

use serde::Serialize;
use std::net::IpAddr;

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
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(ip);
    }

    use trust_dns_resolver::config::*;
    use trust_dns_resolver::TokioAsyncResolver;

    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());
    match resolver.lookup_ip(host).await {
        Ok(ips) => ips.iter().next(),
        Err(_) => None,
    }
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

/// 格式化延迟为字符串
#[allow(dead_code)]
pub fn fmt_ms(ms: f64) -> String {
    format!("{:.2}ms", ms)
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
        assert_eq!(parse_ports("80-82,443,8080-8081"), vec![80, 81, 82, 443, 8080, 8081]);
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
