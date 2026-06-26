//! 国际化模块：根据系统自动切换中英文。
//!
//! 优先级：`--lang` 参数 > `NETUTILS_LANG` 环境变量 > 系统自动检测。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};

use once_cell::sync::Lazy;

/// 支持的语言
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "lower")]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    fn as_u8(self) -> u8 {
        match self {
            Lang::Zh => 0,
            Lang::En => 1,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => Lang::Zh,
            _ => Lang::En,
        }
    }
}

/// 全局语言设置（线程安全，无 unsafe）
static LANG: AtomicU8 = AtomicU8::new(0); // 默认 Zh，init() 会覆盖
static INITIALIZED: AtomicU8 = AtomicU8::new(0); // 0=未初始化, 1=已初始化

/// 初始化语言（从 --lang 参数或环境变量）
pub fn init(lang: Option<Lang>) {
    let detected = lang.unwrap_or_else(detect);
    LANG.store(detected.as_u8(), Ordering::Relaxed);
    INITIALIZED.store(1, Ordering::Relaxed);
}

/// 获取当前语言
pub fn current() -> Lang {
    if INITIALIZED.load(Ordering::Relaxed) == 0 {
        init(None);
    }
    Lang::from_u8(LANG.load(Ordering::Relaxed))
}

/// 自动检测系统语言
pub fn detect() -> Lang {
    // 1. 环境变量 NETUTILS_LANG
    if let Ok(v) = std::env::var("NETUTILS_LANG") {
        match v.to_lowercase().as_str() {
            "en" | "english" => return Lang::En,
            "zh" | "chinese" | "cn" => return Lang::Zh,
            _ => {}
        }
    }

    // 2. 环境变量 LANG（Unix 风格，Windows 上 Git Bash 等也可能设置）
    if let Ok(v) = std::env::var("LANG") {
        if v.starts_with("zh") {
            return Lang::Zh;
        }
        if !v.is_empty() {
            return Lang::En;
        }
    }

    // 3. Windows: 通过 locale ID 检测
    #[cfg(target_os = "windows")]
    {
        if is_chinese_windows() {
            return Lang::Zh;
        }
        return Lang::En;
    }

    // 4. 默认英文
    #[cfg(not(target_os = "windows"))]
    {
        Lang::En
    }
}

#[cfg(target_os = "windows")]
fn is_chinese_windows() -> bool {
    // 使用 GetACP 获取系统 ANSI 代码页
    // 中文代码页: 936 (GBK), 950 (Big5), 54936 (GB18030)
    unsafe {
        let acp = windows_sys::Win32::Globalization::GetACP();
        matches!(acp, 936 | 950 | 54936)
    }
}

