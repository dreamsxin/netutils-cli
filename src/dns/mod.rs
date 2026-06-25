//! DNS 查询模块：支持 A/AAAA/MX/CNAME/NS/TXT 记录。

use colored::*;
use serde::Serialize;

use crate::i18n::t;
use crate::output::{print_json, print_json_error, OutputMode};
use crate::table::print_table;

use trust_dns_resolver::config::*;
use trust_dns_resolver::proto::rr::{RecordType, RData};
use trust_dns_resolver::TokioAsyncResolver;

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
    fn to_record_type(self) -> RecordType {
        match self {
            DnsRecordType::A => RecordType::A,
            DnsRecordType::Aaaa => RecordType::AAAA,
            DnsRecordType::Mx => RecordType::MX,
            DnsRecordType::Cname => RecordType::CNAME,
            DnsRecordType::Ns => RecordType::NS,
            DnsRecordType::Txt => RecordType::TXT,
        }
    }
}

/// DNS 查询结果
#[derive(Serialize)]
pub struct DnsOutput {
    pub domain: String,
    pub record_type: String,
    pub records: Vec<DnsRecord>,
    pub elapsed_ms: f64,
}

#[derive(Serialize, Clone)]
pub struct DnsRecord {
    pub value: String,
    pub ttl: u32,
}

/// 执行 DNS 查询并输出结果
pub async fn run(domain: &str, record_type: DnsRecordType, mode: OutputMode) {
    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());

    let type_str = match record_type {
        DnsRecordType::A => "A",
        DnsRecordType::Aaaa => "AAAA",
        DnsRecordType::Mx => "MX",
        DnsRecordType::Cname => "CNAME",
        DnsRecordType::Ns => "NS",
        DnsRecordType::Txt => "TXT",
    };

    let start = std::time::Instant::now();
    let result = query_record(&resolver, domain, record_type).await;
    let elapsed = start.elapsed();

    match result {
        Ok(records) => {
            let output = DnsOutput {
                domain: domain.to_string(),
                record_type: type_str.to_string(),
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
            println!("  {}", t("dns.elapsed").replace("{0}", &format!("{:.2}", output.elapsed_ms)));
        }
        Err(e) => {
            let msg = t("dns.fail").replace("{0}", &e);
            if mode == OutputMode::Json {
                print_json_error(&msg);
            } else {
                println!("  {}", msg.red());
            }
        }
    }
}

/// 查询指定类型的 DNS 记录
async fn query_record(
    resolver: &TokioAsyncResolver,
    domain: &str,
    record_type: DnsRecordType,
) -> Result<Vec<DnsRecord>, String> {
    let rt = record_type.to_record_type();
    let lookup = resolver.lookup(domain, rt).await.map_err(|e| e.to_string())?;

    let records: Vec<DnsRecord> = lookup
        .record_iter()
        .filter_map(|r| r.data().map(|d| DnsRecord {
            value: format_record(d),
            ttl: r.ttl(),
        }))
        .collect();

    Ok(records)
}

/// 格式化 DNS 记录为字符串
fn format_record(rdata: &RData) -> String {
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
