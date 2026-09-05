//! DNS over HTTPS (RFC 8484) 客户端。
//!
//! 这里补上项目此前自己标注的盲区：`dns_path.rs` 和 `dns_leak.rs` 都提示
//! 「DoH/DoT 可能绕过系统 DNS 服务器列表」，但没有任何命令能真正走 DoH。
//!
//! 实现刻意基于 `reqwest` 而不是 trust-dns 自带的 DoH：只有这样查询才能
//! 复用本项目的代理选择逻辑。DoH 走代理正是排查「代理侧 DNS 行为」的关键，
//! 而 trust-dns 内建的 DoH 无法接入这里的代理配置。

use std::time::{Duration, Instant};

use serde::Serialize;
use trust_dns_resolver::proto::op::{Message, MessageType, OpCode, Query};
use trust_dns_resolver::proto::rr::{Name, RecordType};

use crate::dns::DnsRecord;

/// RFC 8484 规定的 DNS 报文媒体类型
const DNS_MESSAGE: &str = "application/dns-message";

/// 常用 DoH 提供商预设，省去手打完整 URL。
const PRESETS: [(&str, &str); 6] = [
    ("cloudflare", "https://cloudflare-dns.com/dns-query"),
    ("google", "https://dns.google/dns-query"),
    ("quad9", "https://dns.quad9.net/dns-query"),
    ("adguard", "https://dns.adguard-dns.com/dns-query"),
    ("alidns", "https://dns.alidns.com/dns-query"),
    ("dnspod", "https://doh.pub/dns-query"),
];

/// DoH 查询结果
#[derive(Debug, Serialize)]
pub struct DohAnswer {
    /// 实际请求的 DoH endpoint
    pub endpoint: String,
    /// 预设名；直接传 URL 时为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    pub proxy: DohProxy,
    /// DNS 响应码，例如 NOERROR / NXDOMAIN
    pub response_code: String,
    pub records: Vec<DnsRecord>,
    pub elapsed_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct DohProxy {
    pub mode: String,
    pub value: Option<String>,
}

/// 列出所有预设名，供帮助文本和错误提示使用。
pub fn preset_names() -> Vec<&'static str> {
    PRESETS.iter().map(|(name, _)| *name).collect()
}

/// 把 `--doh` 的取值解析成 endpoint URL。
///
/// 接受预设名（如 `cloudflare`）或完整的 https URL。刻意拒绝 `http://`：
/// 明文 HTTP 上的 DoH 没有任何隐私意义，静默接受只会给出虚假的安全感。
pub fn resolve_endpoint(value: &str) -> Result<(String, Option<String>), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("DoH endpoint is empty".to_string());
    }

    let lower = trimmed.to_ascii_lowercase();
    if let Some((name, url)) = PRESETS.iter().find(|(name, _)| *name == lower) {
        return Ok((url.to_string(), Some((*name).to_string())));
    }

    if lower.starts_with("http://") {
        return Err(format!(
            "refusing plaintext DoH endpoint `{trimmed}`: use https://, \
             or one of the presets: {}",
            preset_names().join(", ")
        ));
    }

    if lower.starts_with("https://") {
        return Ok((trimmed.to_string(), None));
    }

    // 既不是已知预设也不是 URL，很可能是拼错了预设名。
    if !trimmed.contains("://") && !trimmed.contains('/') {
        return Err(format!(
            "unknown DoH preset `{trimmed}`; available presets: {}",
            preset_names().join(", ")
        ));
    }

    Err(format!(
        "invalid DoH endpoint `{trimmed}`: expected an https:// URL or one of: {}",
        preset_names().join(", ")
    ))
}

/// 编码一条 DNS 查询为 RFC 8484 的 wire format。
///
/// ID 固定为 0：RFC 8484 建议如此，以便 HTTP 缓存能命中相同查询。
pub fn encode_query(domain: &str, record_type: RecordType) -> Result<Vec<u8>, String> {
    let name = Name::from_utf8(domain)
        .map_err(|err| format!("invalid domain `{domain}`: {err}"))?
        .to_lowercase();
    let mut message = Message::new();
    message
        .set_id(0)
        .set_message_type(MessageType::Query)
        .set_op_code(OpCode::Query)
        .set_recursion_desired(true)
        .add_query(Query::query(name, record_type));
    message
        .to_vec()
        .map_err(|err| format!("failed to encode DNS query: {err}"))
}

