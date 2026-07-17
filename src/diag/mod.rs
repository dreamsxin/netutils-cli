//! 一键诊断模块：组合现有功能，给出网络健康结论。

use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;

use crate::i18n::{t, t1, t2};
use crate::output::{print_json, OutputMode};

const DIAG_GATEWAY_TIMEOUT: Duration = Duration::from_secs(2);
const DIAG_IPV6_TIMEOUT: Duration = Duration::from_secs(5);

/// 单项诊断结果
#[derive(Serialize, Clone)]
pub struct DiagItem {
    pub check: String,
    pub ok: bool,
    pub warning: bool,
    pub message: String,
}

/// 诊断报告
#[derive(Serialize)]
pub struct DiagReport {
    pub timestamp: String,
    pub items: Vec<DiagItem>,
    pub elapsed_secs: f64,
}

/// 执行一键诊断
pub async fn run(mode: OutputMode) {
    let start = Instant::now();

    if mode == OutputMode::Table {
        println!();
        println!("{}  {}", t("diag.title").bold(), current_timestamp().cyan());
        println!();
        println!("  {}...", t("diag.running").dimmed());
    }

    // 各检测并行执行，完成后立即打印
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(check_egress());
    tasks.spawn(check_dns_single("baidu.com"));
    tasks.spawn(check_dns_single("google.com"));
    tasks.spawn(check_gateway());
    tasks.spawn(async { check_proxy_status() });
    tasks.spawn(check_http_single("https://www.baidu.com"));
    tasks.spawn(check_http_single("https://www.google.com"));
    tasks.spawn(check_ipv6());

    let mut items = Vec::new();

    // 按完成顺序等待，Table 模式下实时输出
    while let Some(result) = tasks.join_next().await {
        if let Ok(item) = result {
            if mode == OutputMode::Table {
                let symbol = if item.ok && !item.warning {
                    "✅".green()
                } else if item.warning {
                    "⚠️ ".yellow()
                } else {
                    "❌".red()
                };
                println!("  {} [{}] {}", symbol, item.check.dimmed(), item.message);
            }
            items.push(item);
        }
    }

    let elapsed = start.elapsed();
    let timestamp = current_timestamp();

    let report = DiagReport {
        timestamp: timestamp.clone(),
        items: items.clone(),
        elapsed_secs: elapsed.as_secs_f64(),
    };

    if report.items.iter().any(|item| !item.ok && !item.warning) {
        crate::output::mark_failure();
    }

    if mode == OutputMode::Json {
        print_json(&report);
        return;
    }

    println!();
    println!(
        "  {}",
        t("diag.elapsed").replace("{0}", &format!("{:.1}", elapsed.as_secs_f64()))
    );
}

/// 检测出口
async fn check_egress() -> DiagItem {
    let interfaces = crate::info::get_all_interfaces();
    let egress_ip = crate::info::egress::detect_egress_ip();
    let egress_iface =
        egress_ip.and_then(|ip| crate::info::egress::find_egress_interface(&ip, &interfaces));

    match (egress_iface, egress_ip) {
        (Some(name), Some(ip)) => {
            let iface = interfaces.iter().find(|i| i.name == name);
            let iftype = iface
                .map(|i| {
                    crate::info::interface::classify_interface(&i.description, &i.name).to_label()
                })
                .unwrap_or_default();
            DiagItem {
                check: t("diag.check_egress"),
                ok: true,
                warning: false,
                message: t2("diag.net_ok", &name, &format!("({}) {}", iftype, ip)),
            }
        }
        _ => DiagItem {
            check: t("diag.check_egress"),
            ok: false,
            warning: false,
            message: t("diag.net_fail"),
        },
    }
}

/// 检测单个域名的 DNS 解析
async fn check_dns_single(domain: &str) -> DiagItem {
    let is_cn = domain == "baidu.com";
    let check_label = if is_cn {
        "diag.dns_cn"
    } else {
        "diag.dns_global"
    };
    let start = Instant::now();

    let ips = crate::util::resolve_host_all(domain).await;
    if let Some(ip) = ips.first() {
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let msg = t("diag.dns_ok")
            .replace("{0}", domain)
            .replace("{1}", &ip.to_string())
            .replace("{2}", &format!("{:.0}", elapsed));
        DiagItem {
            check: t(check_label),
            ok: true,
            warning: false,
            message: msg,
        }
    } else {
        DiagItem {
            check: t(check_label),
            ok: false,
            warning: false,
            message: t1("diag.dns_fail", domain),
        }
    }
}

