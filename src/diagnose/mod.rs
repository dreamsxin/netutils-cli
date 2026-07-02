//! 全链路诊断模块：DNS → Ping → TCP → HTTPS → Traceroute，自动定位断点。

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;

use crate::i18n::{t, t1};
use crate::output::{print_json, OutputMode};

/// 单步检测结果
#[derive(Serialize, Clone)]
struct DiagStep {
    check: String,
    ok: bool,
    warning: bool,
    message: String,
}

/// 完整诊断报告
#[derive(Serialize)]
struct DiagnoseReport {
    host: String,
    target: String,
    steps: Vec<DiagStep>,
    conclusion: String,
    elapsed_secs: f64,
}

const PING_COUNT: u32 = 2;
const TCP_TIMEOUT: Duration = Duration::from_secs(3);
const HTTPS_TIMEOUT: Duration = Duration::from_secs(5);
const TRACE_MAX_HOPS: u32 = 10;
const TRACE_HOP_TIMEOUT: Duration = Duration::from_millis(800);
const TRACE_PROBE_DEADLINE: Duration = Duration::from_secs(1);
const TRACE_MAX_CONSECUTIVE_TIMEOUT_HOPS: u32 = 3;
const HTTPS_ATTEMPTS: u32 = 3;

/// 执行全链路诊断
pub async fn run(host: &str, mode: OutputMode) {
    let start = Instant::now();

    // 预解析 IP（供后续步骤复用，避免多 A 记录域名在各步骤解析到不同地址）
    let dns_start = Instant::now();
    let target_ips = crate::util::resolve_host_all(host).await;
    let dns = build_dns_step(host, &target_ips, dns_start.elapsed());
    let target_ip = target_ips.first().copied();

    // DNS 之后的步骤并行执行
    let (ping, tcp, https, trace) = tokio::join!(
        check_ping(host, target_ip),
        check_tcp(host, &target_ips),
        check_https(host),
        check_trace(target_ip),
    );

    let steps = vec![dns, ping, tcp, https, trace];
    let conclusion = derive_conclusion(&steps);

    let elapsed = start.elapsed();
    let target = target_ip
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "N/A".to_string());

    let report = DiagnoseReport {
        host: host.to_string(),
        target: target.clone(),
        steps: steps.clone(),
        conclusion: conclusion.clone(),
        elapsed_secs: elapsed.as_secs_f64(),
    };

    if mode == OutputMode::Json {
        print_json(&report);
        return;
    }

    // 表格输出
    println!();
    println!("{}", t1("diagnose.title", host).bold());
    println!();

    for step in &steps {
        let symbol = if step.ok && !step.warning {
            "✅".green()
        } else if step.warning {
            "⚠️ ".yellow()
        } else {
            "❌".red()
        };
        println!("  {} [{}]", symbol, step.check.dimmed());
        println!("     {}", step.message);
    }

    // 结论
    println!();
    println!("  {}", t1("diagnose.conclusion", &conclusion).bold());

    // 链路状态链
    let chain = build_chain(&steps);
    println!("  {}", t1("diagnose.conclusion_chain", &chain).dimmed());

    println!();
    println!(
        "  {}",
        t1("diagnose.elapsed", &format!("{:.1}", elapsed.as_secs_f64()))
    );
}

// ═══════════════════════════════════════════════════════════════
//  检测步骤
// ═══════════════════════════════════════════════════════════════

/// ① DNS 解析
fn build_dns_step(host: &str, ips: &[IpAddr], elapsed: Duration) -> DiagStep {
    let check = t("diagnose.step_dns");

    match ips.first() {
        Some(ip) => {
            let elapsed = elapsed.as_secs_f64() * 1000.0;
            let msg = t("diagnose.dns_ok")
                .replace("{0}", host)
                .replace("{1}", &ip.to_string())
                .replace("{2}", &format!("{:.0}", elapsed));
            DiagStep {
                check,
                ok: true,
                warning: false,
                message: msg,
            }
        }
        None => {
            let msg = t1("diagnose.dns_fail", host);
            DiagStep {
                check,
                ok: false,
                warning: false,
                message: msg,
            }
        }
    }
}

