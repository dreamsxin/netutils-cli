//! DNS 查询模块：支持 A/AAAA/MX/CNAME/NS/TXT 记录。

use colored::*;
use serde::Serialize;

use crate::i18n::t;
use crate::output::{print_json, print_json_error, OutputMode};
use crate::table::print_table;

use trust_dns_resolver::proto::rr::{RData, RecordType};
use trust_dns_resolver::TokioAsyncResolver;

const DNS_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// DNS 记录类型
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum DnsRecordType {
    A,
    Aaaa,
    Mx,
    Cname,
    Ns,
    Txt,
}

impl DnsRecordType {
    pub(crate) fn to_record_type(self) -> RecordType {
        match self {
            DnsRecordType::A => RecordType::A,
            DnsRecordType::Aaaa => RecordType::AAAA,
            DnsRecordType::Mx => RecordType::MX,
            DnsRecordType::Cname => RecordType::CNAME,
            DnsRecordType::Ns => RecordType::NS,
            DnsRecordType::Txt => RecordType::TXT,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            DnsRecordType::A => "A",
            DnsRecordType::Aaaa => "AAAA",
            DnsRecordType::Mx => "MX",
            DnsRecordType::Cname => "CNAME",
            DnsRecordType::Ns => "NS",
            DnsRecordType::Txt => "TXT",
        }
    }
}