/// 解码 DoH 响应，返回响应码和记录列表。
pub fn decode_response(bytes: &[u8]) -> Result<(String, Vec<DnsRecord>), String> {
    let message =
        Message::from_vec(bytes).map_err(|err| format!("failed to decode DNS response: {err}"))?;
    let response_code = message.response_code().to_string();
    let records = message
        .answers()
        .iter()
        .filter_map(|record| {
            record.data().map(|data| DnsRecord {
                value: crate::dns::format_record(data),
                ttl: record.ttl(),
            })
        })
        .collect();
    Ok((response_code, records))
}

/// 通过 DoH 查询一条记录。
pub async fn query(
    endpoint_value: &str,
    domain: &str,
    record_type: RecordType,
    timeout: Duration,
    proxy: Option<String>,
    no_proxy: bool,
) -> Result<DohAnswer, String> {
    let (endpoint, preset) = resolve_endpoint(endpoint_value)?;
    let body = encode_query(domain, record_type)?;

    // DoH 是普通的 HTTPS 请求，因此复用与其他命令一致的代理选择。
    let proxy_value = if no_proxy {
        None
    } else {
        proxy.or_else(|| crate::util::get_system_proxy_for_url(&endpoint))
    };
    let proxy_info = DohProxy {
        mode: if no_proxy {
            "direct-forced".to_string()
        } else if proxy_value.is_some() {
            "proxy".to_string()
        } else {
            "direct".to_string()
        },
        value: proxy_value
            .as_deref()
            .map(crate::util::redact_url_credentials),
    };

    let client = build_client(timeout, proxy_value.as_deref())
        .map_err(|err| format!("failed to build HTTP client: {err}"))?;

    let start = Instant::now();
    let response = client
        .post(&endpoint)
        .header(reqwest::header::CONTENT_TYPE, DNS_MESSAGE)
        .header(reqwest::header::ACCEPT, DNS_MESSAGE)
        .body(body)
        .send()
        .await
        .map_err(|err| format!("DoH request failed: {}", describe_error(&err)))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("DoH endpoint returned HTTP {status}"));
    }
    // 内容类型不符通常意味着命中了门户页或错误的 URL，此时报文解析会给出
    // 难以理解的错误，先在这里明确指出。
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !content_type.is_empty() && !content_type.starts_with(DNS_MESSAGE) {
        return Err(format!(
            "DoH endpoint returned unexpected content-type `{content_type}`, expected {DNS_MESSAGE}"
        ));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("failed to read DoH response: {err}"))?;
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    let (response_code, records) = decode_response(&bytes)?;

    Ok(DohAnswer {
        endpoint,
        preset,
        proxy: proxy_info,
        response_code,
        records,
        elapsed_ms,
    })
}

/// 展开错误的 source 链。
///
/// `reqwest::Error` 的 Display 只给出「error sending request for url ...」，
/// 对诊断工具来说等于没说；真正的原因（连接被拒、TLS 失败、超时）都在
/// source 链里。
fn describe_error(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        // 相邻层级常常重复同一句话，去掉噪音。
        if !parts.iter().any(|part| part == &text) {
            parts.push(text);
        }
        source = cause.source();
    }
    parts.join(": ")
}