/// ② Ping 探测
async fn check_ping(host: &str, target_ip: Option<IpAddr>) -> DiagStep {
    let check = t("diagnose.step_ping");

    let target = match target_ip {
        Some(ip) => ip,
        None => {
            return DiagStep {
                check,
                ok: false,
                warning: false,
                message: t1("diagnose.dns_fail", host),
            };
        }
    };

    // ICMP 优先，失败回退 TCP
    let probes =
        match crate::ping::surge_ping_probe(target, PING_COUNT, Duration::from_secs(2)).await {
            Some(r) => r,
            None => crate::ping::tcp_ping_probe(target, PING_COUNT, Duration::from_secs(2)).await,
        };

    let success_count = probes.iter().filter(|p| p.success).count();
    let total = probes.len();
    let loss_rate = if total > 0 {
        ((total - success_count) as f64 / total as f64) * 100.0
    } else {
        100.0
    };

    let rtts: Vec<f64> = probes.iter().filter_map(|p| p.rtt_ms).collect();
    let stats = crate::util::compute_stats(&rtts);
    let avg = stats.avg_ms.unwrap_or(0.0);

    if success_count == 0 {
        let msg = t("diagnose.ping_fail").replace("{0}", &target.to_string());
        DiagStep {
            check,
            ok: false,
            warning: false,
            message: msg,
        }
    } else if loss_rate > 0.0 {
        let msg = t("diagnose.ping_ok")
            .replace("{0}", &target.to_string())
            .replace("{1}", &format!("{:.0}", avg))
            .replace("{2}", &format!("{:.0}", loss_rate));
        DiagStep {
            check,
            ok: true,
            warning: true,
            message: msg,
        }
    } else {
        let msg = t("diagnose.ping_ok")
            .replace("{0}", &target.to_string())
            .replace("{1}", &format!("{:.0}", avg))
            .replace("{2}", &format!("{:.0}", loss_rate));
        DiagStep {
            check,
            ok: true,
            warning: false,
            message: msg,
        }
    }
}