/// 翻译条目
fn build_dict() -> HashMap<&'static str, [&'static str; 2]> {
    // 每个 key 对应 [zh, en]
    let entries: Vec<(&str, &str, &str)> = vec![
        // ── common ──
        ("common.success", "成功", "success"),
        ("common.fail", "失败", "fail"),
        ("common.error", "错误", "error"),
        ("common.unknown", "未知", "unknown"),
        ("common.not_set", "未设置", "not set"),
        ("common.yes", "是", "yes"),
        ("common.no", "否", "no"),
        ("common.none", "--", "--"),
        ("common.metric", "指标", "Metric"),

        // ── banner ──
        ("banner.title", "本地网络检测报告", "Local Network Report"),

        // ── iface ──
        ("iface.title", "📡 网络接口列表", "📡 Network Interfaces"),
        ("iface.name", "名称", "Name"),
        ("iface.mac", "MAC 地址", "MAC Address"),
        ("iface.ipv4", "IPv4", "IPv4"),
        ("iface.status", "状态", "Status"),
        ("iface.type", "类型", "Type"),
        ("iface.metric", "跃点", "Metric"),
        ("iface.egress", "出口", "Egress"),
        ("iface.egress_yes", "✓ 出口", "✓ egress"),
        ("iface.egress_backup", "~ 备用", "~ backup"),
        ("iface.summary", "共 {0} 个接口，其中 {1} 个虚拟网卡", "{0} interfaces, {1} virtual"),

        // ── iface types ──
        ("iface.type_loopback", "回环", "Loopback"),
        ("iface.type_ethernet", "以太网", "Ethernet"),
        ("iface.type_wireless", "无线", "Wireless"),
        ("iface.type_mihomo", "Mihomo/TUN", "Mihomo/TUN"),
        ("iface.type_clash", "Clash/TUN", "Clash/TUN"),
        ("iface.type_wireguard", "WireGuard", "WireGuard"),
        ("iface.type_openvpn", "OpenVPN", "OpenVPN"),
        ("iface.type_radmin", "Radmin VPN", "Radmin VPN"),
        ("iface.type_zerotier", "ZeroTier", "ZeroTier"),
        ("iface.type_tailscale", "Tailscale", "Tailscale"),
        ("iface.type_virtualbox", "VirtualBox", "VirtualBox"),
        ("iface.type_vmware", "VMware", "VMware"),
        ("iface.type_hyperv", "Hyper-V", "Hyper-V"),
        ("iface.type_docker", "Docker", "Docker"),
        ("iface.type_tuntap", "TUN/TAP", "TUN/TAP"),
        ("iface.type_other", "其他", "Other"),

        // ── egress ──
        ("egress.title", "🚪 流量出口", "🚪 Egress"),
        ("egress.iface", "接口", "Interface"),
        ("egress.ip", "IP", "IP"),
        ("egress.type", "类型", "Type"),
        ("egress.metric", "跃点", "Metric"),
        ("egress.metric_hint", "接口跃点，越小优先级越高", "interface metric, lower = higher priority"),
        ("egress.logic_title", "选路逻辑", "Routing Logic"),
        ("egress.logic_1", "系统为出站流量选择出口时，比较每个候选路由的 有效跃点：", "System selects egress by comparing effective metric of each candidate route:"),
        ("egress.logic_2", "有效跃点 = 路由跃点(RouteMetric) + 接口跃点(InterfaceMetric)", "Effective Metric = RouteMetric + InterfaceMetric"),
        ("egress.logic_3", "有效跃点越低，接口越优先。", "Lower effective metric = higher priority."),
        ("egress.logic_selected", "{0} 的有效跃点 = 路由跃点({1}) + 接口跃点({2}) = {3}，选中", "{0} effective metric = route({1}) + interface({2}) = {3}, selected"),
        ("egress.unreachable", "无法检测（可能无网络连接）", "Unable to detect (no network connection?)"),

        // ── route ──
        ("route.title", "🗺️  路由表 (默认路由优先)", "🗺️  Routing Table (default first)"),
        ("route.dest", "目标", "Destination"),
        ("route.gateway", "网关", "Gateway"),
        ("route.interface", "接口", "Interface"),
        ("route.metric", "跃点", "Metric"),

        // ── proxy ──
        ("proxy.title", "🔒 代理设置", "🔒 Proxy Settings"),
        ("proxy.type", "类型", "Type"),
        ("proxy.value", "值", "Value"),
        ("proxy.http", "HTTP 代理", "HTTP Proxy"),
        ("proxy.https", "HTTPS 代理", "HTTPS Proxy"),
        ("proxy.all", "全局代理", "All Proxy"),
        ("proxy.no", "排除列表", "No Proxy"),
        ("proxy.env", "环境变量", "Env Variables"),
        ("proxy.system", "系统代理", "System Proxy"),
        ("proxy.disabled", "未启用", "disabled"),

        // ── ping ──
        ("ping.title", "🏓 Ping {0}", "🏓 Ping {0}"),
        ("ping.resolve_fail", "❌ 无法解析主机: {0}", "❌ Failed to resolve host: {0}"),
        ("ping.target", "目标: {0} ({1})", "Target: {0} ({1})"),
        ("ping.icmp_fallback", "⚠ ICMP 不可用，回退到 TCP ping (端口 80)", "⚠ ICMP unavailable, falling back to TCP ping (port 80)"),
        ("ping.client_fail", "ICMP client 创建失败: {0}", "ICMP client creation failed: {0}"),
        ("ping.reply", "seq={0} 来自 {1} 时间={2}ms", "seq={0} from {1} time={2}ms"),
        ("ping.timeout", "TCP: 超时", "TCP: timeout"),
        ("ping.fail", "seq={0} 失败: {1}", "seq={0} failed: {1}"),
        ("ping.stats", "📊 统计", "📊 Statistics"),
        ("ping.sent", "发送", "Sent"),
        ("ping.recv", "接收", "Received"),
        ("ping.lost", "丢失", "Lost"),
        ("ping.loss_rate", "丢包率", "Loss Rate"),
        ("ping.min", "最小延迟", "Min"),
        ("ping.max", "最大延迟", "Max"),
        ("ping.avg", "平均延迟", "Avg"),

        // ── dns ──
        ("dns.title", "🔍 DNS 查询: {0} ({1})", "🔍 DNS Query: {0} ({1})"),
        ("dns.no_record", "未找到 {0} 记录", "No {0} records found"),
        ("dns.fail", "❌ 查询失败: {0}", "❌ Query failed: {0}"),
        ("dns.elapsed", "查询耗时: {0}ms", "Elapsed: {0}ms"),
        ("dns.idx", "序号", "#"),
        ("dns.value", "记录值", "Value"),
        ("dns.ttl", "TTL", "TTL"),

        // ── trace ──
        ("trace.title", "🛤️  Traceroute to {0}", "🛤️  Traceroute to {0}"),
        ("trace.resolve_fail", "❌ 无法解析主机: {0}", "❌ Failed to resolve host: {0}"),
        ("trace.target", "目标: {0} ({1})", "Target: {0} ({1})"),
        ("trace.max_hops", "最大跳数: {0}", "Max hops: {0}"),
        ("trace.not_reached", "⚠ 未在 {0} 跳内到达目标", "⚠ Did not reach target within {0} hops"),
        ("trace.hop", "跳数", "Hop"),
        ("trace.ip", "IP 地址", "IP Address"),
        ("trace.probe", "延迟 {0}", "Probe {0}"),

        // ── scan ──
        ("scan.title", "🔎 端口扫描: {0}", "🔎 Port Scan: {0}"),
        ("scan.resolve_fail", "❌ 无法解析主机: {0}", "❌ Failed to resolve host: {0}"),
        ("scan.target", "目标: {0} ({1})", "Target: {0} ({1})"),
        ("scan.info", "扫描 {0} 个端口，并发 {1}", "Scanning {0} ports, concurrency {1}"),
        ("scan.no_open", "未发现开放端口", "No open ports found"),
        ("scan.done", "扫描完成: {0}/{1} 开放", "Done: {0}/{1} open"),
        ("scan.port", "端口", "Port"),
        ("scan.state", "状态", "State"),
        ("scan.service", "服务", "Service"),

        // ── check ──
        ("check.title", "🔌 连通性测试: {0}", "🔌 Connectivity: {0}"),
        ("check.format_err", "❌ 格式错误，请使用 host:port", "❌ Invalid format, use host:port"),
        ("check.port_err", "❌ 端口号无效: {0}", "❌ Invalid port: {0}"),
        ("check.tcp", "类型: TCP", "Type: TCP"),
        ("check.http", "类型: HTTP", "Type: HTTP"),
        ("check.tcp_ok", "[{0}/{1}] ✓ 连接成功  {2}ms", "[{0}/{1}] ✓ connected  {2}ms"),
        ("check.tcp_fail", "[{0}/{1}] ✗ 连接失败  {2}", "[{0}/{1}] ✗ failed  {2}"),
        ("check.tcp_timeout", "[{0}/{1}] ✗ 连接超时 ({2}s)", "[{0}/{1}] ✗ timeout ({2}s)"),
        ("check.http_ok", "[{0}/{1}] {2} {3}  {4}ms", "[{0}/{1}] {2} {3}  {4}ms"),
        ("check.http_fail", "[{0}/{1}] ✗ {2}  {3}ms", "[{0}/{1}] ✗ {2}  {3}ms"),
        ("check.conn_fail", "连接失败", "connection failed"),
        ("check.req_timeout", "请求超时", "request timeout"),
        ("check.count", "测试次数", "Tests"),
        ("check.ok", "成功", "OK"),
        ("check.fail_count", "失败/错误", "Failed"),
        ("check.ok_2xx", "成功 (2xx)", "OK (2xx)"),

        // ── diag ──
        // ── connections ──
        ("connections.title", "📡 活动网络连接", "📡 Active Network Connections"),
        ("connections.proto", "协议", "Protocol"),
        ("connections.local", "本地地址", "Local Address"),
        ("connections.remote", "远程地址", "Remote Address"),
        ("connections.state", "状态", "State"),
        ("connections.pid", "PID", "PID"),
        ("connections.process", "进程", "Process"),
        ("connections.no_result", "未找到连接", "No connections found"),
        ("connections.summary", "共 {0} 个连接（{1} TCP, {2} UDP）", "{0} connections ({1} TCP, {2} UDP)"),
        ("connections.no_admin", "注意：非管理员权限下进程信息可能不完整", "Note: process info may be incomplete without admin privileges"),

        // ── diag ──
        ("diag.title", "🔍 网络诊断报告", "🔍 Network Diagnostics"),
        ("diag.elapsed", "诊断耗时: {0}s", "Time: {0}s"),
        ("diag.check_egress", "出口", "Egress"),
        ("diag.check_gateway", "网关", "Gateway"),
        ("diag.check_proxy", "代理", "Proxy"),
        ("diag.check_ipv6", "IPv6", "IPv6"),
        ("diag.net_ok", "网络连接正常 (出口: {0} {1})", "Network connected (egress: {0} {1})"),
        ("diag.net_fail", "无网络连接", "No network connection"),
        ("diag.dns_ok", "DNS 解析正常 ({0} → {1}, {2}ms)", "DNS OK ({0} → {1}, {2}ms)"),
        ("diag.dns_fail", "DNS 解析失败 ({0})", "DNS failed ({0})"),
        ("diag.dns_cn", "国内 DNS", "Domestic DNS"),
        ("diag.dns_global", "国际 DNS", "Global DNS"),
        ("diag.gw_ok", "默认网关可达 ({0}, {1}ms)", "Gateway reachable ({0}, {1}ms)"),
        ("diag.gw_ok_no_rtt", "默认网关存在 ({0})", "Gateway found ({0})"),
        ("diag.gw_fail", "默认网关不可达", "Gateway unreachable"),
        ("diag.proxy_on", "系统代理已启用 ({0})", "System proxy enabled ({0})"),
        ("diag.proxy_off", "系统代理未启用", "System proxy disabled"),
        ("diag.http_ok", "HTTPS 连通正常 ({0} → {1}, {2}ms)", "HTTPS OK ({0} → {1}, {2}ms)"),
        ("diag.http_fail", "HTTPS 连通失败 ({0})", "HTTPS failed ({0})"),
        ("diag.http_cn", "国内连通", "Domestic HTTP"),
        ("diag.http_global", "国际连通", "Global HTTP"),
        ("diag.via_proxy", "经代理", "via proxy"),
        ("diag.direct", "直连", "direct"),
        ("diag.ipv6_ok", "IPv6 可用", "IPv6 available"),
        ("diag.ipv6_fail", "IPv6 不可用", "IPv6 unavailable"),
    ];

    let mut map = HashMap::new();
    for (key, zh, en) in entries {
        map.insert(key, [zh, en]);
    }
    map
}

