//! 连通性测试模块：TCP 端口连通性 + HTTP 请求测试。

use std::time::{Duration, Instant};

use colored::*;
use serde::Serialize;

use crate::i18n::t;
use crate::output::{print_json, print_json_error, OutputMode};
use crate::table::print_table;

/// 分阶段耗时
#[derive(Serialize, Clone, Default)]
pub struct TimingBreakdown {
    pub dns_ms: f64,
    pub connect_ms: f64,
    pub tls_ms: f64,
    pub ttfb_ms: f64,
    pub total_ms: f64,
}

/// 单次测试结果
#[derive(Serialize, Clone)]
pub struct CheckProbe {
    pub success: bool,
    pub rtt_ms: f64,
    pub status_code: Option<u16>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<TimingBreakdown>,
}

/// 连通性测试完整输出
#[derive(Serialize)]
pub struct CheckOutput {
    pub target: String,
    pub check_type: String,
    pub probes: Vec<CheckProbe>,
    pub stats: CheckStats,
}

#[derive(Serialize, Clone)]
pub struct CheckStats {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub avg_ms: Option<f64>,
}

/// 执行连通性测试
#[allow(clippy::too_many_arguments)]
pub async fn run(
    target: &str,
    count: u32,
    timeout: Duration,
    timing: bool,
    proxy: Option<String>,
    no_proxy: bool,
    concurrency: usize,
    mode: OutputMode,
) {
    if target.starts_with("http://") || target.starts_with("https://") {
        run_http(
            target,
            count,
            timeout,
            timing,
            proxy,
            no_proxy,
            concurrency,
            mode,
        )
        .await;
    } else {
        run_tcp(target, count, timeout, mode).await;
    }
}

/// 解析 host:port（支持 IPv6 如 [::1]:443）
pub(crate) fn parse_host_port(target: &str) -> Option<(String, u16)> {
    if let Ok(addr) = target.parse::<std::net::SocketAddr>() {
        return Some((addr.ip().to_string(), addr.port()));
    }
    if let Some(idx) = target.rfind(':') {
        let host = &target[..idx];
        let port_str = &target[idx + 1..];
        if let Ok(port) = port_str.parse::<u16>() {
            let host = host.trim_start_matches('[').trim_end_matches(']');
            return Some((host.to_string(), port));
        }
    }
    None
}

/// 从代理 URL 提取 host 和 port
/// 支持 http://host:port, socks5://host:port, socks5h://host:port
fn parse_proxy_host_port(proxy_url: &str) -> Option<(String, u16)> {
    let rest = proxy_url.split("://").nth(1)?;
    let host_port = rest.split('/').next()?;
    let (host, port_str) = host_port.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port: u16 = port_str.parse().ok()?;
    Some((host.to_string(), port))
}

/// 从 URL 提取 host 和 port
fn parse_url(url: &str) -> Option<(String, u16, bool)> {
    let (scheme, rest) = url.split_once("://")?;
    let is_https = scheme == "https";
    let default_port = if is_https { 443 } else { 80 };
    let host = rest.split('/').next()?;
    let (host, port) = if let Some((h, p)) = host.rsplit_once(':') {
        (h.to_string(), p.parse().unwrap_or(default_port))
    } else {
        (host.to_string(), default_port)
    };
    Some((host, port, is_https))
}

