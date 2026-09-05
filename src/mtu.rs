//! 路径 MTU 发现与 PMTUD 黑洞检测。
//!
//! VPN/TUN/隧道场景下最常见的疑难故障之一：小包（DNS、TCP 握手）正常，
//! 大包（TLS 证书、文件下载）静默丢失。根因通常是隧道降低了路径 MTU，
//! 而中间设备又丢弃了 ICMP "fragmentation needed"，导致 PMTUD 失效。
//!
//! 实现方式：复用系统 `ping` 的 DF（Don't Fragment）能力做二分查找。
//! 相比自建原始套接字，这样无需 setsockopt 的三套平台分支，也无需提权，
//! 与项目既有「外部命令 + 超时」的做法一致。各平台输出解析是纯函数，
//! 因此可以在任意平台上被单元测试覆盖。

use std::net::IpAddr;
use std::time::Duration;

use colored::*;
use serde::Serialize;

use crate::output::{print_json, print_json_error, OutputMode};
use crate::table::print_table;

/// IPv4 头 20 字节 + ICMP 头 8 字节
const IPV4_ICMP_OVERHEAD: u32 = 28;
/// IPv6 头 40 字节 + ICMPv6 头 8 字节
const IPV6_ICMP_OVERHEAD: u32 = 48;
/// RFC 791 要求的 IPv4 最小重组缓冲区
const MIN_IPV4_MTU: u32 = 576;
/// RFC 8200 要求的 IPv6 最小链路 MTU
const MIN_IPV6_MTU: u32 = 1280;
/// 以太网标准 MTU
const DEFAULT_MAX_MTU: u32 = 1500;

#[derive(Debug, Serialize)]
pub struct MtuReport {
    pub target: String,
    pub resolved_ip: Option<String>,
    pub family: &'static str,
    /// 出口接口名与其本地 MTU（探测失败时为 None）
    pub local: LocalMtu,
    pub probe: ProbeSummary,
    pub verdict: Verdict,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LocalMtu {
    pub interface: Option<String>,
    pub mtu: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct ProbeSummary {
    /// 探测方式，便于用户判断结果可信度
    pub method: &'static str,
    pub searched_min_mtu: u32,
    pub searched_max_mtu: u32,
    /// 收敛得到的路径 MTU
    pub path_mtu: Option<u32>,
    /// 路径设备主动通告的 MTU（ICMP frag-needed 中携带），最可信
    pub advertised_mtu: Option<u32>,
    pub steps: Vec<ProbeStep>,
}

#[derive(Debug, Serialize)]
pub struct ProbeStep {
    pub mtu: u32,
    pub payload_bytes: u32,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Verdict {
    pub status: &'static str,
    /// PMTUD 黑洞：大包被静默丢弃而没有 ICMP 通告
    pub blackhole: bool,
    pub confidence: &'static str,
    pub summary: String,
}

/// 单次 DF 探测的判定结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 收到回复，该尺寸可通过
    Ok,
    /// 明确收到「需要分片」，PMTUD 正常工作；可能携带通告 MTU
    TooBig { advertised_mtu: Option<u32> },
    /// 无回复，静默丢弃
    Timeout,
    /// 目标不可达或本地错误（与尺寸无关）
    Unreachable(String),
}

/// 目标平台的 ping 方言
///
/// 三种方言在所有平台上都保留：解析逻辑是纯函数，需要在任意平台上被单元测试覆盖。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PingDialect {
    Windows,
    Linux,
    MacOs,
}

impl PingDialect {
    fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            PingDialect::Windows
        }
        #[cfg(target_os = "macos")]
        {
            PingDialect::MacOs
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            PingDialect::Linux
        }
    }
}