static DICT: Lazy<HashMap<&'static str, [&'static str; 2]>> = Lazy::new(build_dict);

/// 获取翻译文本（无格式化参数）
pub fn t(key: &str) -> String {
    let lang = current();
    if let Some(vals) = DICT.get(key) {
        vals[lang.as_u8() as usize].to_string()
    } else {
        key.to_string()
    }
}

/// 获取翻译文本（带一个格式化参数）
pub fn t1(key: &str, a: &str) -> String {
    t(key).replace("{0}", a)
}

/// 获取翻译文本（带两个格式化参数）
pub fn t2(key: &str, a: &str, b: &str) -> String {
    t(key).replace("{0}", a).replace("{1}", b)
}

/// 获取翻译文本（带三个格式化参数）
#[allow(dead_code)]
pub fn t3(key: &str, a: &str, b: &str, c: &str) -> String {
    t(key).replace("{0}", a).replace("{1}", b).replace("{2}", c)
}

/// 获取翻译文本（带四个格式化参数）
pub fn t4(key: &str, a: &str, b: &str, c: &str, d: &str) -> String {
    t(key)
        .replace("{0}", a)
        .replace("{1}", b)
        .replace("{2}", c)
        .replace("{3}", d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_t_basic() {
        init(Some(Lang::Zh));
        assert_eq!(t("common.success"), "成功");
        init(Some(Lang::En));
        assert_eq!(t("common.success"), "success");
    }

    #[test]
    fn test_t1() {
        init(Some(Lang::Zh));
        assert_eq!(t1("ping.title", "baidu.com"), "🏓 Ping baidu.com");
    }

    #[test]
    fn test_t2() {
        init(Some(Lang::En));
        assert_eq!(t2("ping.target", "host", "1.2.3.4"), "Target: host (1.2.3.4)");
    }

    #[test]
    fn test_t4() {
        init(Some(Lang::Zh));
        let result = t4("egress.logic_selected", "eth0", "0", "25", "25");
        assert!(result.contains("eth0"));
        assert!(result.contains("25"));
        assert!(!result.contains("{"));
    }

    #[test]
    fn test_missing_key() {
        init(Some(Lang::En));
        assert_eq!(t("nonexistent.key"), "nonexistent.key");
    }
}
