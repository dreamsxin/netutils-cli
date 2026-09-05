//! DNS over TLS (RFC 7858) 客户端。
//!
//! 与 [`crate::doh`] 一起补齐 `dns_path.rs` / `dns_leak.rs` 里标注的
//! 「DoH/DoT 可能绕过系统 DNS 服务器列表」这一盲区的另一半。
//!
//! DoT 是 853 端口上的裸 TLS 流，不是 HTTP，因此**不支持代理**：
//! 穿代理需要 CONNECT 隧道，本项目的 HTTP 客户端不参与这条链路。
//! 与其静默地退回直连，不如在参数层面就拒绝，见 `dns` 命令的互斥约束。

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use trust_dns_resolver::proto::rr::RecordType;

use crate::dns::DnsRecord;

/// RFC 7858 规定的 DoT 默认端口
const DOT_PORT: u16 = 853;
/// DNS 报文上限（RFC 1035 对 TCP 传输使用 2 字节长度前缀，理论上限 65535）
const MAX_MESSAGE_LEN: usize = 65_535;

/// 常用 DoT 提供商预设。取值是用于证书校验和 SNI 的主机名。
const PRESETS: [(&str, &str); 6] = [
    ("cloudflare", "one.one.one.one"),
    ("google", "dns.google"),
    ("quad9", "dns.quad9.net"),
    ("adguard", "dns.adguard-dns.com"),
    ("alidns", "dns.alidns.com"),
    ("dnspod", "dot.pub"),
];

/// DoT 查询结果
#[derive(Debug, Serialize)]
pub struct DotAnswer {
    /// 实际连接的 `host:port`
    pub endpoint: String,
    /// 预设名；直接传主机名时为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// 实际连接的服务器 IP
    pub server_ip: String,
    /// 协商出的 TLS 版本，用于确认查询确实被加密
    pub tls_version: Option<String>,
    pub response_code: String,
    pub records: Vec<DnsRecord>,
    pub elapsed_ms: f64,
}

/// 列出所有预设名，供帮助文本和错误提示使用。
pub fn preset_names() -> Vec<&'static str> {
    PRESETS.iter().map(|(name, _)| *name).collect()
}

/// 把 `--dot` 的取值解析成 `(主机名, 端口, 预设名)`。
///
/// 接受预设名、`host` 或 `host:port`。主机名是必需的：DoT 依赖证书校验，
/// 只给 IP 无法验证服务器身份，那样的「加密」没有意义。
pub fn resolve_target(value: &str) -> Result<(String, u16, Option<String>), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("DoT server is empty".to_string());
    }

    let lower = trimmed.to_ascii_lowercase();
    if let Some((name, host)) = PRESETS.iter().find(|(name, _)| *name == lower) {
        return Ok(((*host).to_string(), DOT_PORT, Some((*name).to_string())));
    }

    if lower.contains("://") {
        return Err(format!(
            "invalid DoT server `{trimmed}`: expected host or host:port, not a URL; \
             presets: {}",
            preset_names().join(", ")
        ));
    }

    let (host, port) = split_host_port(trimmed)?;
    if host.parse::<IpAddr>().is_ok() {
        return Err(format!(
            "DoT requires a hostname for certificate validation, got IP `{host}`; \
             use a preset ({}) or the resolver's hostname",
            preset_names().join(", ")
        ));
    }
    Ok((host, port, None))
}

fn split_host_port(value: &str) -> Result<(String, u16), String> {
    // IPv6 字面量会带方括号，但上面已经拒绝纯 IP，这里只需处理 host[:port]。
    match value.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("invalid DoT port `{port}`"))?;
            if port == 0 {
                return Err("DoT port must not be 0".to_string());
            }
            Ok((host.to_string(), port))
        }
        _ => Ok((value.to_string(), DOT_PORT)),
    }
}

/// 通过 DoT 查询一条记录。
pub async fn query(
    target: &str,
    domain: &str,
    record_type: RecordType,
    timeout: Duration,
) -> Result<DotAnswer, String> {
    let (host, port, preset) = resolve_target(target)?;
    // wire format 与传输无关，直接复用 DoH 侧的编解码。
    let message = crate::doh::encode_query(domain, record_type)?;

    let ips = crate::util::resolve_host_all_timeout(&host, timeout).await;
    let server_ip = *ips
        .first()
        .ok_or_else(|| format!("cannot resolve DoT server `{host}`"))?;

    let start = Instant::now();
    let (response_code, records, tls_version) =
        tokio::time::timeout(timeout, exchange(&host, server_ip, port, &message))
            .await
            .map_err(|_| format!("DoT query to {host}:{port} timed out"))??;
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    Ok(DotAnswer {
        endpoint: format!("{host}:{port}"),
        preset,
        server_ip: server_ip.to_string(),
        tls_version,
        response_code,
        records,
        elapsed_ms,
    })
}