pub async fn run(
    target: &str,
    min_mtu: Option<u32>,
    max_mtu: Option<u32>,
    timeout: Duration,
    mode: OutputMode,
) {
    let Some(ip) = crate::util::resolve_host(target).await else {
        fail(
            mode,
            &format!("cannot resolve target host: {target}"),
            target,
        );
        return;
    };

    let is_v6 = ip.is_ipv6();
    let local = detect_local_mtu().await;
    let floor = min_mtu.unwrap_or(if is_v6 { MIN_IPV6_MTU } else { MIN_IPV4_MTU });
    let ceiling = max_mtu.or(local.mtu).unwrap_or(DEFAULT_MAX_MTU).max(floor);

    if floor < minimum_allowed(is_v6) {
        fail(
            mode,
            &format!(
                "--min-mtu must be at least {} for this address family",
                minimum_allowed(is_v6)
            ),
            target,
        );
        return;
    }

    let probe = binary_search(ip, floor, ceiling, timeout).await;
    let verdict = judge(&probe, &local, floor, ceiling);

    let report = MtuReport {
        target: target.to_string(),
        resolved_ip: Some(ip.to_string()),
        family: if is_v6 { "ipv6" } else { "ipv4" },
        local,
        probe,
        verdict,
        notes: notes(),
    };

    if !report.verdict.blackhole && report.probe.path_mtu.is_none() {
        crate::output::mark_failure();
    }
    if report.verdict.blackhole {
        crate::output::mark_failure();
    }

    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn minimum_allowed(is_v6: bool) -> u32 {
    if is_v6 {
        MIN_IPV6_MTU
    } else {
        68
    }
}

fn overhead(is_v6: bool) -> u32 {
    if is_v6 {
        IPV6_ICMP_OVERHEAD
    } else {
        IPV4_ICMP_OVERHEAD
    }
}

/// 在 [floor, ceiling] 上二分查找最大可通过的 MTU。
async fn binary_search(ip: IpAddr, floor: u32, ceiling: u32, timeout: Duration) -> ProbeSummary {
    let dialect = PingDialect::current();
    let is_v6 = ip.is_ipv6();
    let mut steps = Vec::new();
    let mut advertised_mtu = None;

    // 先用下界确认目标在 DF 模式下可达，否则后续的「失败」无法归因到尺寸。
    let baseline = probe_once(dialect, ip, floor, is_v6, timeout).await;
    record(&mut steps, floor, is_v6, &baseline);
    collect_advertised(&mut advertised_mtu, &baseline);
    if !matches!(baseline, ProbeOutcome::Ok) {
        return ProbeSummary {
            method: method_name(),
            searched_min_mtu: floor,
            searched_max_mtu: ceiling,
            path_mtu: None,
            advertised_mtu,
            steps,
        };
    }

    let mut low = floor; // 已确认可通过
    let mut high = ceiling; // 尚未确认

    // 上界先试一次：多数网络在此直接收敛，省下全部二分步骤。
    if ceiling > floor {
        let top = probe_once(dialect, ip, ceiling, is_v6, timeout).await;
        record(&mut steps, ceiling, is_v6, &top);
        collect_advertised(&mut advertised_mtu, &top);
        if matches!(top, ProbeOutcome::Ok) {
            return ProbeSummary {
                method: method_name(),
                searched_min_mtu: floor,
                searched_max_mtu: ceiling,
                path_mtu: Some(ceiling),
                advertised_mtu,
                steps,
            };
        }
        high = ceiling - 1;
    }

    // 路径设备通告了确切 MTU 时直接验证该值，避免十来次无谓探测。
    if let Some(hint) = advertised_mtu.filter(|value| *value > low && *value <= high) {
        let probe = probe_once(dialect, ip, hint, is_v6, timeout).await;
        record(&mut steps, hint, is_v6, &probe);
        if matches!(probe, ProbeOutcome::Ok) {
            low = hint;
        } else {
            high = hint - 1;
        }
    }

    while low < high {
        // 向上取整，确保 low/high 相邻时仍能推进，不会死循环。
        let mid = low + (high - low).div_ceil(2);
        let probe = probe_once(dialect, ip, mid, is_v6, timeout).await;
        record(&mut steps, mid, is_v6, &probe);
        collect_advertised(&mut advertised_mtu, &probe);
        match probe {
            ProbeOutcome::Ok => low = mid,
            ProbeOutcome::TooBig { .. } | ProbeOutcome::Timeout => high = mid - 1,
            // 与尺寸无关的错误无法二分，停止收敛并保留已知下界。
            ProbeOutcome::Unreachable(_) => break,
        }
    }

    ProbeSummary {
        method: method_name(),
        searched_min_mtu: floor,
        searched_max_mtu: ceiling,
        path_mtu: Some(low),
        advertised_mtu,
        steps,
    }
}

fn method_name() -> &'static str {
    "system-ping-df-binary-search"
}