fn build_client(timeout: Duration, proxy: Option<&str>) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("netutils doh");
    match proxy {
        Some(proxy_url) => builder = builder.proxy(reqwest::Proxy::all(proxy_url)?),
        // 未选定代理时必须显式禁用，否则 reqwest 会静默读取环境代理，
        // 让 --no-proxy 失效。
        None => builder = builder.no_proxy(),
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_presets() {
        let (url, preset) = resolve_endpoint("cloudflare").unwrap();

        assert_eq!(url, "https://cloudflare-dns.com/dns-query");
        assert_eq!(preset.as_deref(), Some("cloudflare"));
    }

    #[test]
    fn preset_lookup_is_case_insensitive() {
        assert_eq!(
            resolve_endpoint(" Google ").unwrap().0,
            "https://dns.google/dns-query"
        );
    }

    #[test]
    fn accepts_explicit_https_url() {
        let (url, preset) = resolve_endpoint("https://example.com/dns-query").unwrap();

        assert_eq!(url, "https://example.com/dns-query");
        assert_eq!(preset, None);
    }

    #[test]
    fn rejects_plaintext_endpoint() {
        let err = resolve_endpoint("http://example.com/dns-query").unwrap_err();

        assert!(err.contains("refusing plaintext"), "{err}");
    }

    #[test]
    fn unknown_preset_lists_available_presets() {
        let err = resolve_endpoint("cloudfalre").unwrap_err();

        assert!(err.contains("unknown DoH preset"), "{err}");
        assert!(err.contains("cloudflare"), "{err}");
    }

    #[test]
    fn rejects_empty_endpoint() {
        assert!(resolve_endpoint("   ").is_err());
    }

    #[test]
    fn encodes_query_with_zero_id_for_cacheability() {
        let bytes = encode_query("example.com", RecordType::A).unwrap();
        let decoded = Message::from_vec(&bytes).unwrap();

        assert_eq!(decoded.id(), 0);
        assert_eq!(decoded.message_type(), MessageType::Query);
        assert!(decoded.recursion_desired());
        assert_eq!(decoded.queries().len(), 1);
        assert_eq!(decoded.queries()[0].query_type(), RecordType::A);
        assert_eq!(
            decoded.queries()[0].name().to_utf8().trim_end_matches('.'),
            "example.com"
        );
    }

    #[test]
    fn encodes_aaaa_query_type() {
        let bytes = encode_query("example.com", RecordType::AAAA).unwrap();
        let decoded = Message::from_vec(&bytes).unwrap();

        assert_eq!(decoded.queries()[0].query_type(), RecordType::AAAA);
    }

    #[test]
    fn rejects_invalid_domain() {
        let err = encode_query("not a domain", RecordType::A).unwrap_err();

        assert!(err.contains("invalid domain"), "{err}");
    }

    /// 用编码器造一份应答报文，验证解码路径，无需联网。
    fn sample_response() -> Vec<u8> {
        use std::net::Ipv4Addr;
        use trust_dns_resolver::proto::rr::{rdata::A, RData, Record};

        let mut message = Message::new();
        message
            .set_id(0)
            .set_message_type(MessageType::Response)
            .set_op_code(OpCode::Query);
        let name = Name::from_utf8("example.com").unwrap();
        message.add_query(Query::query(name.clone(), RecordType::A));
        message.add_answer(Record::from_rdata(
            name,
            300,
            RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
        ));
        message.to_vec().unwrap()
    }

    #[test]
    fn decodes_answer_records_with_ttl() {
        let (code, records) = decode_response(&sample_response()).unwrap();

        assert_eq!(code, "No Error");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].value, "93.184.216.34");
        assert_eq!(records[0].ttl, 300);
    }

    #[test]
    fn decoding_garbage_reports_an_error() {
        let err = decode_response(&[0xff, 0x00, 0x13]).unwrap_err();

        assert!(err.contains("failed to decode DNS response"), "{err}");
    }

    #[test]
    fn presets_are_all_https() {
        for (name, url) in PRESETS {
            assert!(url.starts_with("https://"), "{name} must use https");
            // 预设名必须能被 resolve_endpoint 找回，否则帮助文本会撒谎。
            assert_eq!(resolve_endpoint(name).unwrap().0, url);
        }
    }

    #[derive(Debug)]
    struct Layer {
        message: &'static str,
        source: Option<Box<Layer>>,
    }

    impl std::fmt::Display for Layer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.message)
        }
    }

    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_ref()
                .map(|inner| inner.as_ref() as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn describes_the_whole_error_chain() {
        let err = Layer {
            message: "error sending request",
            source: Some(Box::new(Layer {
                message: "tcp connect error",
                source: Some(Box::new(Layer {
                    message: "connection refused",
                    source: None,
                })),
            })),
        };

        assert_eq!(
            describe_error(&err),
            "error sending request: tcp connect error: connection refused"
        );
    }

    #[test]
    fn error_chain_drops_duplicate_layers() {
        let err = Layer {
            message: "timed out",
            source: Some(Box::new(Layer {
                message: "timed out",
                source: None,
            })),
        };

        assert_eq!(describe_error(&err), "timed out");
    }
}