/// DNS 查询结果
#[derive(Serialize)]
pub struct DnsOutput {
    pub domain: String,
    pub record_type: String,
    pub resolver: String,
    pub records: Vec<DnsRecord>,
    pub elapsed_ms: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct DnsRecord {
    pub value: String,
    pub ttl: u32,
}

/// DoH 查询结果。`transport` 固定为 `doh`，便于脚本区分链路。
#[derive(Serialize)]
struct DohOutput {
    domain: String,
    record_type: String,
    transport: &'static str,
    doh: crate::doh::DohAnswer,
    notes: Vec<String>,
}

/// DoT 查询结果。`transport` 固定为 `dot`，便于脚本区分链路。
#[derive(Serialize)]
struct DotOutput {
    domain: String,
    record_type: String,
    transport: &'static str,
    dot: crate::dot::DotAnswer,
    notes: Vec<String>,
}

/// 执行 DNS 查询并输出结果
#[allow(clippy::too_many_arguments)]
pub async fn run(
    domain: &str,
    record_type: DnsRecordType,
    server: Option<String>,
    doh: Option<String>,
    dot: Option<String>,
    proxy: Option<String>,
    no_proxy: bool,
    mode: OutputMode,
) {
    // 三种传输互斥由 clap 保证（conflicts_with），这里只负责分派。
    if let Some(target) = dot {
        // clap 的 `--proxy requires doh` 在 `--dot` 同时出现时会被冲突规则抵消，
        // 于是 `--dot X --proxy Y` 能通过解析并**静默丢弃** --proxy。
        // 对诊断工具来说静默忽略用户显式给出的代理不可接受，这里显式拦下。
        if proxy.is_some() || no_proxy {
            usage_error(
                "--proxy and --no-proxy only apply to --doh; DoT is raw TLS on port 853 \
                 and does not go through a proxy",
                mode,
            );
            return;
        }
        run_dot(domain, record_type, &target, mode).await;
        return;
    }
    if let Some(endpoint) = doh {
        run_doh(domain, record_type, &endpoint, proxy, no_proxy, mode).await;
        return;
    }

    let resolver = match build_resolver(server.as_deref()) {
        Ok(resolver) => resolver,
        Err(err) => {
            fail(&err, mode);
            return;
        }
    };

    let type_str = record_type.as_str();

    let start = std::time::Instant::now();
    let result = query_record(&resolver, domain, record_type).await;
    let elapsed = start.elapsed();

    match result {
        Ok(records) => {
            let output = DnsOutput {
                domain: domain.to_string(),
                record_type: type_str.to_string(),
                resolver: server.unwrap_or_else(|| "system".to_string()),
                elapsed_ms: elapsed.as_secs_f64() * 1000.0,
                records: records.clone(),
            };

            if mode == OutputMode::Json {
                print_json(&output);
                return;
            }

            // 表格输出
            println!();
            println!(
                "{}",
                t("dns.title")
                    .replace("{0}", domain)
                    .replace("{1}", type_str)
                    .bold()
            );

            if records.is_empty() {
                println!("  {}", t("dns.no_record").replace("{0}", type_str));
            } else {
                let h_idx = t("dns.idx");
                let h_val = t("dns.value");
                let h_ttl = t("dns.ttl");
                let headers = [h_idx.as_str(), h_val.as_str(), h_ttl.as_str()];
                let rows: Vec<Vec<String>> = records
                    .iter()
                    .enumerate()
                    .map(|(i, r)| vec![(i + 1).to_string(), r.value.clone(), format!("{}s", r.ttl)])
                    .collect();
                print_table(&headers, &rows);
            }

            println!();
            println!(
                "  {}",
                t("dns.elapsed").replace("{0}", &format!("{:.2}", output.elapsed_ms))
            );
        }
        Err(e) => {
            let msg = t("dns.fail").replace("{0}", &e);
            crate::output::mark_failure();
            if mode == OutputMode::Json {
                print_json_error(&msg);
            } else {
                println!("  {}", msg.red());
            }
        }
    }
}

fn fail(message: &str, mode: OutputMode) {
    crate::output::mark_failure();
    if mode == OutputMode::Json {
        print_json_error(message);
    } else {
        println!("  {}", message.red());
    }
}

/// 参数用法错误，退出码 2，与「探测失败」(1) 区分。
fn usage_error(message: &str, mode: OutputMode) {
    crate::output::mark_exit_code(2);
    if mode == OutputMode::Json {
        print_json_error(message);
    } else {
        eprintln!("{message}");
    }
}

/// 通过 DoH 查询并输出。
///
/// DoH 走 HTTPS，因此与系统 DNS 服务器列表完全无关——这正是它能绕过
/// VPN/TUN split-DNS 的原因，也是 `dns-path`/`dns-leak` 里标注的盲区。
async fn run_doh(
    domain: &str,
    record_type: DnsRecordType,
    endpoint: &str,
    proxy: Option<String>,
    no_proxy: bool,
    mode: OutputMode,
) {
    let answer = crate::doh::query(
        endpoint,
        domain,
        record_type.to_record_type(),
        DNS_QUERY_TIMEOUT,
        proxy,
        no_proxy,
    )
    .await;

    let answer = match answer {
        Ok(answer) => answer,
        Err(err) => {
            fail(&t("dns.fail").replace("{0}", &err), mode);
            return;
        }
    };

    let type_str = record_type.as_str();
    let output = DohOutput {
        domain: domain.to_string(),
        record_type: type_str.to_string(),
        transport: "doh",
        doh: answer,
        notes: doh_notes(),
    };

    if output.doh.records.is_empty() {
        crate::output::mark_failure();
    }

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    println!();
    println!(
        "{}",
        t("dns.title")
            .replace("{0}", domain)
            .replace("{1}", type_str)
            .bold()
    );
    println!(
        "  Resolver: {}{} (DoH)",
        output.doh.endpoint,
        output
            .doh
            .preset
            .as_ref()
            .map(|preset| format!(" [{preset}]"))
            .unwrap_or_default()
    );
    println!(
        "  Proxy: {}{}",
        output.doh.proxy.mode,
        output
            .doh
            .proxy
            .value
            .as_ref()
            .map(|value| format!(" ({value})"))
            .unwrap_or_default()
    );
    println!("  Response Code: {}", output.doh.response_code);

    if output.doh.records.is_empty() {
        println!("  {}", t("dns.no_record").replace("{0}", type_str));
    } else {
        let h_idx = t("dns.idx");
        let h_val = t("dns.value");
        let h_ttl = t("dns.ttl");
        let headers = [h_idx.as_str(), h_val.as_str(), h_ttl.as_str()];
        let rows: Vec<Vec<String>> = output
            .doh
            .records
            .iter()
            .enumerate()
            .map(|(i, r)| vec![(i + 1).to_string(), r.value.clone(), format!("{}s", r.ttl)])
            .collect();
        print_table(&headers, &rows);
    }

    println!();
    println!(
        "  {}",
        t("dns.elapsed").replace("{0}", &format!("{:.2}", output.doh.elapsed_ms))
    );
    println!();
    for note in &output.notes {
        println!("  {}", note.dimmed());
    }
}

fn doh_notes() -> Vec<String> {
    vec![
        "DoH bypasses the operating-system resolver entirely, so hosts file, \
         VPN split-DNS, and the system DNS cache do not apply."
            .to_string(),
        "Compare with `netutils dns` and `netutils dns-compare` to see whether \
         the DoH answer differs from the system resolution path."
            .to_string(),
    ]
}

/// 通过 DoT 查询并输出。
///
/// DoT 与 DoH 一样绕过系统 resolver，但它是 853 端口上的裸 TLS 流，
/// 不经过 HTTP 客户端，因此不支持代理。
async fn run_dot(domain: &str, record_type: DnsRecordType, target: &str, mode: OutputMode) {
    let answer = crate::dot::query(
        target,
        domain,
        record_type.to_record_type(),
        DNS_QUERY_TIMEOUT,
    )
    .await;

    let answer = match answer {
        Ok(answer) => answer,
        Err(err) => {
            fail(&t("dns.fail").replace("{0}", &err), mode);
            return;
        }
    };

    let type_str = record_type.as_str();
    let output = DotOutput {
        domain: domain.to_string(),
        record_type: type_str.to_string(),
        transport: "dot",
        dot: answer,
        notes: dot_notes(),
    };

    if output.dot.records.is_empty() {
        crate::output::mark_failure();
    }

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    println!();
    println!(
        "{}",
        t("dns.title")
            .replace("{0}", domain)
            .replace("{1}", type_str)
            .bold()
    );
    println!(
        "  Resolver: {}{} (DoT)",
        output.dot.endpoint,
        output
            .dot
            .preset
            .as_ref()
            .map(|preset| format!(" [{preset}]"))
            .unwrap_or_default()
    );
    println!("  Server IP: {}", output.dot.server_ip);
    println!(
        "  TLS: {}",
        output.dot.tls_version.as_deref().unwrap_or("unknown")
    );
    println!("  Response Code: {}", output.dot.response_code);

    if output.dot.records.is_empty() {
        println!("  {}", t("dns.no_record").replace("{0}", type_str));
    } else {
        let h_idx = t("dns.idx");
        let h_val = t("dns.value");
        let h_ttl = t("dns.ttl");
        let headers = [h_idx.as_str(), h_val.as_str(), h_ttl.as_str()];
        let rows: Vec<Vec<String>> = output
            .dot
            .records
            .iter()
            .enumerate()
            .map(|(i, r)| vec![(i + 1).to_string(), r.value.clone(), format!("{}s", r.ttl)])
            .collect();
        print_table(&headers, &rows);
    }

    println!();
    println!(
        "  {}",
        t("dns.elapsed").replace("{0}", &format!("{:.2}", output.dot.elapsed_ms))
    );
    println!();
    for note in &output.notes {
        println!("  {}", note.dimmed());
    }
}

fn dot_notes() -> Vec<String> {
    vec![
        "DoT bypasses the operating-system resolver entirely, so hosts file, \
         VPN split-DNS, and the system DNS cache do not apply."
            .to_string(),
        "DoT is raw TLS on port 853, not HTTP, so proxies do not apply; \
         use --doh when you need to observe proxy-side DNS behavior."
            .to_string(),
        "A hostname is required because the server certificate is validated; \
         the reported TLS version confirms the query was actually encrypted."
            .to_string(),
    ]
}

/// 构建 DNS resolver，支持自定义服务器
fn build_resolver(server: Option<&str>) -> Result<TokioAsyncResolver, String> {
    match server {
        Some(addr) => {
            use std::net::SocketAddr;
            use std::str::FromStr;
            use trust_dns_resolver::config::*;

            // 解析服务器地址，默认端口 53
            let socket_addr = if let Ok(ip) = addr.parse::<std::net::IpAddr>() {
                SocketAddr::new(ip, 53)
            } else {
                SocketAddr::from_str(addr)
                    .map_err(|err| format!("invalid DNS server `{addr}`: {err}"))?
            };

            let name_server = NameServerConfig {
                socket_addr,
                protocol: trust_dns_resolver::config::Protocol::Udp,
                tls_dns_name: None,
                trust_negative_responses: false,
                bind_addr: None,
            };

            let config = ResolverConfig::from_parts(None, vec![], vec![name_server]);
            Ok(TokioAsyncResolver::tokio(config, ResolverOpts::default()))
        }
        None => TokioAsyncResolver::tokio_from_system_conf()
            .map_err(|err| format!("failed to load system DNS configuration: {err}")),
    }
}

/// 查询指定类型的 DNS 记录
async fn query_record(
    resolver: &TokioAsyncResolver,
    domain: &str,
    record_type: DnsRecordType,
) -> Result<Vec<DnsRecord>, String> {
    let rt = record_type.to_record_type();
    let lookup = tokio::time::timeout(DNS_QUERY_TIMEOUT, resolver.lookup(domain, rt))
        .await
        .map_err(|_| "timeout".to_string())?
        .map_err(|e| e.to_string())?;

    let records: Vec<DnsRecord> = lookup
        .record_iter()
        .filter_map(|r| {
            r.data().map(|d| DnsRecord {
                value: format_record(d),
                ttl: r.ttl(),
            })
        })
        .collect();

    Ok(records)
}

/// 格式化 DNS 记录为字符串
pub(crate) fn format_record(rdata: &RData) -> String {
    match rdata {
        RData::A(addr) => addr.0.to_string(),
        RData::AAAA(addr) => addr.0.to_string(),
        RData::MX(mx) => format!("{} {}", mx.preference(), mx.exchange()),
        RData::CNAME(cname) => cname.0.to_string(),
        RData::NS(ns) => ns.0.to_string(),
        RData::TXT(txt) => {
            let data: Vec<String> = txt
                .txt_data()
                .iter()
                .map(|d| String::from_utf8_lossy(d).to_string())
                .collect();
            data.join(" ")
        }
        other => format!("{:?}", other),
    }
}