/// TCP 连通性测试
async fn run_tcp(target: &str, count: u32, connect_timeout: Duration, mode: OutputMode) {
    use tokio::net::TcpStream;
    use tokio::time::timeout;

    let (host, port) = match parse_host_port(target) {
        Some(hp) => hp,
        None => {
            crate::output::mark_failure();
            if mode == OutputMode::Json {
                print_json_error(&t("check.format_err"));
            } else {
                println!("  {}", t("check.format_err").red());
            }
            return;
        }
    };

    let mut probes = Vec::new();

    for i in 0..count {
        let start = Instant::now();
        let addr = format!("{}:{}", host, port);
        let result = timeout(connect_timeout, TcpStream::connect(&addr)).await;
        let elapsed = start.elapsed();

        match result {
            Ok(Ok(_stream)) => {
                if mode == OutputMode::Table {
                    println!(
                        "  {}",
                        t("check.tcp_ok")
                            .replace("{0}", &(i + 1).to_string())
                            .replace("{1}", &count.to_string())
                            .replace("{2}", &format!("{:.2}", elapsed.as_secs_f64() * 1000.0))
                            .green()
                    );
                }
                probes.push(CheckProbe {
                    success: true,
                    rtt_ms: elapsed.as_secs_f64() * 1000.0,
                    status_code: None,
                    error: None,
                    timing: None,
                });
            }
            Ok(Err(e)) => {
                if mode == OutputMode::Table {
                    println!(
                        "  {}",
                        t("check.tcp_fail")
                            .replace("{0}", &(i + 1).to_string())
                            .replace("{1}", &count.to_string())
                            .replace("{2}", &e.to_string())
                            .red()
                    );
                }
                probes.push(CheckProbe {
                    success: false,
                    rtt_ms: elapsed.as_secs_f64() * 1000.0,
                    status_code: None,
                    error: Some(e.to_string()),
                    timing: None,
                });
            }
            Err(_) => {
                if mode == OutputMode::Table {
                    println!(
                        "  {}",
                        t("check.tcp_timeout")
                            .replace("{0}", &(i + 1).to_string())
                            .replace("{1}", &count.to_string())
                            .replace("{2}", &connect_timeout.as_secs().to_string())
                            .red()
                    );
                }
                probes.push(CheckProbe {
                    success: false,
                    rtt_ms: connect_timeout.as_secs_f64() * 1000.0,
                    status_code: None,
                    error: Some(t("check.req_timeout")),
                    timing: None,
                });
            }
        }

        if i + 1 < count {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    let stats = compute_stats(&probes);
    let output = CheckOutput {
        target: target.to_string(),
        check_type: "tcp".to_string(),
        probes: probes.clone(),
        stats: stats.clone(),
    };

    if output.stats.success == 0 {
        crate::output::mark_failure();
    }

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    print_stats(&stats, false);
}

/// HTTP 连通性测试（自动检测并使用系统代理）
#[allow(clippy::too_many_arguments)]
async fn run_http(
    url: &str,
    count: u32,
    connect_timeout: Duration,
    timing: bool,
    proxy: Option<String>,
    no_proxy: bool,
    concurrency: usize,
    mode: OutputMode,
) {
    // 确定代理：--proxy 优先 > --no-proxy 强制直连 > 系统自动检测
    let proxy_addr = if let Some(ref p) = proxy {
        Some(p.clone())
    } else if no_proxy {
        None
    } else {
        crate::util::get_system_proxy_for_url(url)
    };
    // --timing 仅在直连 HTTPS 时生效，有代理时静默忽略
    let can_breakdown = timing && proxy_addr.is_none() && url.starts_with("https://");

    // 如果指定了代理，先验证代理地址有效且端口可达，避免 reqwest 静默回退直连
    if let Some(ref proxy_url) = proxy_addr {
        match parse_proxy_host_port(proxy_url) {
            Some((host, port)) => {
                let addr = format!("{}:{}", host, port);
                match tokio::time::timeout(
                    Duration::from_secs(2),
                    tokio::net::TcpStream::connect(&addr),
                )
                .await
                {
                    Ok(Ok(_)) => {} // 代理端口可达，继续
                    _ => {
                        let msg = format!("代理不可达: {}", proxy_url);
                        crate::output::mark_failure();
                        if mode == OutputMode::Json {
                            print_json_error(&msg);
                        } else {
                            println!("  {}", format!("❌ {}", msg).red());
                        }
                        return;
                    }
                }
            }
            None => {
                // 代理地址格式无效（如端口超范围），直接报错
                let msg = format!("代理地址无效: {}", proxy_url);
                crate::output::mark_failure();
                if mode == OutputMode::Json {
                    print_json_error(&msg);
                } else {
                    println!("  {}", format!("❌ {}", msg).red());
                }
                return;
            }
        }
    }

    let mut builder = reqwest::Client::builder()
        .timeout(connect_timeout)
        .no_proxy();

    if let Some(ref proxy_url) = proxy_addr {
        if let Ok(proxy) = reqwest::Proxy::all(proxy_url) {
            builder = builder.proxy(proxy);
        }
    }

    let client = builder.build().unwrap();
    let proxy_tag = if proxy_addr.is_some() {
        format!(" [{}]", t("diagnose.via_proxy"))
    } else {
        String::new()
    };

    let is_concurrent = concurrency > 1;

    let mut probes = Vec::new();

    if is_concurrent {
        // 并发模式：用 semaphore 控制并发数
        use tokio::sync::Semaphore;
        let sem = std::sync::Arc::new(Semaphore::new(concurrency));
        let url = std::sync::Arc::new(url.to_string());
        let client = std::sync::Arc::new(client);

        let mut handles = Vec::new();
        for i in 0..count {
            let sem = sem.clone();
            let url = url.clone();
            let client = client.clone();

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.unwrap();
                if can_breakdown {
                    run_http_timing(&url, connect_timeout, OutputMode::Json, i, count).await
                } else {
                    run_http_single(&client, &url, i, count).await
                }
            }));
        }

        for handle in handles {
            if let Ok(probe) = handle.await {
                probes.push(probe);
            }
        }
    } else {
        // 串行模式
        for i in 0..count {
            if can_breakdown {
                let probe = run_http_timing(url, connect_timeout, mode, i, count).await;
                probes.push(probe);
            } else {
                let probe = run_http_single(&client, url, i, count).await;
                if mode == OutputMode::Table {
                    print_probe(&probe, i, count, &proxy_tag);
                }
                probes.push(probe);
            }

            if i + 1 < count {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    // 并发模式下打印结果（按序号排序）
    if is_concurrent && mode == OutputMode::Table {
        probes.sort_by_key(|p| p.rtt_ms as u64); // 按延迟排序展示
        for (i, probe) in probes.iter().enumerate() {
            print_probe(probe, i as u32, count, &proxy_tag);
        }
    }

    let stats = compute_stats(&probes);
    let output = CheckOutput {
        target: url.to_string(),
        check_type: if is_concurrent {
            "http-concurrent".to_string()
        } else {
            "http".to_string()
        },
        probes: probes.clone(),
        stats: stats.clone(),
    };

    if output.stats.success == 0 {
        crate::output::mark_failure();
    }

    if mode == OutputMode::Json {
        print_json(&output);
        return;
    }

    print_stats(&stats, true);

    // 并发模式额外显示并发统计
    if is_concurrent {
        print_concurrent_stats(&probes, concurrency);
    }
}

/// 单次 HTTP 请求（reqwest，返回 CheckProbe，不打印）
async fn run_http_single(client: &reqwest::Client, url: &str, _i: u32, _count: u32) -> CheckProbe {
    let start = Instant::now();
    let result = client.get(url).send().await;
    let elapsed = start.elapsed();

    match result {
        Ok(resp) => {
            let status_code = resp.status().as_u16();
            let is_success = resp.status().is_success();
            CheckProbe {
                success: is_success,
                rtt_ms: elapsed.as_secs_f64() * 1000.0,
                status_code: Some(status_code),
                error: None,
                timing: None,
            }
        }
        Err(e) => {
            let msg = if e.is_connect() {
                t("check.conn_fail")
            } else if e.is_timeout() {
                t("check.req_timeout")
            } else {
                e.to_string()
            };
            CheckProbe {
                success: false,
                rtt_ms: elapsed.as_secs_f64() * 1000.0,
                status_code: None,
                error: Some(msg),
                timing: None,
            }
        }
    }
}

/// 打印单次结果
fn print_probe(probe: &CheckProbe, i: u32, count: u32, proxy_tag: &str) {
    if probe.success {
        let status = probe.status_code.unwrap_or(0);
        let symbol = if (200..300).contains(&status) {
            "✓".green()
        } else {
            "⚠".yellow()
        };
        println!(
            "  [{}/{}] {} {}  {:.2}ms{}",
            i + 1,
            count,
            symbol,
            status,
            probe.rtt_ms,
            proxy_tag
        );
        if let Some(ref tm) = probe.timing {
            println!("    {:<10} {:.2}ms", t("check.timing_dns"), tm.dns_ms);
            println!(
                "    {:<10} {:.2}ms",
                t("check.timing_connect"),
                tm.connect_ms
            );
            println!("    {:<10} {:.2}ms", t("check.timing_tls"), tm.tls_ms);
            println!("    {:<10} {:.2}ms", t("check.timing_ttfb"), tm.ttfb_ms);
            println!("    {:<10} {:.2}ms", t("check.timing_total"), tm.total_ms);
        }
    } else {
        let msg = probe.error.as_deref().unwrap_or("unknown");
        println!(
            "  {}",
            format!(
                "[{}/{}] ✗ {}  {:.2}ms{}",
                i + 1,
                count,
                msg,
                probe.rtt_ms,
                proxy_tag
            )
            .red()
        );
    }
}

/// 打印并发统计
fn print_concurrent_stats(probes: &[CheckProbe], concurrency: usize) {
    println!();
    let success = probes.iter().filter(|p| p.success).count();
    let total = probes.len();
    let rtts: Vec<f64> = probes
        .iter()
        .filter(|p| p.success)
        .map(|p| p.rtt_ms)
        .collect();

    let h_metric = t("common.metric");
    let h_value = t("proxy.value");
    let headers = [h_metric.as_str(), h_value.as_str()];
    let mut rows = vec![
        vec![t("check.concurrency"), concurrency.to_string()],
        vec![t("check.total_reqs"), total.to_string()],
        vec![t("check.success_reqs"), success.to_string()],
        vec![t("check.fail_reqs"), (total - success).to_string()],
    ];

    if !rtts.is_empty() {
        let stats = crate::util::compute_stats(&rtts);
        if let (Some(min), Some(max), Some(avg)) = (stats.min_ms, stats.max_ms, stats.avg_ms) {
            rows.push(vec![t("ping.min"), format!("{:.2}ms", min)]);
            rows.push(vec![t("ping.max"), format!("{:.2}ms", max)]);
            rows.push(vec![t("ping.avg"), format!("{:.2}ms", avg)]);
            // QPS: 成功请求数 / 最大延迟（秒）
            let max_sec = max / 1000.0;
            if max_sec > 0.0 {
                let qps = success as f64 / max_sec;
                rows.push(vec![t("check.qps"), format!("{:.1}", qps)]);
            }
        }
    }

    print_table(&headers, &rows);
}

/// 手动分步计时 HTTPS 请求（DNS → TCP → TLS → TTFB）
async fn run_http_timing(
    url: &str,
    timeout: Duration,
    mode: OutputMode,
    i: u32,
    count: u32,
) -> CheckProbe {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio_rustls::TlsConnector;

    let (host, port, _) = match parse_url(url) {
        Some(hp) => hp,
        None => {
            return CheckProbe {
                success: false,
                rtt_ms: 0.0,
                status_code: None,
                error: Some("URL parse error".to_string()),
                timing: None,
            };
        }
    };

    let path = url
        .split("://")
        .nth(1)
        .and_then(|s| s.find('/').map(|idx| &s[idx..]))
        .unwrap_or("/");

    let t0 = Instant::now();

    // ① DNS
    let ips = crate::util::resolve_host_all(&host).await;
    if ips.is_empty() {
        return CheckProbe {
            success: false,
            rtt_ms: t0.elapsed().as_secs_f64() * 1000.0,
            status_code: None,
            error: Some("DNS resolve failed".to_string()),
            timing: None,
        };
    }
    let dns_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // ② TCP Connect
    let t1 = Instant::now();
    let mut last_tcp_error = None;
    let mut tcp_stream = None;
    for ip in ips.iter().copied().take(8) {
        let tcp_result = tokio::time::timeout(timeout, TcpStream::connect((ip, port))).await;
        match tcp_result {
            Ok(Ok(s)) => {
                tcp_stream = Some(s);
                break;
            }
            Ok(Err(e)) => last_tcp_error = Some(format!("TCP: {}", e)),
            Err(_) => last_tcp_error = Some("TCP: timeout".to_string()),
        }
    }
    let tcp_stream = match tcp_stream {
        Some(s) => s,
        None => {
            return CheckProbe {
                success: false,
                rtt_ms: t0.elapsed().as_secs_f64() * 1000.0,
                status_code: None,
                error: last_tcp_error.or_else(|| Some("TCP: failed".to_string())),
                timing: Some(TimingBreakdown {
                    dns_ms,
                    connect_ms: t1.elapsed().as_secs_f64() * 1000.0,
                    ..Default::default()
                }),
            }
        }
    };
    let connect_ms = t1.elapsed().as_secs_f64() * 1000.0;

    // ③ TLS Handshake
    let t2 = Instant::now();
    let root_store = crate::util::system_root_store();
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(root_store)
    .with_no_client_auth();
    let connector = TlsConnector::from(std::sync::Arc::new(config));

    let server_name = match rustls::pki_types::ServerName::try_from(host.clone()) {
        Ok(n) => n,
        Err(e) => {
            return CheckProbe {
                success: false,
                rtt_ms: t0.elapsed().as_secs_f64() * 1000.0,
                status_code: None,
                error: Some(format!("TLS: {}", e)),
                timing: Some(TimingBreakdown {
                    dns_ms,
                    connect_ms,
                    ..Default::default()
                }),
            };
        }
    };

    let tls_stream =
        match tokio::time::timeout(timeout, connector.connect(server_name, tcp_stream)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return CheckProbe {
                    success: false,
                    rtt_ms: t0.elapsed().as_secs_f64() * 1000.0,
                    status_code: None,
                    error: Some(format!("TLS: {}", e)),
                    timing: Some(TimingBreakdown {
                        dns_ms,
                        connect_ms,
                        tls_ms: t2.elapsed().as_secs_f64() * 1000.0,
                        ..Default::default()
                    }),
                };
            }
            Err(_) => {
                return CheckProbe {
                    success: false,
                    rtt_ms: t0.elapsed().as_secs_f64() * 1000.0,
                    status_code: None,
                    error: Some("TLS: timeout".to_string()),
                    timing: Some(TimingBreakdown {
                        dns_ms,
                        connect_ms,
                        tls_ms: t2.elapsed().as_secs_f64() * 1000.0,
                        ..Default::default()
                    }),
                };
            }
        };
    let tls_ms = t2.elapsed().as_secs_f64() * 1000.0;

    // ④ TTFB: 发送 HTTP GET，读取响应
    let t3 = Instant::now();
    let mut tls_stream = tls_stream;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: netutils/0.3\r\nConnection: close\r\n\r\n",
        path, host
    );

    match tokio::time::timeout(timeout, tls_stream.write_all(request.as_bytes())).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            return CheckProbe {
                success: false,
                rtt_ms: t0.elapsed().as_secs_f64() * 1000.0,
                status_code: None,
                error: Some("HTTP: write failed".to_string()),
                timing: Some(TimingBreakdown {
                    dns_ms,
                    connect_ms,
                    tls_ms,
                    ..Default::default()
                }),
            };
        }
        Err(_) => {
            return CheckProbe {
                success: false,
                rtt_ms: t0.elapsed().as_secs_f64() * 1000.0,
                status_code: None,
                error: Some("HTTP: write timeout".to_string()),
                timing: Some(TimingBreakdown {
                    dns_ms,
                    connect_ms,
                    tls_ms,
                    ..Default::default()
                }),
            };
        }
    }

    // 读取响应（至少读取状态行）
    let mut buf = [0u8; 4096];
    let n = match tokio::time::timeout(timeout, tls_stream.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            return CheckProbe {
                success: false,
                rtt_ms: t0.elapsed().as_secs_f64() * 1000.0,
                status_code: None,
                error: Some(format!("HTTP: {}", e)),
                timing: Some(TimingBreakdown {
                    dns_ms,
                    connect_ms,
                    tls_ms,
                    ..Default::default()
                }),
            };
        }
        Err(_) => {
            return CheckProbe {
                success: false,
                rtt_ms: t0.elapsed().as_secs_f64() * 1000.0,
                status_code: None,
                error: Some("HTTP: read timeout".to_string()),
                timing: Some(TimingBreakdown {
                    dns_ms,
                    connect_ms,
                    tls_ms,
                    ..Default::default()
                }),
            };
        }
    };
    let ttfb_ms = t3.elapsed().as_secs_f64() * 1000.0;
    let total_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // 解析状态码
    let response = String::from_utf8_lossy(&buf[..n]);
    let status_code = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let is_success = (200..300).contains(&status_code);

    let timing_bd = TimingBreakdown {
        dns_ms,
        connect_ms,
        tls_ms,
        ttfb_ms,
        total_ms,
    };

    if mode == OutputMode::Table {
        let symbol = if is_success {
            "✓".green()
        } else {
            "⚠".yellow()
        };
        println!(
            "  [{}/{}] {} {}  {:.2}ms",
            i + 1,
            count,
            symbol,
            status_code,
            total_ms
        );
        println!("    {:<10} {:.2}ms", t("check.timing_dns"), dns_ms);
        println!("    {:<10} {:.2}ms", t("check.timing_connect"), connect_ms);
        println!("    {:<10} {:.2}ms", t("check.timing_tls"), tls_ms);
        println!("    {:<10} {:.2}ms", t("check.timing_ttfb"), ttfb_ms);
        println!("    {:<10} {:.2}ms", t("check.timing_total"), total_ms);
    }

    CheckProbe {
        success: is_success,
        rtt_ms: total_ms,
        status_code: Some(status_code),
        error: None,
        timing: Some(timing_bd),
    }
}

