//! 一键诊断模块：组合现有功能，给出网络健康结论。

use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;

use crate::i18n::{t, t1, t2};
use crate::output::{print_json, OutputMode};

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
    let mut items = Vec::new();

    // 1. 检测出口
    let egress = check_egress().await;
    items.push(egress);

    // 2. DNS 检测
    let dns = check_dns().await;
    items.push(dns);

    // 3. 网关可达性
    let gateway = check_gateway().await;
    items.push(gateway);

    // 4. 代理状态
    let proxy = check_proxy_status();
    items.push(proxy);

    // 5. HTTP 连通性
    let http = check_http().await;
    items.push(http);

    // 6. IPv6 检测
    let ipv6 = check_ipv6().await;
    items.push(ipv6);

    let elapsed = start.elapsed();
    let timestamp = current_timestamp();

    let report = DiagReport {
        timestamp: timestamp.clone(),
        items: items.clone(),
        elapsed_secs: elapsed.as_secs_f64(),
    };

    if mode == OutputMode::Json {
        print_json(&report);
        return;
    }

    // 表格输出
    println!();
    println!("{}  {}", t("diag.title").bold(), timestamp.cyan());
    println!();

    for item in &items {
        let symbol = if item.ok && !item.warning {
            "✅"
        } else if item.warning {
            "⚠️ "
        } else {
            "❌"
        };
        let colored = if item.ok && !item.warning {
            symbol.green()
        } else if item.warning {
            symbol.yellow()
        } else {
            symbol.red()
        };
        println!("  {} {}", colored, item.message);
    }

    println!();
    println!("  {}", t("diag.elapsed").replace("{0:.1}", &format!("{:.1}", elapsed.as_secs_f64())));
}

/// 检测出口
async fn check_egress() -> DiagItem {
    let interfaces = crate::info::get_all_interfaces();
    let egress_ip = crate::info::egress::detect_egress_ip();
    let egress_iface = egress_ip.and_then(|ip| crate::info::egress::find_egress_interface(&ip, &interfaces));

    match (egress_iface, egress_ip) {
        (Some(name), Some(ip)) => {
            let iface = interfaces.iter().find(|i| i.name == name);
            let iftype = iface
                .map(|i| crate::info::interface::classify_interface(&i.description, &i.name).to_label())
                .unwrap_or_default();
            DiagItem {
                check: "egress".to_string(),
                ok: true,
                warning: false,
                message: t2("diag.net_ok", &name, &format!("({}) {}", iftype, ip)),
            }
        }
        _ => DiagItem {
            check: "egress".to_string(),
            ok: false,
            warning: false,
            message: t("diag.net_fail"),
        },
    }
}

/// 检测 DNS
async fn check_dns() -> DiagItem {
    use trust_dns_resolver::config::*;
    use trust_dns_resolver::TokioAsyncResolver;

    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());
    let start = Instant::now();

    match resolver.lookup_ip("baidu.com").await {
        Ok(ips) => {
            if let Some(ip) = ips.iter().next() {
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                DiagItem {
                    check: "dns".to_string(),
                    ok: true,
                    warning: false,
                    message: t("diag.dns_ok")
                        .replace("{0}", "baidu.com")
                        .replace("{1}", &ip.to_string())
                        .replace("{2}", &format!("{:.0}", elapsed)),
                }
            } else {
                DiagItem {
                    check: "dns".to_string(),
                    ok: false,
                    warning: false,
                    message: t1("diag.dns_fail", "baidu.com"),
                }
            }
        }
        Err(e) => DiagItem {
            check: "dns".to_string(),
            ok: false,
            warning: false,
            message: t1("diag.dns_fail", &e.to_string()),
        },
    }
}

/// 检测网关可达性
async fn check_gateway() -> DiagItem {
    let routes = crate::info::get_default_routes();
    if routes.is_empty() {
        return DiagItem {
            check: "gateway".to_string(),
            ok: false,
            warning: false,
            message: t("diag.gw_fail"),
        };
    }

    let gw_ip = &routes[0].0;
    let gw_addr: std::net::IpAddr = match gw_ip.parse() {
        Ok(ip) => ip,
        Err(_) => return DiagItem {
            check: "gateway".to_string(),
            ok: false,
            warning: false,
            message: t("diag.gw_fail"),
        },
    };

    // 用 surge-ping 测网关
    use surge_ping::{Client, ConfigBuilder, PingIdentifier, PingSequence};
    let client = match Client::new(&ConfigBuilder::default().build()) {
        Ok(c) => c,
        Err(_) => return DiagItem {
            check: "gateway".to_string(),
            ok: true,
            warning: true,
            message: t1("diag.gw_ok_no_rtt", gw_ip),
        },
    };

    let mut pinger = client.pinger(gw_addr, PingIdentifier(0)).await;
    match pinger.ping(PingSequence(0), &[0u8; 32]).await {
        Ok((_, rtt)) => {
            let ms = rtt.as_secs_f64() * 1000.0;
            DiagItem {
                check: "gateway".to_string(),
                ok: true,
                warning: false,
                message: t2("diag.gw_ok", gw_ip, &format!("{:.1}", ms)),
            }
        }
        Err(_) => DiagItem {
            check: "gateway".to_string(),
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
    let system_proxy = proxies.iter().find(|p| {
        p.ptype == sys_label && p.value != disabled
    });

    // 找环境变量代理（非系统代理、非环境变量占位行、值不是 "not set"）
    let env_proxy = proxies.iter().find(|p| {
        p.ptype != sys_label && p.ptype != env_label && p.value != not_set
    });

    let proxy_value = system_proxy
        .or(env_proxy)
        .map(|p| p.value.clone());

    match proxy_value {
        Some(val) => DiagItem {
            check: "proxy".to_string(),
            ok: true,
            warning: true,
            message: t1("diag.proxy_on", &val),
        },
        None => DiagItem {
            check: "proxy".to_string(),
            ok: true,
            warning: false,
            message: t("diag.proxy_off"),
        },
    }
}

/// 检测 HTTP 连通性
async fn check_http() -> DiagItem {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let url = "https://www.baidu.com";
    let start = Instant::now();

    match client.get(url).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            DiagItem {
                check: "http".to_string(),
                ok: true,
                warning: false,
                message: t("diag.http_ok")
                    .replace("{0}", url)
                    .replace("{1}", &status.to_string())
                    .replace("{2}", &format!("{:.0}", elapsed)),
            }
        }
        Err(e) => DiagItem {
            check: "http".to_string(),
            ok: false,
            warning: false,
            message: t1("diag.http_fail", &e.to_string()),
        },
    }
}

/// 检测 IPv6
async fn check_ipv6() -> DiagItem {
    use trust_dns_resolver::config::*;
    use trust_dns_resolver::TokioAsyncResolver;

    let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default());

    match resolver.ipv6_lookup("baidu.com").await {
        Ok(ips) => {
            if ips.iter().next().is_some() {
                DiagItem {
                    check: "ipv6".to_string(),
                    ok: true,
                    warning: false,
                    message: t("diag.ipv6_ok"),
                }
            } else {
                DiagItem {
                    check: "ipv6".to_string(),
                    ok: false,
                    warning: false,
                    message: t("diag.ipv6_fail"),
                }
            }
        }
        Err(_) => DiagItem {
            check: "ipv6".to_string(),
            ok: false,
            warning: false,
            message: t("diag.ipv6_fail"),
        },
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
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", year, month, day, hour, min, sec)
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
