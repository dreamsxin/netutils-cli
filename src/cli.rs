//! 子命令定义。

use std::ffi::OsString;

use clap::{Parser, Subcommand};

use crate::dns::DnsRecordType;
use crate::i18n::Lang;

/// 本地网络检测工具集
#[derive(Parser, Debug)]
#[command(name = "netutils", version, about = "Local network diagnostic toolkit", long_about = None)]
pub struct Cli {
    /// JSON 输出（便于脚本处理）
    #[arg(long, global = true)]
    pub json: bool,

    /// 覆盖语言（zh/en），默认自动检测
    #[arg(long, global = true, value_enum)]
    pub lang: Option<Lang>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 显示全部网络信息（默认）
    #[command(alias = "a")]
    All,

    /// 仅显示网络接口列表
    #[command(alias = "i")]
    Iface,

    /// 仅显示流量出口
    #[command(alias = "e")]
    Egress,

    /// 仅显示路由表
    #[command(alias = "r")]
    Route,

    /// 查询目标 IP/域名的实际路由选择，并解释是否走 TUN
    #[command(alias = "rt")]
    RouteGet {
        /// 目标主机名或 IP
        target: String,
        /// 快速 trace 最大跳数（默认 10）
        #[arg(long, default_value_t = 10)]
        max_hops: u32,
        /// 只显示路由选择，不执行快速 trace
        #[arg(long)]
        no_trace: bool,
    },

    /// 仅显示代理设置
    #[command(alias = "p")]
    Proxy,

    /// Ping 主机（ICMP，无权限时回退 TCP）
    #[command(alias = "pg")]
    Ping {
        /// 目标主机名或 IP
        host: String,
        /// 发送包数（默认 4）
        #[arg(short, long, default_value_t = 4)]
        count: u32,
        /// 单次探测超时秒数（默认 2）
        #[arg(long, default_value_t = 2)]
        timeout: u64,
        /// 探测间隔秒数（默认 1）
        #[arg(long, default_value_t = 1)]
        interval: u64,
    },

    /// DNS 查询
    #[command(alias = "d")]
    Dns {
        /// 目标域名
        domain: String,
        /// 记录类型（默认 A）
        #[arg(short, long, value_enum, default_value_t = DnsRecordType::A)]
        r#type: DnsRecordType,
        /// 指定 DNS 服务器（如 8.8.8.8）
        #[arg(long)]
        server: Option<String>,
    },

    /// 检查系统 DNS 缓存，排查代理/TUN 切换后的陈旧解析
    #[command(alias = "dc")]
    DnsCache {
        /// 只检查指定域名；不指定时显示缓存前若干项
        domain: Option<String>,
        /// 清理系统 DNS 缓存后再检查
        #[arg(long)]
        flush: bool,
        /// 最多显示多少条缓存记录
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },

    /// 检查 DNS 查询会发往哪些 DNS server，以及到 DNS server 的本机路由
    #[command(alias = "dp")]
    DnsPath {
        /// 要测试解析的域名；不指定时只显示 DNS server 和路由
        domain: Option<String>,
        /// 指定 DNS server，仅检查该 server
        #[arg(long)]
        server: Option<String>,
    },

    /// 对比系统默认解析与指定/系统 DNS server 的直查结果
    #[command(alias = "dcp")]
    DnsCompare {
        /// 要对比解析的域名
        domain: String,
        /// 指定 DNS server，可重复传入；不指定时使用系统配置的 DNS servers
        #[arg(long = "server")]
        servers: Vec<String>,
    },

    /// 路由追踪（TTL 递增）
    #[command(alias = "t")]
    Trace {
        /// 目标主机名或 IP
        host: String,
        /// 最大跳数（默认 30）
        #[arg(long, default_value_t = 30)]
        max_hops: u32,
    },

    /// 端口扫描（并发 TCP connect）
    #[command(alias = "s")]
    Scan {
        /// 目标主机名或 IP
        host: String,
        /// 端口列表，逗号分隔（如 80,443,8080），不指定则扫描常见端口
        ports: Option<String>,
        /// 并发数（默认 100）
        #[arg(long, default_value_t = 100)]
        concurrency: usize,
    },

