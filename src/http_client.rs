//! Lightweight HTTP request diagnostics.

use std::time::{Duration, Instant};

use colored::*;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url};
use serde::Serialize;

use crate::assertion::{self, Assertion, AssertionReport, Metrics};
use crate::output::{print_json, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
pub struct HttpReport {
    pub input_url: String,
    pub url: String,
    pub method: String,
    pub proxy: HttpProxy,
    pub request_headers: Vec<HttpHeader>,
    pub request_body_bytes: usize,
    pub response: HttpResponse,
    pub timings: HttpTimings,
    /// `--assert` 判定结果；未传断言时不出现在 JSON 中
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assertions: Option<AssertionReport>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct HttpProxy {
    pub mode: String,
    pub value: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct HttpResponse {
    pub ok: bool,
    pub status: Option<u16>,
    pub final_url: Option<String>,
    pub headers: Vec<HttpHeader>,
    pub body_preview: Option<String>,
    pub body_bytes: usize,
    pub body_truncated: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HttpTimings {
    pub total_ms: f64,
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    input_url: &str,
    method: &str,
    raw_headers: Vec<String>,
    body: Option<String>,
    timeout: Duration,
    proxy: Option<String>,
    no_proxy: bool,
    show_headers: bool,
    body_limit: usize,
    assertions: &[Assertion],
    mode: OutputMode,
) {
    let url = normalize_url(input_url);
    let method = match Method::from_bytes(method.as_bytes()) {
        Ok(method) => method,
        Err(err) => {
            output(
                error_report(input_url, &url, method, format!("invalid method: {err}")),
                assertions,
                mode,
            );
            return;
        }
    };
    if let Err(err) = Url::parse(&url) {
        output(
            error_report(
                input_url,
                &url,
                method.as_str(),
                format!("invalid URL: {err}"),
            ),
            assertions,
            mode,
        );
        return;
    }

    let headers = match parse_headers(&raw_headers) {
        Ok(headers) => headers,
        Err(err) => {
            output(
                error_report(input_url, &url, method.as_str(), err),
                assertions,
                mode,
            );
            return;
        }
    };
    let request_headers = headers_to_vec(&headers);
    let request_body_bytes = body.as_ref().map(|body| body.len()).unwrap_or(0);

    let proxy_value = if no_proxy {
        None
    } else {
        proxy.or_else(|| crate::util::get_system_proxy_for_url(&url))
    };
    let proxy_info = HttpProxy {
        mode: if no_proxy {
            "direct-forced".to_string()
        } else if proxy_value.is_some() {
            "proxy".to_string()
        } else {
            "direct".to_string()
        },
        value: proxy_value
            .as_deref()
            .map(crate::util::redact_url_credentials),
    };

    let client = match build_client(timeout, proxy_value.as_deref()) {
        Ok(client) => client,
        Err(err) => {
            output(
                report_with_response(
                    input_url,
                    &url,
                    method.as_str(),
                    proxy_info,
                    request_headers,
                    request_body_bytes,
                    failed_response(format!("failed to build client: {err}")),
                    0.0,
                ),
                assertions,
                mode,
            );
            return;
        }
    };

    let start = Instant::now();
    let mut request = client.request(method.clone(), &url).headers(headers);
    if let Some(body) = body {
        request = request.body(body);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            output(
                report_with_response(
                    input_url,
                    &url,
                    method.as_str(),
                    proxy_info,
                    request_headers,
                    request_body_bytes,
                    failed_response(err.to_string()),
                    start.elapsed().as_secs_f64() * 1000.0,
                ),
                assertions,
                mode,
            );
            return;
        }
    };

    let status = response.status();
    let final_url = response.url().to_string();
    let response_headers = if show_headers {
        headers_to_vec(response.headers())
    } else {
        Vec::new()
    };
    let response = read_response_body(response, status.as_u16(), final_url, body_limit).await;

    output(
        report_with_response(
            input_url,
            &url,
            method.as_str(),
            proxy_info,
            request_headers,
            request_body_bytes,
            HttpResponse {
                headers: response_headers,
                ..response
            },
            start.elapsed().as_secs_f64() * 1000.0,
        ),
        assertions,
        mode,
    );
}

fn normalize_url(input: &str) -> String {
    if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    }
}

fn build_client(timeout: Duration, proxy: Option<&str>) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("netutils http");
    if let Some(proxy_url) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy_url)?);
    } else {
        builder = builder.no_proxy();
    }
    builder.build()
}

fn parse_headers(raw_headers: &[String]) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for raw in raw_headers {
        let Some((name, value)) = raw.split_once(':') else {
            return Err(format!("invalid header, expected 'Name: value': {raw}"));
        };
        let name = HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|err| format!("invalid header name '{name}': {err}"))?;
        let value = HeaderValue::from_str(value.trim())
            .map_err(|err| format!("invalid header value for '{name}': {err}"))?;
        headers.append(name, value);
    }
    Ok(headers)
}

async fn read_response_body(
    mut response: reqwest::Response,
    status: u16,
    final_url: String,
    body_limit: usize,
) -> HttpResponse {
    if body_limit == 0 {
        return HttpResponse {
            ok: (200..400).contains(&status),
            status: Some(status),
            final_url: Some(final_url),
            headers: Vec::new(),
            body_preview: None,
            body_bytes: 0,
            body_truncated: false,
            error: None,
        };
    }

    let mut collected = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;
    while let Ok(Some(chunk)) = response.chunk().await {
        total += chunk.len();
        if collected.len() < body_limit {
            let remaining = body_limit - collected.len();
            let take = remaining.min(chunk.len());
            collected.extend_from_slice(&chunk[..take]);
            if take < chunk.len() {
                truncated = true;
                break;
            }
        } else {
            truncated = true;
            break;
        }
    }

    HttpResponse {
        ok: (200..400).contains(&status),
        status: Some(status),
        final_url: Some(final_url),
        headers: Vec::new(),
        body_preview: Some(String::from_utf8_lossy(&collected).to_string()),
        body_bytes: total,
        body_truncated: truncated,
        error: None,
    }
}

