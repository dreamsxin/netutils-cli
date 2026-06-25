//! 网络信息检测模块：接口、路由、出口、代理。

pub mod egress;
pub mod interface;
pub mod proxy;
pub mod route;

// 平台特定实现
#[cfg(target_os = "windows")]
mod interface_win;
#[cfg(target_os = "windows")]
mod route_win;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod interface_unix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod route_unix;

// 平台分发
#[cfg(target_os = "windows")]
pub use interface_win::get_all_interfaces;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use interface_unix::get_all_interfaces;

#[cfg(target_os = "windows")]
pub use route_win::{get_default_routes, get_route_table};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use route_unix::{get_default_routes, get_route_table};

use colored::*;
use serde::Serialize;

use crate::i18n::{t, t2, t4};
use crate::output::{print_json, OutputMode};
use crate::table::print_table;

use egress::{detect_egress_ip, find_egress_interface};
use interface::{classify_interface, InterfaceInfo};
use route::RouteEntry;
use proxy::{get_proxy_info, ProxyEntry};

// ═══════════════════════════════════════════════════════════════
//  数据结构（供 JSON 序列化）
// ═══════════════════════════════════════════════════════════════

/// Egress 信息
#[derive(Serialize)]
pub struct EgressInfo {
    pub interface: String,
    pub ip: String,
    pub iftype: String,
    pub metric: u32,
}

/// 全量信息
#[derive(Serialize)]
pub struct AllInfo {
    pub interfaces: Vec<InterfaceInfo>,
    pub egress: Option<EgressInfo>,
    pub routes: Vec<RouteEntry>,
    pub proxies: Vec<ProxyEntry>,
}

// ═══════════════════════════════════════════════════════════════
//  Collect 函数（纯数据，无输出）
// ═══════════════════════════════════════════════════════════════

/// 收集接口数据（含出口/备用标记）
pub fn collect_interfaces() -> Vec<InterfaceInfo> {
    let mut interfaces = get_all_interfaces();
    let egress_ip = detect_egress_ip();
    let egress_iface = egress_ip.and_then(|ip| find_egress_interface(&ip, &interfaces));

    let default_routes = get_default_routes();
    let default_route_ifaces: Vec<&str> = default_routes
        .iter()
        .map(|(_, iface)| iface.as_str())
        .collect();

    for iface in &mut interfaces {
        iface.is_egress = Some(&iface.name) == egress_iface.as_ref();
        iface.is_backup = !iface.is_egress && default_route_ifaces.contains(&iface.name.as_str());
    }

    interfaces
}

/// 收集出口信息
pub fn collect_egress(interfaces: &[InterfaceInfo]) -> Option<EgressInfo> {
    let egress_ip = detect_egress_ip()?;
    let interfaces_full = get_all_interfaces();
    let egress_iface = find_egress_interface(&egress_ip, &interfaces_full)?;
    let iface = interfaces.iter().find(|i| i.name == egress_iface)?;
    Some(EgressInfo {
        interface: iface.name.clone(),
        ip: iface.ipv4.clone(),
        iftype: iface.iftype.clone(),
        metric: iface.metric,
    })
}

// ═══════════════════════════════════════════════════════════════
//  Render 函数（表格 + 颜色）
// ═══════════════════════════════════════════════════════════════

/// 打印 banner 标题框
pub fn print_banner(title: &str) {
    let title_w = crate::table::display_width(title);
    let inner_w = title_w + 4;
    let border: String = "─".repeat(inner_w);
    let line = format!("│  {}  │", title);
    println!();
    println!("{}", format!("┌{}┐", border).cyan());
    println!("{}", line.cyan().bold());
    println!("{}", format!("└{}┘", border).cyan());
}

