//! 子命令定义。

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
}