fn record(steps: &mut Vec<ProbeStep>, mtu: u32, is_v6: bool, outcome: &ProbeOutcome) {
    let (result, detail) = match outcome {
        ProbeOutcome::Ok => ("ok", None),
        ProbeOutcome::TooBig { advertised_mtu } => (
            "too-big",
            advertised_mtu.map(|value| format!("router advertised mtu {value}")),
        ),
        ProbeOutcome::Timeout => ("timeout", None),
        ProbeOutcome::Unreachable(reason) => ("error", Some(reason.clone())),
    };
    steps.push(ProbeStep {
        mtu,
        payload_bytes: mtu.saturating_sub(overhead(is_v6)),
        result: result.to_string(),
        detail,
    });
}

fn collect_advertised(slot: &mut Option<u32>, outcome: &ProbeOutcome) {
    if let ProbeOutcome::TooBig {
        advertised_mtu: Some(value),
    } = outcome
    {
        // 保留最小的通告值：路径上最窄的那一跳才是瓶颈。
        *slot = Some(slot.map_or(*value, |current| current.min(*value)));
    }
}

async fn probe_once(
    dialect: PingDialect,
    ip: IpAddr,
    mtu: u32,
    is_v6: bool,
    timeout: Duration,
) -> ProbeOutcome {
    let payload = mtu.saturating_sub(overhead(is_v6));
    let ip_text = ip.to_string();
    let payload_text = payload.to_string();
    let timeout_text = ping_timeout_arg(dialect, timeout);
    let args = ping_args(dialect, &ip_text, &payload_text, &timeout_text);
    let program = ping_program(dialect, is_v6);

    // 外层留出余量，避免系统 ping 自身卡死拖住整条命令。
    let hard_timeout = timeout + Duration::from_secs(2);
    let output = tokio::task::spawn_blocking(move || {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        crate::util::command_output_timeout(program, &arg_refs, hard_timeout)
    })
    .await
    .ok()
    .flatten();

    let Some(output) = output else {
        return ProbeOutcome::Unreachable("failed to run system ping".to_string());
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    classify(dialect, &stdout, &stderr, output.status.success())
}

fn ping_program(dialect: PingDialect, is_v6: bool) -> &'static str {
    match (dialect, is_v6) {
        // Windows 与 macOS 的 ping 通过参数区分协议族；Linux 传统上分为两个程序。
        (PingDialect::Linux, true) => "ping6",
        _ => "ping",
    }
}

fn ping_timeout_arg(dialect: PingDialect, timeout: Duration) -> String {
    match dialect {
        // Windows 以毫秒为单位，Linux/macOS 以秒为单位。
        PingDialect::Windows => timeout.as_millis().max(1).to_string(),
        _ => timeout.as_secs().max(1).to_string(),
    }
}