/// 打印网络接口列表
pub fn print_interfaces(mode: OutputMode) {
    let interfaces = collect_interfaces();

    if mode == OutputMode::Json {
        print_json(&interfaces);
        return;
    }

    println!();
    println!("{}", t("iface.title").bold());

    let h_name = t("iface.name");
    let h_mac = t("iface.mac");
    let h_ipv4 = t("iface.ipv4");
    let h_status = t("iface.status");
    let h_type = t("iface.type");
    let h_metric = t("iface.metric");
    let h_egress = t("iface.egress");
    let headers = [
        h_name.as_str(),
        h_mac.as_str(),
        h_ipv4.as_str(),
        h_status.as_str(),
        h_type.as_str(),
        h_metric.as_str(),
        h_egress.as_str(),
    ];

    let rows: Vec<Vec<String>> = interfaces
        .iter()
        .map(|iface| {
            let iftype_enum = classify_interface(&iface.description, &iface.name);
            let iftype_label = iftype_enum.to_label();
            let status = if iface.status == "Up" {
                "Up".green().to_string()
            } else {
                "Down".red().to_string()
            };
            let metric_str = if iface.metric == 0 {
                "0 *".to_string()
            } else {
                iface.metric.to_string()
            };
            let egress = if iface.is_egress {
                t("iface.egress_yes").green().bold().to_string()
            } else if iface.is_backup {
                t("iface.egress_backup").yellow().to_string()
            } else {
                "".to_string()
            };

            // 虚拟网卡类型用黄色
            let iftype_colored = if iftype_enum.is_virtual() {
                iftype_label.yellow().to_string()
            } else {
                iftype_label
            };

            vec![
                iface.name.clone(),
                iface.mac.clone(),
                iface.ipv4.clone(),
                status,
                iftype_colored,
                metric_str,
                egress,
            ]
        })
        .collect();

    print_table(&headers, &rows);

    let virtual_count = interfaces.iter().filter(|i| i.is_virtual).count();
    println!(
        "  {}",
        t2("iface.summary", &interfaces.len().to_string(), &virtual_count.to_string())
    );
}

/// 打印流量出口信息
pub fn print_egress(mode: OutputMode) {
    let interfaces = collect_interfaces();
    let egress = collect_egress(&interfaces);

    if mode == OutputMode::Json {
        print_json(&egress);
        return;
    }

    println!();
    println!("{}", t("egress.title").bold());

    match egress {
        Some(info) => {
            let iftype_enum = interface::classify_interface(&info.iftype, &info.interface);
            println!("  {}: {}", t("egress.iface"), info.interface.green());
            println!("  {}:    {}", t("egress.ip"), info.ip.yellow());
            println!("  {}:  {}", t("egress.type"), iftype_enum.to_label());
            println!("  {}:  {} ({})", t("egress.metric"), info.metric, t("egress.metric_hint"));
            println!();
            println!("  ┌─ {}", t("egress.logic_title"));
            println!("  │  {}", t("egress.logic_1"));
            println!("  │  {}", t("egress.logic_2"));
            println!("  │  {}", t("egress.logic_3"));
            println!("  │");
            println!(
                "  │  {}",
                t4(
                    "egress.logic_selected",
                    &info.interface,
                    "0",            // 路由跃点
                    &info.metric.to_string(), // 接口跃点
                    &info.metric.to_string()  // 有效跃点
                )
            );
            println!("  └─");
        }
        None => {
            println!("  {}", t("egress.unreachable").red());
        }
    }
}

/// 打印路由表
pub fn print_routes(mode: OutputMode) {
    let routes = get_route_table();

    if mode == OutputMode::Json {
        print_json(&routes);
        return;
    }

    println!();
    println!("{}", t("route.title").bold());
    let h_dest = t("route.dest");
    let h_gw = t("route.gateway");
    let h_iface = t("route.interface");
    let h_metric = t("route.metric");
    let headers = [h_dest.as_str(), h_gw.as_str(), h_iface.as_str(), h_metric.as_str()];
    let rows: Vec<Vec<String>> = routes
        .iter()
        .map(|r| {
            vec![
                r.destination.clone(),
                r.gateway.clone(),
                r.interface.clone(),
                r.metric.clone(),
            ]
        })
        .collect();
    print_table(&headers, &rows);
}

/// 打印代理设置
pub fn print_proxy(mode: OutputMode) {
    let proxies = get_proxy_info();

    if mode == OutputMode::Json {
        print_json(&proxies);
        return;
    }

    println!();
    println!("{}", t("proxy.title").bold());
    let h_type = t("proxy.type");
    let h_value = t("proxy.value");
    let headers = [h_type.as_str(), h_value.as_str()];
    let rows: Vec<Vec<String>> = proxies
        .iter()
        .map(|p| vec![p.ptype.clone(), p.value.clone()])
        .collect();
    print_table(&headers, &rows);
}

/// 打印全部网络信息
pub fn print_all(mode: OutputMode) {
    if mode == OutputMode::Json {
        let interfaces = collect_interfaces();
        let egress = collect_egress(&interfaces);
        let routes = get_route_table();
        let proxies = get_proxy_info();
        let all = AllInfo {
            interfaces,
            egress,
            routes,
            proxies,
        };
        print_json(&all);
        return;
    }

    print_banner(&t("banner.title"));
    print_interfaces(mode);
    print_egress(mode);
    print_routes(mode);
    print_proxy(mode);
    println!();
}