/// 检测网关可达性
async fn check_gateway() -> DiagItem {
    let routes = crate::info::get_default_routes();
    if routes.is_empty() {
        return DiagItem {
            check: t("diag.check_gateway"),
            ok: false,
            warning: false,
            message: t("diag.gw_fail"),
        };
    }

    let gw_ip = &routes[0].0;
    let gw_addr: std::net::IpAddr = match gw_ip.parse() {
        Ok(ip) => ip,
        Err(_) => {
            return DiagItem {
                check: t("diag.check_gateway"),
                ok: false,
                warning: false,
                message: t("diag.gw_fail"),
            }
        }
    };

    // 用 surge-ping 测网关
    use surge_ping::{Client, ConfigBuilder, PingIdentifier, PingSequence};
    let client = match Client::new(&ConfigBuilder::default().build()) {
        Ok(c) => c,
        Err(_) => {
            return DiagItem {
                check: t("diag.check_gateway"),
                ok: true,
                warning: true,
                message: t1("diag.gw_ok_no_rtt", gw_ip),
            }
        }
    };

    let mut pinger = client.pinger(gw_addr, PingIdentifier(0)).await;
    match tokio::time::timeout(
        DIAG_GATEWAY_TIMEOUT,
        pinger.ping(PingSequence(0), &[0u8; 32]),
    )
    .await
    {
        Ok(Ok((_, rtt))) => {
            let ms = rtt.as_secs_f64() * 1000.0;
            DiagItem {
                check: t("diag.check_gateway"),
                ok: true,
                warning: false,
                message: t2("diag.gw_ok", gw_ip, &format!("{:.1}", ms)),
            }
        }
        Ok(Err(_)) | Err(_) => DiagItem {
            check: t("diag.check_gateway"),
            ok: false,
            warning: false,
            message: t("diag.gw_fail"),
        },
    }
}

/// 检测代理状态
fn check_proxy_status() -> DiagItem {
    let proxies = crate::info::proxy::get_proxy_info();
    let sys_label = t("proxy.system");
    let disabled = t("proxy.disabled");
    let env_label = t("proxy.env");
    let not_set = t("common.not_set");

    // 找系统代理（值不是 "disabled"）
    let system_proxy = proxies
        .iter()
        .find(|p| p.ptype == sys_label && p.value != disabled);

    // 找环境变量代理（非系统代理、非环境变量占位行、值不是 "not set"）
    let env_proxy = proxies
        .iter()
        .find(|p| p.ptype != sys_label && p.ptype != env_label && p.value != not_set);

    let proxy_value = system_proxy.or(env_proxy).map(|p| p.value.clone());

    match proxy_value {
        Some(val) => DiagItem {
            check: t("diag.check_proxy"),
            ok: true,
            warning: true,
            message: t1("diag.proxy_on", &val),
        },
        None => DiagItem {
            check: t("diag.check_proxy"),
            ok: true,
            warning: false,
            message: t("diag.proxy_off"),
        },
    }
}

/// 检测单个 URL 的 HTTP 连通性（自动检测并使用系统代理）
async fn check_http_single(url: &str) -> DiagItem {
    let is_cn = url.contains("baidu.com");
    let check_label = if is_cn {
        "diag.http_cn"
    } else {
        "diag.http_global"
    };
    let timeout_secs = 5;

    // 检测系统代理
    let proxy_addr = crate::util::get_system_proxy_addr();
    let via_proxy = proxy_addr.is_some();

    // 构建 client：有系统代理则显式使用，否则直连
    let client = if let Some(ref proxy_url) = proxy_addr {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .proxy(
                reqwest::Proxy::all(proxy_url)
                    .unwrap_or_else(|_| reqwest::Proxy::all("http://0.0.0.0:0").unwrap()),
            )
            .build()
            .unwrap_or_else(|_| {
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(timeout_secs))
                    .build()
                    .unwrap()
            })
    } else {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .no_proxy()
            .build()
            .unwrap()
    };

    let start = Instant::now();

    match client.get(url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let proxy_tag = if via_proxy {
                t("diag.via_proxy")
            } else {
                t("diag.direct")
            };
            let msg = format!(
                "{} [{}]",
                t("diag.http_ok")
                    .replace("{0}", url)
                    .replace("{1}", &status.to_string())
                    .replace("{2}", &format!("{:.0}", elapsed)),
                proxy_tag
            );
            DiagItem {
                check: t(check_label),
                ok: true,
                warning: false,
                message: msg,
            }
        }
        Err(e) => {
            let proxy_tag = if via_proxy {
                t("diag.via_proxy")
            } else {
                t("diag.direct")
            };
            let msg = format!("{} [{}]", t1("diag.http_fail", &e.to_string()), proxy_tag);
            DiagItem {
                check: t(check_label),
                ok: false,
                warning: false,
                message: msg,
            }
        }
    }
}

/// 检测 IPv6
async fn check_ipv6() -> DiagItem {
    let ips = crate::util::resolve_host_all_timeout("baidu.com", DIAG_IPV6_TIMEOUT).await;
    if ips.iter().any(|ip| ip.is_ipv6()) {
        DiagItem {
            check: t("diag.check_ipv6"),
            ok: true,
            warning: false,
            message: t("diag.ipv6_ok"),
        }
    } else {
        DiagItem {
            check: t("diag.check_ipv6"),
            ok: false,
            warning: false,
            message: t("diag.ipv6_fail"),
        }
    }
}

/// 获取当前时间戳
fn current_timestamp() -> String {
    // 简单时间戳，不依赖 chrono
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let hour = (secs % 86400) / 3600;
    let min = (secs % 3600) / 60;
    let sec = secs % 60;
    // 粗略日期（从 1970-01-01 起）
    let (year, month, day) = days_to_date(days as i64);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hour, min, sec
    )
}

/// 天数转日期（从 1970-01-01）
fn days_to_date(days: i64) -> (i64, u32, u32) {
    let mut year = 1970i64;
    let mut remaining = days;

    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u32;
    let mut day = remaining as u32 + 1;

    for (i, &md) in month_days.iter().enumerate() {
        let md = if i == 1 && is_leap(year) { 29 } else { md };
        if day <= md {
            month = (i + 1) as u32;
            break;
        }
        day -= md;
    }

    (year, month, day)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}