fn headers_to_vec(headers: &HeaderMap) -> Vec<HttpHeader> {
    headers
        .iter()
        .map(|(name, value)| HttpHeader {
            name: name.to_string(),
            value: crate::util::redact_header_value(
                name.as_str(),
                value.to_str().unwrap_or("<non-utf8>"),
            ),
        })
        .collect()
}

fn failed_response(error: String) -> HttpResponse {
    HttpResponse {
        ok: false,
        status: None,
        final_url: None,
        headers: Vec::new(),
        body_preview: None,
        body_bytes: 0,
        body_truncated: false,
        error: Some(error),
    }
}

fn error_report(input_url: &str, url: &str, method: &str, error: String) -> HttpReport {
    report_with_response(
        input_url,
        url,
        method,
        HttpProxy {
            mode: "not-built".to_string(),
            value: None,
        },
        Vec::new(),
        0,
        failed_response(error),
        0.0,
    )
}

#[allow(clippy::too_many_arguments)]
fn report_with_response(
    input_url: &str,
    url: &str,
    method: &str,
    proxy: HttpProxy,
    request_headers: Vec<HttpHeader>,
    request_body_bytes: usize,
    response: HttpResponse,
    total_ms: f64,
) -> HttpReport {
    HttpReport {
        input_url: input_url.to_string(),
        url: url.to_string(),
        method: method.to_string(),
        proxy,
        request_headers,
        request_body_bytes,
        response,
        timings: HttpTimings { total_ms },
        assertions: None,
        notes: vec![
            "Use --show-headers to include response headers.".to_string(),
            "Use --body-limit 0 to skip response body preview.".to_string(),
        ],
    }
}

/// 把报告映射成断言可用的指标集合。
///
/// 支持但本次取不到的指标显式声明为 unavailable，避免与「指标名写错」混淆。
fn metrics(report: &HttpReport) -> Metrics {
    let mut metrics = Metrics::new();
    metrics
        .num("latency_ms", report.timings.total_ms)
        .num("body_bytes", report.response.body_bytes as f64)
        .num("ok", if report.response.ok { 1.0 } else { 0.0 });
    match report.response.status {
        Some(status) => metrics.num("status", f64::from(status)),
        None => metrics.unavailable("status", "no HTTP response was received"),
    };
    match &report.response.final_url {
        Some(final_url) => metrics.text("final_url", final_url.clone()),
        None => metrics.unavailable("final_url", "no HTTP response was received"),
    };
    match &report.response.body_preview {
        Some(body) => metrics.text("body", body.clone()),
        None => metrics.unavailable("body", "body preview is disabled or empty"),
    };
    match &report.response.error {
        Some(error) => metrics.text("error", error.clone()),
        None => metrics.text("error", ""),
    };
    metrics
}

fn output(mut report: HttpReport, assertions: &[Assertion], mode: OutputMode) {
    if !report.response.ok {
        crate::output::mark_failure();
    }
    if !assertions.is_empty() {
        let evaluated = assertion::evaluate(assertions, &metrics(&report));
        assertion::mark_exit_code(&evaluated);
        report.assertions = Some(evaluated);
    }
    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
        if let Some(evaluated) = &report.assertions {
            assertion::print_report(evaluated);
        }
    }
}

fn print_report(report: &HttpReport) {
    println!();
    println!("{}", "🌐 HTTP Request".bold());
    println!("  URL: {}", report.url);
    println!("  Method: {}", report.method);
    println!(
        "  Proxy: {}{}",
        report.proxy.mode,
        report
            .proxy
            .value
            .as_ref()
            .map(|value| format!(" ({value})"))
            .unwrap_or_default()
    );

    println!();
    println!("{}", "Response".bold());
    println!(
        "  Result: {}",
        if report.response.ok {
            "ok".green().to_string()
        } else {
            "failed".red().to_string()
        }
    );
    println!(
        "  Status: {}",
        report
            .response
            .status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "--".to_string())
    );
    if let Some(final_url) = &report.response.final_url {
        println!("  Final URL: {}", final_url);
    }
    if let Some(error) = &report.response.error {
        println!("  Error: {}", error);
    }
    println!("  Time: {:.2}ms", report.timings.total_ms);
    println!("  Body Bytes Read: {}", report.response.body_bytes);
    println!(
        "  Body Truncated: {}",
        if report.response.body_truncated {
            "yes"
        } else {
            "no"
        }
    );

    if !report.response.headers.is_empty() {
        println!();
        println!("{}", "Headers".bold());
        let rows = report
            .response
            .headers
            .iter()
            .map(|header| vec![header.name.clone(), header.value.clone()])
            .collect::<Vec<_>>();
        print_table(&["Name", "Value"], &rows);
    }

    if let Some(body) = &report.response.body_preview {
        if !body.is_empty() {
            println!();
            println!("{}", "Body Preview".bold());
            println!("{}", body);
        }
    }

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_url_without_scheme() {
        assert_eq!(normalize_url("example.com"), "https://example.com");
    }

    #[test]
    fn parses_headers() {
        let headers = parse_headers(&[
            "Accept: application/json".to_string(),
            "X-Test: yes".to_string(),
        ])
        .unwrap();
        assert_eq!(headers.get("accept").unwrap(), "application/json");
        assert_eq!(headers.get("x-test").unwrap(), "yes");
    }

    #[test]
    fn rejects_invalid_header() {
        assert!(parse_headers(&["broken".to_string()]).is_err());
    }
}