    /// 连通性测试（TCP 端口 / HTTP URL）
    #[command(alias = "c")]
    Check {
        /// 目标地址（host:port 或 http(s)://url）
        target: String,
        /// 测试次数（默认 4）
        #[arg(short, long, default_value_t = 4)]
        count: u32,
        /// 连接超时秒数（默认 5）
        #[arg(long, default_value_t = 5)]
        timeout: u64,
        /// 显示分阶段耗时 (DNS/Connect/TLS/TTFB)，仅直连 HTTPS
        #[arg(long)]
        timing: bool,
        /// 指定代理（如 http://127.0.0.1:7897）
        #[arg(long)]
        proxy: Option<String>,
        /// 强制直连，忽略系统代理
        #[arg(long)]
        no_proxy: bool,
        /// 并发数（默认 1，串行）
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
    },

    /// 发起一次 HTTP 请求并显示响应结果
    #[command(alias = "h")]
    Http {
        /// HTTP/HTTPS URL，省略 scheme 时默认 https://
        url: String,
        /// HTTP 方法（默认 GET）
        #[arg(short = 'X', long, default_value = "GET")]
        method: String,
        /// 请求头，可重复传入，如 -H "Accept: application/json"
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        /// 请求体文本
        #[arg(long)]
        body: Option<String>,
        /// 请求/连接超时秒数（默认 10）
        #[arg(long, default_value_t = 10)]
        timeout: u64,
        /// 指定代理（如 http://127.0.0.1:7897 或 socks5h://127.0.0.1:1080）
        #[arg(long)]
        proxy: Option<String>,
        /// 强制直连，忽略系统代理
        #[arg(long, alias = "no-system-proxy")]
        no_proxy: bool,
        /// 显示响应头
        #[arg(long)]
        show_headers: bool,
        /// 最多显示多少字节响应体（默认 2048，0 表示不显示）
        #[arg(long, default_value_t = 2048)]
        body_limit: usize,
    },

    /// 测试 Server-Sent Events / text/event-stream
    #[command(alias = "event")]
    Sse {
        /// SSE URL，省略 scheme 时默认 https://
        url: String,
        /// 请求头，可重复传入，如 -H "Authorization: Bearer xxx"
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        /// 连接超时秒数（默认 10）
        #[arg(long, default_value_t = 10)]
        timeout: u64,
        /// 最多接收多少个事件（默认 5）
        #[arg(long, default_value_t = 5)]
        max_events: usize,
        /// 最多监听多少秒（默认 30）
        #[arg(long, default_value_t = 30)]
        max_seconds: u64,
        /// 指定代理（如 http://127.0.0.1:7897 或 socks5h://127.0.0.1:1080）
        #[arg(long)]
        proxy: Option<String>,
        /// 强制直连，忽略系统代理
        #[arg(long, alias = "no-system-proxy")]
        no_proxy: bool,
    },

    /// 测试 WebSocket 握手、发送消息和接收消息
    #[command(visible_alias = "websocket")]
    Ws {
        /// WebSocket URL，省略 scheme 时默认 wss://；http(s) 会转换为 ws(s)
        url: String,
        /// 请求头，可重复传入，如 -H "Authorization: Bearer xxx"
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        /// 连接/接收超时秒数（默认 10）
        #[arg(long, default_value_t = 10)]
        timeout: u64,
        /// 连接后发送的文本消息，可重复传入
        #[arg(long = "message")]
        messages: Vec<String>,
        /// 最多接收多少条消息（默认 5）
        #[arg(long, default_value_t = 5)]
        max_messages: usize,
        /// 最多监听多少秒（默认 30）
        #[arg(long, default_value_t = 30)]
        max_seconds: u64,
    },

    /// 安装官方或本地插件
    Install {
        /// 插件名，例如 mcp
        name: String,
        /// 从本地插件 crate 路径安装
        #[arg(long)]
        path: Option<String>,
        /// 强制重新安装
        #[arg(long)]
        force: bool,
    },

    /// 插件管理
    Plugin {
        #[command(subcommand)]
        command: PluginCommands,
    },

