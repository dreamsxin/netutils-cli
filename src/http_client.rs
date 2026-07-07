//! Lightweight HTTP request diagnostics.

use std::time::{Duration, Instant};

use colored::*;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url};
use serde::Serialize;

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
    mode: OutputMode,
) {
    let url = normalize_url(input_url);
    let method = match Method::from_bytes(method.as_bytes()) {
        Ok(method) => method,
        Err(err) => {
            output(
                error_report(input_url, &url, method, format!("invalid method: {err}")),
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
            mode,
        );
        return;
    }

    let headers = match parse_headers(&raw_headers) {
        Ok(headers) => headers,
        Err(err) => {
            output(error_report(input_url, &url, method.as_str(), err), mode);
            return;
        }
    };
    let request_headers = headers_to_vec(&headers);
    let request_body_bytes = body.as_ref().map(|body| body.as_bytes().len()).unwrap_or(0);

    let proxy_value = if no_proxy {
        None
    } else {
        proxy.or_else(crate::util::get_system_proxy_addr)
    };
    let proxy_info = HttpProxy {
        mode: if no_proxy {
            "direct-forced".to_string()
        } else if proxy_value.is_some() {
            "proxy".to_string()
        } else {
            "direct".to_string()
        },
        value: proxy_value.clone(),
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
            value: value.to_str().unwrap_or("<non-utf8>").to_string(),
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
        notes: vec![
            "Use --show-headers to include response headers.".to_string(),
            "Use --body-limit 0 to skip response body preview.".to_string(),
        ],
    }
}

fn output(report: HttpReport, mode: OutputMode) {
    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
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
