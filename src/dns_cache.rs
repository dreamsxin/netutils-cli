//! DNS cache inspection and stale-cache hints.

use std::collections::HashSet;
use std::net::IpAddr;
use std::time::Duration;

use colored::*;
use serde::Serialize;

use crate::output::{print_json, OutputMode};
use crate::table::print_table;

const DNS_CACHE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
pub struct DnsCacheEntry {
    pub name: String,
    pub record_type: String,
    pub ttl: Option<u32>,
    pub data: String,
}

#[derive(Debug, Serialize)]
pub struct DnsCacheReport {
    pub domain: Option<String>,
    pub platform: String,
    pub cache_supported: bool,
    pub cache_source: String,
    pub flush: Option<FlushReport>,
    pub current_ips: Vec<String>,
    pub cached_records: Vec<DnsCacheEntry>,
    pub cached_ips: Vec<String>,
    pub proxy: Option<String>,
    pub egress_interface: Option<String>,
    pub tun_mode: Option<bool>,
    pub assessment: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct FlushReport {
    pub attempted: bool,
    pub success: bool,
    pub message: String,
}

struct CacheLoad {
    supported: bool,
    source: String,
    entries: Vec<DnsCacheEntry>,
    message: Option<String>,
}

pub async fn run(domain: Option<String>, flush: bool, limit: usize, mode: OutputMode) {
    let flush_report = if flush { Some(flush_dns_cache()) } else { None };
    let cache = load_dns_cache();
    let filtered = filter_entries(cache.entries, domain.as_deref(), limit);
    let current_ips = match domain.as_deref() {
        Some(domain) => crate::util::resolve_host_all(domain)
            .await
            .into_iter()
            .map(|ip| ip.to_string())
            .collect(),
        None => Vec::new(),
    };
    let cached_ips = collect_entry_ips(&filtered);
    let proxy = crate::util::get_system_proxy_addr();
    let interfaces = crate::info::collect_interfaces();
    let egress = crate::info::collect_egress(&interfaces);
    let assessment = assess(
        domain.as_deref(),
        cache.supported,
        &current_ips,
        &cached_ips,
    );
    let mut notes = vec![
        "This checks the OS resolver cache. Browsers and proxy clients may keep separate DNS caches."
            .to_string(),
        "When a proxy/TUN mode changes, stale OS or browser DNS entries can keep traffic on the old path."
            .to_string(),
    ];
    if let Some(message) = cache.message {
        notes.push(message);
    }

    let report = DnsCacheReport {
        domain,
        platform: std::env::consts::OS.to_string(),
        cache_supported: cache.supported,
        cache_source: cache.source,
        flush: flush_report,
        current_ips,
        cached_records: filtered,
        cached_ips,
        proxy,
        egress_interface: egress.as_ref().map(|e| e.interface.clone()),
        tun_mode: egress.as_ref().map(|e| e.tun_mode),
        assessment,
        notes,
    };

    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn print_report(report: &DnsCacheReport) {
    println!();
    println!("{}", "🧠 DNS Cache Check".bold());
    if let Some(domain) = &report.domain {
        println!("  Domain: {}", domain);
    }
    println!("  Platform: {}", report.platform);
    println!("  Cache source: {}", report.cache_source);

    if let Some(flush) = &report.flush {
        let symbol = if flush.success {
            "✓".green()
        } else {
            "✗".red()
        };
        println!("  Flush: {} {}", symbol, flush.message);
    }

    println!();
    println!("{}", "Network Context".bold());
    println!(
        "  Proxy: {}",
        report.proxy.as_deref().unwrap_or("not detected")
    );
    println!(
        "  Egress: {}{}",
        report.egress_interface.as_deref().unwrap_or("unknown"),
        report
            .tun_mode
            .map(|tun| format!(" (TUN: {})", if tun { "yes" } else { "no" }))
            .unwrap_or_default()
    );

    if !report.current_ips.is_empty() {
        println!();
        println!("{}", "Current Resolve".bold());
        println!("  {}", report.current_ips.join(", "));
    }

    println!();
    println!("{}", "Cached Records".bold());
    if report.cached_records.is_empty() {
        println!("  <none>");
    } else {
        let rows = report
            .cached_records
            .iter()
            .map(|entry| {
                vec![
                    entry.name.clone(),
                    entry.record_type.clone(),
                    entry
                        .ttl
                        .map(|ttl| ttl.to_string())
                        .unwrap_or_else(|| "--".to_string()),
                    entry.data.clone(),
                ]
            })
            .collect::<Vec<_>>();
        print_table(&["Name", "Type", "TTL", "Data"], &rows);
    }

    println!();
    println!("{}", "Assessment".bold());
    println!("  {}", report.assessment);
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn filter_entries(
    entries: Vec<DnsCacheEntry>,
    domain: Option<&str>,
    limit: usize,
) -> Vec<DnsCacheEntry> {
    let limit = limit.max(1);
    let Some(domain) = domain else {
        return entries.into_iter().take(limit).collect();
    };
    let needle = domain.trim_end_matches('.').to_lowercase();
    entries
        .into_iter()
        .filter(|entry| {
            let name = entry.name.trim_end_matches('.').to_lowercase();
            name == needle || name.ends_with(&format!(".{}", needle))
        })
        .take(limit)
        .collect()
}

fn collect_entry_ips(entries: &[DnsCacheEntry]) -> Vec<String> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .filter_map(|entry| entry.data.parse::<IpAddr>().ok())
        .map(|ip| ip.to_string())
        .filter(|ip| seen.insert(ip.clone()))
        .collect()
}

fn assess(
    domain: Option<&str>,
    cache_supported: bool,
    current_ips: &[String],
    cached_ips: &[String],
) -> String {
    if domain.is_none() {
        return "Pass a domain to compare cached IPs with current DNS resolution.".to_string();
    }
    if !cache_supported {
        return "This platform does not expose a reliable DNS cache dump without extra privileges; compare current DNS and flush if needed.".to_string();
    }
    if cached_ips.is_empty() {
        return "No cached A/AAAA records found for this domain in the OS DNS cache.".to_string();
    }
    if current_ips.is_empty() {
        return "Current DNS resolution failed, but cached records exist; a stale cache or resolver/proxy DNS issue is possible.".to_string();
    }
    let current = current_ips.iter().collect::<HashSet<_>>();
    let stale = cached_ips
        .iter()
        .filter(|ip| !current.contains(ip))
        .cloned()
        .collect::<Vec<_>>();
    if stale.is_empty() {
        "Cached IPs overlap with current DNS resolution; OS DNS cache is unlikely to be the immediate cause.".to_string()
    } else {
        format!(
            "Possible stale DNS cache: cached IPs not in current resolution: {}",
            stale.join(", ")
        )
    }
}

fn load_dns_cache() -> CacheLoad {
    #[cfg(target_os = "windows")]
    {
        let output = crate::util::command_output_timeout(
            "ipconfig",
            &["/displaydns"],
            DNS_CACHE_COMMAND_TIMEOUT,
        );
        match output {
            Some(output) => CacheLoad {
                supported: true,
                source: "ipconfig /displaydns".to_string(),
                entries: parse_windows_displaydns(&String::from_utf8_lossy(&output.stdout)),
                message: None,
            },
            None => CacheLoad {
                supported: true,
                source: "ipconfig /displaydns".to_string(),
                entries: Vec::new(),
                message: Some("Failed to read DNS cache or command timed out.".to_string()),
            },
        }
    }

    #[cfg(target_os = "macos")]
    {
        let output = crate::util::command_output_timeout(
            "dscacheutil",
            &["-cachedump", "-entries", "Host"],
            DNS_CACHE_COMMAND_TIMEOUT,
        );
        match output {
            Some(output) => CacheLoad {
                supported: true,
                source: "dscacheutil -cachedump -entries Host".to_string(),
                entries: parse_macos_cachedump(&String::from_utf8_lossy(&output.stdout)),
                message: Some("macOS may restrict DNS cache dumps; an empty result does not always mean there is no cache.".to_string()),
            },
            None => CacheLoad {
                supported: false,
                source: "macOS DNS cache".to_string(),
                entries: Vec::new(),
                message: Some("macOS does not expose a reliable DNS cache dump without privileges on many versions.".to_string()),
            },
        }
    }

    #[cfg(target_os = "linux")]
    {
        CacheLoad {
            supported: false,
            source: "Linux resolver cache varies by service".to_string(),
            entries: Vec::new(),
            message: Some("Linux DNS cache is service-specific (systemd-resolved, dnsmasq, nscd, browser cache). Use --flush to try resolvectl/systemd-resolve flush.".to_string()),
        }
    }
}

#[cfg(target_os = "windows")]
fn parse_windows_displaydns(text: &str) -> Vec<DnsCacheEntry> {
    let mut entries = Vec::new();
    let mut current = DnsCacheEntry {
        name: String::new(),
        record_type: String::new(),
        ttl: None,
        data: String::new(),
    };

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            push_entry(&mut entries, &mut current);
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key_norm = key
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        let value = value.trim();

        if key_norm == "recordname" {
            push_entry(&mut entries, &mut current);
            current.name = value.to_string();
        } else if key_norm == "recordtype" {
            current.record_type = map_record_type(value).to_string();
        } else if key_norm == "timetolive" {
            current.ttl = value.parse::<u32>().ok();
        } else if key_norm.ends_with("record")
            && key_norm != "record"
            && key_norm != "recordname"
            && key_norm != "recordtype"
        {
            current.data = value.to_string();
            if current.record_type.is_empty() {
                current.record_type = infer_record_type(&key_norm).to_string();
            }
        }
    }
    push_entry(&mut entries, &mut current);

    entries
}

#[cfg(target_os = "windows")]
fn push_entry(entries: &mut Vec<DnsCacheEntry>, current: &mut DnsCacheEntry) {
    if !current.name.is_empty() && (!current.record_type.is_empty() || !current.data.is_empty()) {
        entries.push(current.clone());
    }
    current.name.clear();
    current.record_type.clear();
    current.ttl = None;
    current.data.clear();
}

#[cfg(target_os = "windows")]
fn map_record_type(value: &str) -> &str {
    match value.trim() {
        "1" => "A",
        "5" => "CNAME",
        "12" => "PTR",
        "28" => "AAAA",
        other => other,
    }
}

#[cfg(target_os = "windows")]
fn infer_record_type(key: &str) -> &str {
    if key.contains("aaaa") {
        "AAAA"
    } else if key.contains("cname") {
        "CNAME"
    } else if key.contains("ptr") {
        "PTR"
    } else if key.contains("host") || key.starts_with('a') {
        "A"
    } else {
        "unknown"
    }
}

#[cfg(target_os = "macos")]
fn parse_macos_cachedump(text: &str) -> Vec<DnsCacheEntry> {
    let mut entries = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || !line.contains("name:") {
            continue;
        }
        let name = line
            .split("name:")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("")
            .to_string();
        if !name.is_empty() {
            entries.push(DnsCacheEntry {
                name,
                record_type: "unknown".to_string(),
                ttl: None,
                data: line.to_string(),
            });
        }
    }
    entries
}