/// ③ TCP 端口 443
async fn check_tcp(host: &str, target_ips: &[IpAddr]) -> DiagStep {
    let check = t1("diagnose.step_tcp", "443/80");

    match tcp_connect_any(target_ips, 443).await {
        Some((addr, elapsed)) => {
            let msg = t("diagnose.tcp_ok_detail")
                .replace("{0}", "443")
                .replace("{1}", &addr.to_string())
                .replace("{2}", &format!("{:.0}", elapsed.as_secs_f64() * 1000.0));
            DiagStep {
                check,
                ok: true,
                warning: false,
                message: msg,
            }
        }
        None => match tcp_connect_any(target_ips, 80).await {
            Some((addr, elapsed)) => {
                let msg = t("diagnose.tcp_fallback_ok")
                    .replace("{0}", "443")
                    .replace("{1}", "80")
                    .replace("{2}", &addr.to_string())
                    .replace("{3}", &format!("{:.0}", elapsed.as_secs_f64() * 1000.0));
                DiagStep {
                    check,
                    ok: true,
                    warning: true,
                    message: msg,
                }
            }
            None => {
                let target = if target_ips.is_empty() {
                    host.to_string()
                } else {
                    target_ips
                        .iter()
                        .map(IpAddr::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let msg = t("diagnose.tcp_fail_targets")
                    .replace("{0}", &target)
                    .replace("{1}", &TCP_TIMEOUT.as_secs().to_string());
                DiagStep {
                    check,
                    ok: false,
                    warning: false,
                    message: msg,
                }
            }
        },
    }
}

async fn tcp_connect_any(target_ips: &[IpAddr], port: u16) -> Option<(SocketAddr, Duration)> {
    let mut tasks = tokio::task::JoinSet::new();
    for ip in target_ips.iter().copied().take(8) {
        tasks.spawn(async move {
            let addr = SocketAddr::new(ip, port);
            let start = Instant::now();
            let result =
                tokio::time::timeout(TCP_TIMEOUT, tokio::net::TcpStream::connect(addr)).await;
            match result {
                Ok(Ok(_)) => Some((addr, start.elapsed())),
                Ok(Err(_)) | Err(_) => None,
            }
        });
    }

    while let Some(result) = tasks.join_next().await {
        if let Ok(Some(success)) = result {
            tasks.abort_all();
            return Some(success);
        }
    }

    None
}

/// ④ HTTPS 请求
async fn check_https(host: &str) -> DiagStep {
    let check = t("diagnose.step_https");
    let url = format!("https://{}", host);

    // 检测系统代理
    let proxy_addr = crate::util::get_system_proxy_addr();
    let via_proxy = proxy_addr.is_some();
    let proxy_tag = if via_proxy {
        t("diagnose.via_proxy")
    } else {
        t("diagnose.no_proxy")
    };

    let mut builder = reqwest::Client::builder().timeout(HTTPS_TIMEOUT).no_proxy();
    if let Some(ref proxy_url) = proxy_addr {
        if let Ok(proxy) = reqwest::Proxy::all(proxy_url) {
            builder = builder.proxy(proxy);
        }
    }
    let client = builder.build().unwrap();

    let start = Instant::now();
    let mut last_https_error = None;
    for _ in 0..HTTPS_ATTEMPTS {
        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                let is_success = resp.status().is_success();
                let msg = t("diagnose.https_ok")
                    .replace("{0}", &url)
                    .replace("{1}", &status.to_string())
                    .replace("{2}", &format!("{:.0}", elapsed))
                    .replace("{3}", &proxy_tag);
                return DiagStep {
                    check,
                    ok: is_success,
                    warning: !is_success,
                    message: msg,
                };
            }
            Err(e) => {
                last_https_error = Some(e.to_string());
            }
        }
    }

    let https_error = last_https_error.unwrap_or_else(|| "request failed".to_string());
    let http_url = format!("http://{}", host);
    let http_start = Instant::now();
    match client.get(&http_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let status = resp.status().as_u16();
            let elapsed = http_start.elapsed().as_secs_f64() * 1000.0;
            let msg = t("diagnose.https_http_fallback_ok")
                .replace("{0}", &https_error)
                .replace("{1}", &http_url)
                .replace("{2}", &status.to_string())
                .replace("{3}", &format!("{:.0}", elapsed))
                .replace("{4}", &proxy_tag);
            DiagStep {
                check,
                ok: true,
                warning: true,
                message: msg,
            }
        }
        _ => {
            let msg = t("diagnose.https_fail")
                .replace("{0}", &https_error)
                .replace("{1}", &proxy_tag);
            DiagStep {
                check,
                ok: false,
                warning: false,
                message: msg,
            }
        }
    }
}

/// ⑤ Traceroute
async fn check_trace(target_ip: Option<IpAddr>) -> DiagStep {
    let check = t1("diagnose.step_trace", &TRACE_MAX_HOPS.to_string());

    let target = match target_ip {
        Some(ip) => ip,
        None => {
            return DiagStep {
                check,
                ok: false,
                warning: true,
                message: t("diagnose.trace_skip"),
            };
        }
    };

    // IPv4 only（traceroute 不支持 IPv6）
    let target_v4 = match target {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(_) => {
            return DiagStep {
                check,
                ok: false,
                warning: true,
                message: t("diagnose.trace_skip"),
            };
        }
    };

    // 尝试创建 raw socket，失败则跳过
    use socket2::{Domain, Protocol, Socket, Type};
    let test_socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4));
    if test_socket.is_err() {
        return DiagStep {
            check,
            ok: false,
            warning: true,
            message: t("diagnose.trace_skip"),
        };
    }
    drop(test_socket);

    // 执行 traceroute（最多 TRACE_MAX_HOPS 跳，每跳 2 次探测）
    let mut hops_reached = 0u32;
    let mut reached = false;
    let mut consecutive_timeout_hops = 0;

    for ttl in 1..=TRACE_MAX_HOPS {
        let hop = trace_hop_simple(target_v4, ttl).await;
        hops_reached = ttl;
        if hop.reached {
            reached = true;
            break;
        }
        if hop.responded {
            consecutive_timeout_hops = 0;
        } else {
            consecutive_timeout_hops += 1;
            if consecutive_timeout_hops >= TRACE_MAX_CONSECUTIVE_TIMEOUT_HOPS {
                break;
            }
        }
    }

    if reached {
        let msg = t("diagnose.trace_reached").replace("{0}", &hops_reached.to_string());
        DiagStep {
            check,
            ok: true,
            warning: false,
            message: msg,
        }
    } else {
        let msg = t("diagnose.trace_not_reached").replace("{0}", &TRACE_MAX_HOPS.to_string());
        DiagStep {
            check,
            ok: false,
            warning: true,
            message: msg,
        }
    }
}