    /// 列出当前网络连接（TCP/UDP）
    #[command(visible_alias = "co", alias = "conn")]
    Connections {
        /// 按状态过滤（如 ESTABLISHED, LISTEN）
        #[arg(short, long)]
        state: Option<String>,
        /// 按端口过滤
        #[arg(short, long)]
        port: Option<u16>,
        /// 按进程名过滤
        #[arg(long)]
        process: Option<String>,
        /// 按协议过滤（tcp/udp）
        #[arg(long)]
        proto: Option<String>,
    },

    /// 一键诊断（组合检测，给出结论）
    #[command(alias = "dx")]
    Diag,

    /// 全链路诊断：DNS → Ping → TCP → HTTPS → Traceroute，给出结论
    #[command(alias = "dg")]
    Diagnose {
        /// 目标主机名或 IP
        host: String,
    },

    /// HTTP 请求路径分析：DNS → 代理/出口 → Trace → TCP/TLS/HTTP
    #[command(alias = "pa")]
    Path {
        /// HTTP/HTTPS URL，省略 scheme 时默认 https://
        url: String,
        /// traceroute 最大跳数（默认 10）
        #[arg(long, default_value_t = 10)]
        max_hops: u32,
        /// 请求/连接超时秒数（默认 5）
        #[arg(long, default_value_t = 5)]
        timeout: u64,
        /// 指定代理（如 http://127.0.0.1:7897）
        #[arg(long)]
        proxy: Option<String>,
        /// 强制直连，忽略系统代理
        #[arg(long)]
        no_proxy: bool,
    },

    /// 检查代理是否能通过域名访问目标，并推断代理侧 DNS 可用性
    #[command(alias = "pt")]
    ProxyTest {
        /// 目标域名或 HTTP/HTTPS URL，省略 scheme 时默认 https://
        target: String,
        /// 指定代理（如 http://127.0.0.1:7897 或 socks5h://127.0.0.1:1080）
        #[arg(long)]
        proxy: Option<String>,
        /// 不自动读取系统代理；仅使用 --proxy
        #[arg(long)]
        no_system_proxy: bool,
        /// 请求/连接超时秒数（默认 5）
        #[arg(long, default_value_t = 5)]
        timeout: u64,
    },

    /// TLS 握手与证书诊断
    #[command(alias = "tl")]
    Tls {
        /// 目标主机、host:port 或 https:// URL
        target: String,
        /// 覆盖目标端口；不指定时使用 URL/host 中的端口或 443
        #[arg(long)]
        port: Option<u16>,
        /// 覆盖 SNI；不指定时使用目标主机名
        #[arg(long)]
        sni: Option<String>,
        /// 请求/连接超时秒数（默认 5）
        #[arg(long, default_value_t = 5)]
        timeout: u64,
        /// ALPN 协议列表，逗号分隔（默认 h2,http/1.1）
        #[arg(long, default_value = "h2,http/1.1")]
        alpn: String,
    },

    /// 外部插件命令，例如 netutils mcp ...
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

#[derive(Subcommand, Debug)]
pub enum PluginCommands {
    /// 创建一个 Rust 插件项目骨架
    New {
        /// 插件命令名，例如 whois
        name: String,
        /// 目标父目录，默认当前目录；最终生成 <dir>/<name>
        #[arg(long)]
        dir: Option<String>,
        /// 模板名，目前支持 rust
        #[arg(long, default_value = "rust")]
        template: String,
        /// 二进制名，默认 netutils-<name>
        #[arg(long)]
        binary: Option<String>,
        /// crate 名，默认 netutils-plugin-<name>
        #[arg(long = "crate")]
        crate_name: Option<String>,
        /// 覆盖已有模板文件
        #[arg(long)]
        force: bool,
    },

    /// 列出已知和已安装插件
    List,

    /// 更新已安装插件
    Update {
        /// 插件名
        name: String,
    },

    /// 校验插件目录结构和 manifest
    Validate {
        /// 插件目录路径
        path: String,
    },

    /// 删除已安装插件
    Remove {
        /// 插件名
        name: String,
    },

    /// 显示插件安装目录
    Dir,
}