/// 计算统计
fn compute_stats(probes: &[CheckProbe]) -> CheckStats {
    let total = probes.len();
    let success = probes.iter().filter(|p| p.success).count();
    let rtts: Vec<f64> = probes
        .iter()
        .filter(|p| p.success)
        .map(|p| p.rtt_ms)
        .collect();
    let stats = crate::util::compute_stats(&rtts);

    CheckStats {
        total,
        success,
        failed: total - success,
        min_ms: stats.min_ms,
        max_ms: stats.max_ms,
        avg_ms: stats.avg_ms,
    }
}

/// 打印统计
fn print_stats(stats: &CheckStats, is_http: bool) {
    println!();
    println!("{}", t("ping.stats").bold());

    let h_metric = t("common.metric");
    let h_value = t("proxy.value");
    let headers = [h_metric.as_str(), h_value.as_str()];
    let mut rows = Vec::new();
    rows.push(vec![t("check.count"), stats.total.to_string()]);

    if is_http {
        rows.push(vec![t("check.ok_2xx"), stats.success.to_string()]);
    } else {
        rows.push(vec![t("check.ok"), stats.success.to_string()]);
    }
    rows.push(vec![t("check.fail_count"), stats.failed.to_string()]);

    if let (Some(min), Some(max), Some(avg)) = (stats.min_ms, stats.max_ms, stats.avg_ms) {
        rows.push(vec![t("ping.min"), format!("{:.2}ms", min)]);
        rows.push(vec![t("ping.max"), format!("{:.2}ms", max)]);
        rows.push(vec![t("ping.avg"), format!("{:.2}ms", avg)]);
    }

    print_table(&headers, &rows);
}