fn ping_args(dialect: PingDialect, ip: &str, payload: &str, timeout_value: &str) -> Vec<String> {
    let owned = |values: &[&str]| values.iter().map(|v| (*v).to_string()).collect();
    match dialect {
        // -f: 设置 DF 位；-l: 负载字节数；-w: 单次等待毫秒
        PingDialect::Windows => owned(&["-n", "1", "-f", "-l", payload, "-w", timeout_value, ip]),
        // -M do: 禁止分片；-s: 负载字节数；-W: 等待秒数
        PingDialect::Linux => owned(&[
            "-c",
            "1",
            "-M",
            "do",
            "-s",
            payload,
            "-W",
            timeout_value,
            ip,
        ]),
        // -D: 禁止分片；-t: 整体超时秒数
        PingDialect::MacOs => owned(&["-c", "1", "-D", "-s", payload, "-t", timeout_value, ip]),
    }
}

/// 解析系统 ping 输出。纯函数，便于跨平台单元测试。
pub fn classify(dialect: PingDialect, stdout: &str, stderr: &str, exit_ok: bool) -> ProbeOutcome {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();

    // 「需要分片」的判定必须先于成功判定：部分实现会同时打印统计行。
    if is_too_big(&combined) {
        return ProbeOutcome::TooBig {
            advertised_mtu: parse_advertised_mtu(&combined),
        };
    }
    if is_unreachable(&combined) {
        return ProbeOutcome::Unreachable(first_meaningful_line(stderr, stdout));
    }
    if is_timeout(&combined) {
        return ProbeOutcome::Timeout;
    }
    // 先看正向证据，再退回退出码：Windows ping 对超时也可能返回 0。
    if is_reply(dialect, &combined) {
        return ProbeOutcome::Ok;
    }
    if exit_ok {
        return ProbeOutcome::Ok;
    }
    ProbeOutcome::Timeout
}

fn is_too_big(text: &str) -> bool {
    const MARKERS: [&str; 7] = [
        "needs to be fragmented", // Windows
        "message too long",       // Linux (local error)
        "message too big",        // macOS
        "frag needed",            // Linux/macOS ICMP report
        "fragmentation needed",   // 通用 ICMP 文案
        "packet too big",         // ICMPv6
        "would require fragmentation",
    ];
    MARKERS.iter().any(|marker| text.contains(marker))
}

fn is_timeout(text: &str) -> bool {
    const MARKERS: [&str; 5] = [
        "request timed out",
        "100% packet loss",
        "0 received",
        "0 packets received",
        "no answer",
    ];
    MARKERS.iter().any(|marker| text.contains(marker))
}

fn is_unreachable(text: &str) -> bool {
    const MARKERS: [&str; 6] = [
        "unknown host",
        "name or service not known",
        "network is unreachable",
        "general failure",
        "cannot resolve",
        "operation not permitted",
    ];
    MARKERS.iter().any(|marker| text.contains(marker))
}

fn is_reply(dialect: PingDialect, text: &str) -> bool {
    match dialect {
        PingDialect::Windows => text.contains("reply from") || text.contains("bytes="),
        _ => text.contains("bytes from") || text.contains("1 received"),
    }
}