/// 简化版单跳探测（复用 traceroute 逻辑）
struct SimpleHop {
    responded: bool,
    reached: bool,
}

/// 简化版 trace_hop：发 1 个探测包判断是否到达
async fn trace_hop_simple(target: std::net::Ipv4Addr, ttl: u32) -> SimpleHop {
    tokio::time::timeout(
        TRACE_PROBE_DEADLINE,
        tokio::task::spawn_blocking(move || trace_hop_simple_blocking(target, ttl)),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or(SimpleHop {
        responded: false,
        reached: false,
    })
}

fn trace_hop_simple_blocking(target: std::net::Ipv4Addr, ttl: u32) -> SimpleHop {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::mem::MaybeUninit;
    use std::net::{IpAddr, SocketAddr};

    let socket = match Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)) {
        Ok(s) => s,
        Err(_) => {
            return SimpleHop {
                responded: false,
                reached: false,
            }
        }
    };
    let _ = socket.set_ttl_v4(ttl);
    let _ = socket.set_read_timeout(Some(TRACE_HOP_TIMEOUT));

    let ident = (std::process::id() & 0xFFFF) as u16;
    let seq = (ttl * 10) as u16;
    let packet = crate::icmp::build_icmp_echo_request(ident, seq);

    let dest = SocketAddr::new(IpAddr::V4(target), 0);
    if socket.send_to(&packet, &dest.into()).is_err() {
        return SimpleHop {
            responded: false,
            reached: false,
        };
    }

    let start = Instant::now();
    let mut buf = [MaybeUninit::new(0); 1024];
    while start.elapsed() < TRACE_HOP_TIMEOUT {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                let from_ip = match from.as_socket() {
                    Some(s) => s.ip(),
                    None => continue,
                };
                let data: &[u8] =
                    unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, len) };
                if crate::icmp::parse_icmp_response(data, ident, seq).is_some() {
                    return SimpleHop {
                        responded: true,
                        reached: from_ip == IpAddr::V4(target),
                    };
                }
            }
            Err(_) => {
                return SimpleHop {
                    responded: false,
                    reached: false,
                }
            }
        }
    }

    SimpleHop {
        responded: false,
        reached: false,
    }
}

// ═══════════════════════════════════════════════════════════════
//  结论推导
// ═══════════════════════════════════════════════════════════════

/// 根据各步骤状态自动推导结论
fn derive_conclusion(steps: &[DiagStep]) -> String {
    // steps: [dns, ping, tcp, https, trace]
    let dns_ok = steps.get(0).map(|s| s.ok).unwrap_or(false);
    let ping_ok = steps.get(1).map(|s| s.ok).unwrap_or(false);
    let tcp_ok = steps.get(2).map(|s| s.ok).unwrap_or(false);
    let https_ok = steps.get(3).map(|s| s.ok).unwrap_or(false);

    if !dns_ok {
        return t("diagnose.conclusion_dns");
    }
    if !ping_ok {
        return t("diagnose.conclusion_ping");
    }
    if !tcp_ok {
        return t("diagnose.conclusion_tcp");
    }
    if !https_ok {
        return t("diagnose.conclusion_https");
    }
    t("diagnose.conclusion_healthy")
}

/// 构建链路状态链（DNS → Ping → TCP → HTTPS → Trace）
fn build_chain(steps: &[DiagStep]) -> String {
    let labels = ["DNS", "Ping", "TCP", "HTTPS", "Trace"];
    let arrow = " → ";

    steps
        .iter()
        .take(5)
        .enumerate()
        .map(|(i, step)| {
            let label = labels.get(i).unwrap_or(&"?");
            if step.ok {
                format!("✅ {}", label)
            } else if step.warning {
                format!("⚠️ {}", label)
            } else {
                format!("❌ {}", label)
            }
        })
        .collect::<Vec<_>>()
        .join(arrow)
}