async fn exchange(
    host: &str,
    server_ip: IpAddr,
    port: u16,
    message: &[u8],
) -> Result<(String, Vec<DnsRecord>, Option<String>), String> {
    let stream = TcpStream::connect((server_ip, port))
        .await
        .map_err(|err| format!("TCP connect to {server_ip}:{port} failed: {err}"))?;

    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|err| format!("invalid DoT server name `{host}`: {err}"))?;
    let connector =
        build_connector().map_err(|err| format!("failed to build TLS config: {err}"))?;
    let mut tls = connector
        .connect(server_name, stream)
        .await
        .map_err(|err| format!("TLS handshake with {host} failed: {err}"))?;

    let tls_version = tls
        .get_ref()
        .1
        .protocol_version()
        .map(|version| format!("{version:?}"));

    // RFC 7858 沿用 RFC 1035 的 TCP 封装：2 字节大端长度前缀。
    let length = u16::try_from(message.len())
        .map_err(|_| "DNS query exceeds the 65535-byte TCP framing limit".to_string())?;
    let mut framed = Vec::with_capacity(message.len() + 2);
    framed.extend_from_slice(&length.to_be_bytes());
    framed.extend_from_slice(message);
    tls.write_all(&framed)
        .await
        .map_err(|err| format!("failed to send DoT query: {err}"))?;
    tls.flush()
        .await
        .map_err(|err| format!("failed to flush DoT query: {err}"))?;

    let mut len_buf = [0u8; 2];
    tls.read_exact(&mut len_buf)
        .await
        .map_err(|err| format!("failed to read DoT response length: {err}"))?;
    let response_len = usize::from(u16::from_be_bytes(len_buf));
    if response_len == 0 {
        return Err("DoT server returned an empty response".to_string());
    }
    if response_len > MAX_MESSAGE_LEN {
        return Err(format!(
            "DoT response length {response_len} exceeds the DNS message limit"
        ));
    }

    let mut body = vec![0u8; response_len];
    tls.read_exact(&mut body)
        .await
        .map_err(|err| format!("failed to read DoT response body: {err}"))?;

    let (response_code, records) = crate::doh::decode_response(&body)?;
    Ok((response_code, records, tls_version))
}

fn build_connector() -> Result<TlsConnector, rustls::Error> {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(crate::util::system_root_store())
    .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_presets() {
        let (host, port, preset) = resolve_target("cloudflare").unwrap();

        assert_eq!(host, "one.one.one.one");
        assert_eq!(port, DOT_PORT);
        assert_eq!(preset.as_deref(), Some("cloudflare"));
    }

    #[test]
    fn preset_lookup_is_case_insensitive() {
        assert_eq!(resolve_target(" Quad9 ").unwrap().0, "dns.quad9.net");
    }

    #[test]
    fn accepts_bare_hostname_with_default_port() {
        let (host, port, preset) = resolve_target("dns.example.net").unwrap();

        assert_eq!(host, "dns.example.net");
        assert_eq!(port, 853);
        assert_eq!(preset, None);
    }

    #[test]
    fn accepts_explicit_port() {
        let (host, port, _) = resolve_target("dns.example.net:8853").unwrap();

        assert_eq!(host, "dns.example.net");
        assert_eq!(port, 8853);
    }

    #[test]
    fn rejects_bare_ip_because_certificates_cannot_be_validated() {
        let err = resolve_target("1.1.1.1").unwrap_err();

        assert!(err.contains("requires a hostname"), "{err}");
    }

    #[test]
    fn rejects_ip_with_port() {
        let err = resolve_target("1.1.1.1:853").unwrap_err();

        assert!(err.contains("requires a hostname"), "{err}");
    }

    #[test]
    fn rejects_url_form() {
        let err = resolve_target("tls://dns.example.net").unwrap_err();

        assert!(err.contains("expected host or host:port"), "{err}");
    }

    #[test]
    fn rejects_invalid_port() {
        assert!(resolve_target("dns.example.net:notaport").is_err());
        assert!(resolve_target("dns.example.net:0").is_err());
        assert!(resolve_target("dns.example.net:70000").is_err());
    }

    #[test]
    fn rejects_empty_target() {
        assert!(resolve_target("   ").is_err());
    }

    #[test]
    fn presets_all_use_hostnames() {
        for (name, host) in PRESETS {
            assert!(
                host.parse::<IpAddr>().is_err(),
                "{name} preset must be a hostname, not an IP"
            );
            // 预设名必须能被 resolve_target 找回，否则帮助文本会撒谎。
            assert_eq!(resolve_target(name).unwrap().0, host);
        }
    }

    #[test]
    fn splits_host_and_port() {
        assert_eq!(
            split_host_port("dns.example.net").unwrap(),
            ("dns.example.net".to_string(), 853)
        );
        assert_eq!(
            split_host_port("dns.example.net:5353").unwrap(),
            ("dns.example.net".to_string(), 5353)
        );
    }
}