/// 从 ICMP frag-needed 文案中提取通告 MTU，例如 `mtu = 1400` / `MTU 1400`。
fn parse_advertised_mtu(text: &str) -> Option<u32> {
    let idx = text.find("mtu")?;
    let rest = &text[idx + 3..];
    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    // 跳过 "mtu" 与数字之间可能出现的 `=`、`:`、空格；若数字距离过远则视为无关文本。
    let gap = rest
        .chars()
        .take_while(|c| !c.is_ascii_digit())
        .filter(|c| !matches!(c, ' ' | '=' | ':' | '(' | '\t'))
        .count();
    if gap > 0 || digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn first_meaningful_line(stderr: &str, stdout: &str) -> String {
    stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown error")
        .to_string()
}

fn judge(probe: &ProbeSummary, local: &LocalMtu, floor: u32, ceiling: u32) -> Verdict {
    let saw_too_big = probe.steps.iter().any(|step| step.result == "too-big");
    let saw_timeout = probe.steps.iter().any(|step| step.result == "timeout");
    let baseline_failed = probe
        .steps
        .first()
        .map(|step| step.result != "ok")
        .unwrap_or(true);

    if baseline_failed {
        return Verdict {
            status: "inconclusive",
            blackhole: false,
            confidence: "low",
            summary: format!(
                "target did not answer DF ping at the {floor}-byte floor; \
                 ICMP may be filtered end to end, so path MTU cannot be measured this way"
            ),
        };
    }

    let Some(path_mtu) = probe.path_mtu else {
        return Verdict {
            status: "inconclusive",
            blackhole: false,
            confidence: "low",
            summary: "path MTU did not converge".to_string(),
        };
    };

    if path_mtu >= ceiling {
        return Verdict {
            status: "ok",
            blackhole: false,
            confidence: "high",
            summary: format!("path MTU is at least {ceiling} (search ceiling)"),
        };
    }

    // 大包被静默丢弃且全程没有任何 ICMP 通告 —— 典型的 PMTUD 黑洞。
    if saw_timeout && !saw_too_big {
        return Verdict {
            status: "blackhole",
            blackhole: true,
            confidence: "medium",
            summary: format!(
                "packets above {path_mtu} bytes are dropped silently with no ICMP \
                 fragmentation-needed reply: PMTUD is broken on this path, \
                 large transfers will stall"
            ),
        };
    }

    let local_hint = match local.mtu {
        Some(mtu) if mtu > path_mtu => format!(
            "; local interface MTU is {mtu}, so {} bytes are lost to tunnel overhead",
            mtu - path_mtu
        ),
        _ => String::new(),
    };

    Verdict {
        status: "reduced",
        blackhole: false,
        confidence: if saw_too_big { "high" } else { "medium" },
        summary: format!("path MTU is {path_mtu}{local_hint}"),
    }
}

fn notes() -> Vec<String> {
    vec![
        "Probing relies on the system ping with the DF bit set; \
         paths that filter all ICMP cannot be measured this way."
            .to_string(),
        "A blackhole verdict means large packets vanish without an ICMP notice; \
         lowering the tunnel MTU or enabling TCP MSS clamping is the usual fix."
            .to_string(),
        "Proxied traffic is not covered: the proxy establishes its own path to the target."
            .to_string(),
    ]
}

async fn detect_local_mtu() -> LocalMtu {
    let interfaces = tokio::task::spawn_blocking(crate::info::collect_interfaces)
        .await
        .unwrap_or_default();
    let Some(egress) = interfaces.iter().find(|iface| iface.is_egress) else {
        return LocalMtu {
            interface: None,
            mtu: None,
        };
    };
    let name = egress.name.clone();
    let lookup = name.clone();
    let mtu = tokio::task::spawn_blocking(move || query_interface_mtu(&lookup))
        .await
        .ok()
        .flatten();
    LocalMtu {
        interface: Some(name),
        mtu,
    }
}

#[cfg(target_os = "windows")]
fn query_interface_mtu(interface: &str) -> Option<u32> {
    let script = format!(
        "Get-NetIPInterface -InterfaceAlias '{}' -AddressFamily IPv4 | \
         Select-Object -First 1 -ExpandProperty NlMtu",
        interface.replace('\'', "''")
    );
    let output = crate::util::powershell_output(&script, Duration::from_secs(5))?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn query_interface_mtu(interface: &str) -> Option<u32> {
    let output = crate::util::command_output_timeout(
        "ip",
        &["-o", "link", "show", "dev", interface],
        Duration::from_secs(5),
    )?;
    parse_ip_link_mtu(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "macos")]
fn query_interface_mtu(interface: &str) -> Option<u32> {
    let output =
        crate::util::command_output_timeout("ifconfig", &[interface], Duration::from_secs(5))?;
    parse_ifconfig_mtu(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn query_interface_mtu(_interface: &str) -> Option<u32> {
    None
}

/// 解析 `ip -o link show` 输出中的 `mtu N`
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_ip_link_mtu(text: &str) -> Option<u32> {
    parse_token_after(text, "mtu")
}

/// 解析 `ifconfig` 输出中的 `mtu N`
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_ifconfig_mtu(text: &str) -> Option<u32> {
    parse_token_after(text, "mtu")
}

fn parse_token_after(text: &str, token: &str) -> Option<u32> {
    let mut iter = text.split_whitespace();
    while let Some(word) = iter.next() {
        if word.eq_ignore_ascii_case(token) {
            return iter.next()?.trim_end_matches(':').parse().ok();
        }
    }
    None
}

fn fail(mode: OutputMode, message: &str, target: &str) {
    crate::output::mark_failure();
    if mode == OutputMode::Json {
        print_json_error(message);
    } else {
        println!();
        println!("{} {}", "🧩 Path MTU".bold(), target);
        println!("  {}", message.red());
    }
}

fn print_report(report: &MtuReport) {
    println!();
    println!("{} {}", "🧩 Path MTU".bold(), report.target);
    println!(
        "  Target: {} ({})",
        report.resolved_ip.as_deref().unwrap_or("--"),
        report.family
    );
    println!(
        "  Local: {} (MTU {})",
        report.local.interface.as_deref().unwrap_or("--"),
        report
            .local
            .mtu
            .map(|mtu| mtu.to_string())
            .unwrap_or_else(|| "--".to_string())
    );
    println!(
        "  Search Range: {}-{} bytes",
        report.probe.searched_min_mtu, report.probe.searched_max_mtu
    );
    println!("  Method: {}", report.probe.method);

    println!();
    println!("{}", "Probes".bold());
    let rows = report
        .probe
        .steps
        .iter()
        .map(|step| {
            let result = match step.result.as_str() {
                "ok" => step.result.green().to_string(),
                "too-big" => step.result.yellow().to_string(),
                _ => step.result.red().to_string(),
            };
            vec![
                step.mtu.to_string(),
                step.payload_bytes.to_string(),
                result,
                step.detail.clone().unwrap_or_default(),
            ]
        })
        .collect::<Vec<_>>();
    print_table(&["MTU", "Payload", "Result", "Detail"], &rows);

    println!();
    println!("{}", "Result".bold());
    println!(
        "  Path MTU: {}",
        report
            .probe
            .path_mtu
            .map(|mtu| mtu.to_string())
            .unwrap_or_else(|| "--".to_string())
    );
    if let Some(advertised) = report.probe.advertised_mtu {
        println!("  Router Advertised MTU: {advertised}");
    }
    let status = match report.verdict.status {
        "ok" => report.verdict.status.green().to_string(),
        "reduced" => report.verdict.status.yellow().to_string(),
        _ => report.verdict.status.red().to_string(),
    };
    println!("  Status: {} ({})", status, report.verdict.confidence);
    println!("  {}", report.verdict.summary);

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_reply_is_ok() {
        let stdout = "Reply from 8.8.8.8: bytes=1472 time=12ms TTL=115";

        assert_eq!(
            classify(PingDialect::Windows, stdout, "", true),
            ProbeOutcome::Ok
        );
    }

    #[test]
    fn windows_df_rejection_is_too_big() {
        let stdout = "Packet needs to be fragmented but DF set.";

        assert_eq!(
            classify(PingDialect::Windows, stdout, "", false),
            ProbeOutcome::TooBig {
                advertised_mtu: None
            }
        );
    }

    #[test]
    fn windows_timeout_is_timeout() {
        let stdout = "Request timed out.";

        assert_eq!(
            classify(PingDialect::Windows, stdout, "", false),
            ProbeOutcome::Timeout
        );
    }

    #[test]
    fn linux_local_error_reports_advertised_mtu() {
        let stderr = "ping: local error: Message too long, mtu=1400";

        assert_eq!(
            classify(PingDialect::Linux, "", stderr, false),
            ProbeOutcome::TooBig {
                advertised_mtu: Some(1400)
            }
        );
    }

    #[test]
    fn linux_icmp_frag_needed_reports_advertised_mtu() {
        let stdout = "From 10.0.0.1 icmp_seq=1 Frag needed and DF set (mtu = 1380)";

        assert_eq!(
            classify(PingDialect::Linux, stdout, "", false),
            ProbeOutcome::TooBig {
                advertised_mtu: Some(1380)
            }
        );
    }

    #[test]
    fn linux_reply_is_ok() {
        let stdout = "1480 bytes from 8.8.8.8: icmp_seq=1 ttl=115 time=11.9 ms";

        assert_eq!(
            classify(PingDialect::Linux, stdout, "", true),
            ProbeOutcome::Ok
        );
    }

    #[test]
    fn full_packet_loss_is_timeout() {
        let stdout = "1 packets transmitted, 0 received, 100% packet loss";

        assert_eq!(
            classify(PingDialect::Linux, stdout, "", false),
            ProbeOutcome::Timeout
        );
    }

    #[test]
    fn macos_message_too_big_is_too_big() {
        let stderr = "ping: sendto: Message too big";

        assert_eq!(
            classify(PingDialect::MacOs, "", stderr, false),
            ProbeOutcome::TooBig {
                advertised_mtu: None
            }
        );
    }

    #[test]
    fn icmpv6_packet_too_big_is_too_big() {
        let stdout = "From 2001:db8::1 icmp_seq=1 Packet too big: mtu=1280";

        assert_eq!(
            classify(PingDialect::Linux, stdout, "", false),
            ProbeOutcome::TooBig {
                advertised_mtu: Some(1280)
            }
        );
    }

    #[test]
    fn unknown_host_is_unreachable() {
        let stderr = "ping: nope.invalid: Name or service not known";

        let outcome = classify(PingDialect::Linux, "", stderr, false);

        assert!(matches!(outcome, ProbeOutcome::Unreachable(_)));
    }

    #[test]
    fn too_big_wins_over_loss_statistics() {
        // Linux 在拒绝后仍会打印统计行，两个标记同时出现时必须判为 too-big。
        let stdout = "ping: local error: Message too long, mtu=1400\n\
                      1 packets transmitted, 0 received, 100% packet loss";

        assert_eq!(
            classify(PingDialect::Linux, stdout, "", false),
            ProbeOutcome::TooBig {
                advertised_mtu: Some(1400)
            }
        );
    }

    #[test]
    fn advertised_mtu_ignores_unrelated_numbers() {
        assert_eq!(parse_advertised_mtu("mtu = 1400"), Some(1400));
        assert_eq!(parse_advertised_mtu("mtu 1400"), Some(1400));
        assert_eq!(parse_advertised_mtu("mtu=1400"), Some(1400));
        assert_eq!(parse_advertised_mtu("no mtu here"), None);
        assert_eq!(parse_advertised_mtu("time=12ms ttl=115"), None);
    }

    #[test]
    fn windows_ping_args_set_df_bit() {
        let args = ping_args(PingDialect::Windows, "8.8.8.8", "1472", "2000");

        assert!(args.contains(&"-f".to_string()));
        assert!(args.contains(&"1472".to_string()));
        assert_eq!(args.last().unwrap(), "8.8.8.8");
    }

    #[test]
    fn linux_ping_args_disable_fragmentation() {
        let args = ping_args(PingDialect::Linux, "8.8.8.8", "1472", "2");

        assert!(args.windows(2).any(|pair| pair == ["-M", "do"]));
    }

    #[test]
    fn macos_ping_args_disable_fragmentation() {
        let args = ping_args(PingDialect::MacOs, "8.8.8.8", "1472", "2");

        assert!(args.contains(&"-D".to_string()));
    }

    #[test]
    fn payload_subtracts_protocol_overhead() {
        let mut steps = Vec::new();
        record(&mut steps, 1500, false, &ProbeOutcome::Ok);
        record(&mut steps, 1500, true, &ProbeOutcome::Ok);

        assert_eq!(steps[0].payload_bytes, 1472);
        assert_eq!(steps[1].payload_bytes, 1452);
    }

    #[test]
    fn advertised_mtu_keeps_narrowest_hop() {
        let mut slot = None;
        collect_advertised(
            &mut slot,
            &ProbeOutcome::TooBig {
                advertised_mtu: Some(1400),
            },
        );
        collect_advertised(
            &mut slot,
            &ProbeOutcome::TooBig {
                advertised_mtu: Some(1280),
            },
        );
        collect_advertised(
            &mut slot,
            &ProbeOutcome::TooBig {
                advertised_mtu: Some(1450),
            },
        );

        assert_eq!(slot, Some(1280));
    }

    fn summary(steps: Vec<ProbeStep>, path_mtu: Option<u32>) -> ProbeSummary {
        ProbeSummary {
            method: method_name(),
            searched_min_mtu: 576,
            searched_max_mtu: 1500,
            path_mtu,
            advertised_mtu: None,
            steps,
        }
    }

    fn step(mtu: u32, result: &str) -> ProbeStep {
        ProbeStep {
            mtu,
            payload_bytes: mtu - IPV4_ICMP_OVERHEAD,
            result: result.to_string(),
            detail: None,
        }
    }

    fn no_local() -> LocalMtu {
        LocalMtu {
            interface: None,
            mtu: None,
        }
    }

    #[test]
    fn silent_drop_without_icmp_is_blackhole() {
        let probe = summary(
            vec![step(576, "ok"), step(1500, "timeout"), step(1400, "ok")],
            Some(1400),
        );

        let verdict = judge(&probe, &no_local(), 576, 1500);

        assert!(verdict.blackhole);
        assert_eq!(verdict.status, "blackhole");
    }

    #[test]
    fn explicit_icmp_report_is_reduced_not_blackhole() {
        let probe = summary(
            vec![step(576, "ok"), step(1500, "too-big"), step(1400, "ok")],
            Some(1400),
        );

        let verdict = judge(&probe, &no_local(), 576, 1500);

        assert!(!verdict.blackhole);
        assert_eq!(verdict.status, "reduced");
        assert_eq!(verdict.confidence, "high");
    }

    #[test]
    fn reduced_verdict_reports_tunnel_overhead() {
        let probe = summary(
            vec![step(576, "ok"), step(1500, "too-big"), step(1400, "ok")],
            Some(1400),
        );
        let local = LocalMtu {
            interface: Some("wg0".to_string()),
            mtu: Some(1500),
        };

        let verdict = judge(&probe, &local, 576, 1500);

        assert!(
            verdict.summary.contains("100 bytes are lost"),
            "{}",
            verdict.summary
        );
    }

    #[test]
    fn full_ceiling_pass_is_ok() {
        let probe = summary(vec![step(576, "ok"), step(1500, "ok")], Some(1500));

        let verdict = judge(&probe, &no_local(), 576, 1500);

        assert_eq!(verdict.status, "ok");
        assert!(!verdict.blackhole);
    }

    #[test]
    fn unreachable_floor_is_inconclusive() {
        let probe = summary(vec![step(576, "timeout")], None);

        let verdict = judge(&probe, &no_local(), 576, 1500);

        assert_eq!(verdict.status, "inconclusive");
        assert_eq!(verdict.confidence, "low");
        assert!(!verdict.blackhole);
    }

    #[test]
    fn parses_ip_link_mtu() {
        let text = "2: eth0: <BROADCAST,MULTICAST,UP> mtu 1450 qdisc mq state UP";

        assert_eq!(parse_ip_link_mtu(text), Some(1450));
    }

    #[test]
    fn parses_ifconfig_mtu() {
        let text = "en0: flags=8863<UP,BROADCAST,SMART,RUNNING> mtu 1500";

        assert_eq!(parse_ifconfig_mtu(text), Some(1500));
    }

    #[test]
    fn missing_mtu_token_returns_none() {
        assert_eq!(parse_token_after("eth0: no mtu value here", "mtu"), None);
    }
}