fn flush_dns_cache() -> FlushReport {
    #[cfg(target_os = "windows")]
    {
        let output = crate::util::command_output_timeout(
            "ipconfig",
            &["/flushdns"],
            DNS_CACHE_COMMAND_TIMEOUT,
        );
        return FlushReport {
            attempted: true,
            success: output.as_ref().map(|o| o.status.success()).unwrap_or(false),
            message: output
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "ipconfig /flushdns failed or timed out".to_string()),
        };
    }

    #[cfg(target_os = "macos")]
    {
        let first = crate::util::command_output_timeout(
            "dscacheutil",
            &["-flushcache"],
            DNS_CACHE_COMMAND_TIMEOUT,
        );
        let second = crate::util::command_output_timeout(
            "killall",
            &["-HUP", "mDNSResponder"],
            DNS_CACHE_COMMAND_TIMEOUT,
        );
        return FlushReport {
            attempted: true,
            success: first.as_ref().map(|o| o.status.success()).unwrap_or(false),
            message: format!(
                "dscacheutil: {}; mDNSResponder signal: {}",
                status_word(first.as_ref()),
                status_word(second.as_ref())
            ),
        };
    }

    #[cfg(target_os = "linux")]
    {
        for (program, args) in [
            ("resolvectl", &["flush-caches"][..]),
            ("systemd-resolve", &["--flush-caches"][..]),
        ] {
            if let Some(output) =
                crate::util::command_output_timeout(program, args, DNS_CACHE_COMMAND_TIMEOUT)
            {
                if output.status.success() {
                    return FlushReport {
                        attempted: true,
                        success: true,
                        message: format!("{} {}", program, args.join(" ")),
                    };
                }
            }
        }
        FlushReport {
            attempted: true,
            success: false,
            message: "No supported Linux DNS cache flush command succeeded.".to_string(),
        }
    }
}

#[cfg(target_os = "macos")]
fn status_word(output: Option<&std::process::Output>) -> &'static str {
    match output {
        Some(output) if output.status.success() => "ok",
        Some(_) => "failed",
        None => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_windows_displaydns_records() {
        let text = r#"
Windows IP Configuration

    example.com
    ----------------------------------------
    Record Name . . . . . : example.com
    Record Type . . . . . : 1
    Time To Live  . . . . : 42
    Data Length . . . . . : 4
    Section . . . . . . . : Answer
    A (Host) Record . . . : 93.184.216.34

    Record Name . . . . . : alias.example.com
    Record Type . . . . . : 5
    Time To Live  . . . . : 40
    CNAME Record  . . . . : example.com
"#;

        let parsed = parse_windows_displaydns(text);

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "example.com");
        assert_eq!(parsed[0].record_type, "A");
        assert_eq!(parsed[0].ttl, Some(42));
        assert_eq!(parsed[0].data, "93.184.216.34");
        assert_eq!(parsed[1].record_type, "CNAME");
    }
}
