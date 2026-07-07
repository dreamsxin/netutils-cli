//! Server-Sent Events diagnostics.

use std::time::{Duration, Instant};

use colored::*;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT};
use serde::Serialize;

use crate::output::{print_json, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
pub struct SseReport {
    pub input_url: String,
    pub url: String,
    pub proxy: SseProxy,
    pub status: Option<u16>,
    pub content_type: Option<String>,
    pub connected: bool,
    pub events: Vec<SseEvent>,
    pub error: Option<String>,
    pub elapsed_ms: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SseProxy {
    pub mode: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SseEvent {
    pub index: usize,
    pub event: Option<String>,
    pub id: Option<String>,
    pub retry: Option<String>,
    pub data: String,
}

#[derive(Default)]
struct SseEventBuilder {
    event: Option<String>,
    id: Option<String>,
    retry: Option<String>,
    data_lines: Vec<String>,
}

pub async fn run(
    input_url: &str,
    raw_headers: Vec<String>,
    timeout: Duration,
    max_events: usize,
    max_duration: Duration,
    proxy: Option<String>,
    no_proxy: bool,
    mode: OutputMode,
) {
    let start = Instant::now();
    let url = normalize_url(input_url);
    let headers = match parse_headers(&raw_headers) {
        Ok(headers) => headers,
        Err(err) => {
            output(error_report(input_url, &url, err, start.elapsed()), mode);
            return;
        }
    };
    let proxy_value = if no_proxy {
        None
    } else {
        proxy.or_else(crate::util::get_system_proxy_addr)
    };
    let proxy_info = SseProxy {
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
                report(
                    input_url,
                    &url,
                    proxy_info,
                    None,
                    None,
                    false,
                    Vec::new(),
                    Some(format!("failed to build client: {err}")),
                    start.elapsed(),
                ),
                mode,
            );
            return;
        }
    };

    let response = match tokio::time::timeout(
        timeout,
        client
            .get(&url)
            .headers(headers)
            .header(ACCEPT, "text/event-stream")
            .send(),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => {
            output(
                report(
                    input_url,
                    &url,
                    proxy_info,
                    None,
                    None,
                    false,
                    Vec::new(),
                    Some(err.to_string()),
                    start.elapsed(),
                ),
                mode,
            );
            return;
        }
        Err(_) => {
            output(
                report(
                    input_url,
                    &url,
                    proxy_info,
                    None,
                    None,
                    false,
                    Vec::new(),
                    Some("connect timeout".to_string()),
                    start.elapsed(),
                ),
                mode,
            );
            return;
        }
    };

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut response = response;
    let mut parser = SseParser::default();
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + max_duration;
    let mut error = None;

    while events.len() < max_events && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                events.extend(parser.push(&chunk));
                if events.len() > max_events {
                    events.truncate(max_events);
                }
            }
            Ok(Ok(None)) => {
                events.extend(parser.finish());
                break;
            }
            Ok(Err(err)) => {
                error = Some(err.to_string());
                break;
            }
            Err(_) => break,
        }
    }

    output(
        report(
            input_url,
            &url,
            proxy_info,
            Some(status),
            content_type,
            true,
            renumber(events),
            error,
            start.elapsed(),
        ),
        mode,
    );
}

#[derive(Default)]
struct SseParser {
    buffer: String,
    current: SseEventBuilder,
}

impl SseParser {
    fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let mut line = self.buffer[..pos].to_string();
            self.buffer.drain(..=pos);
            if line.ends_with('\r') {
                line.pop();
            }
            if let Some(event) = self.parse_line(&line) {
                events.push(event);
            }
        }
        events
    }

    fn finish(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            if let Some(event) = self.parse_line(line.trim_end_matches('\r')) {
                events.push(event);
            }
        }
        if self.current.has_data() {
            events.push(self.current.take_event());
        }
        events
    }

    fn parse_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.current.has_data().then(|| self.current.take_event());
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.current.event = Some(value.to_string()),
            "id" => self.current.id = Some(value.to_string()),
            "retry" => self.current.retry = Some(value.to_string()),
            "data" => self.current.data_lines.push(value.to_string()),
            _ => {}
        }
        None
    }
}

impl SseEventBuilder {
    fn has_data(&self) -> bool {
        self.event.is_some()
            || self.id.is_some()
            || self.retry.is_some()
            || !self.data_lines.is_empty()
    }

    fn take_event(&mut self) -> SseEvent {
        let event = SseEvent {
            index: 0,
            event: self.event.take(),
            id: self.id.take(),
            retry: self.retry.take(),
            data: self.data_lines.join("\n"),
        };
        self.data_lines.clear();
        event
    }
}

fn renumber(events: Vec<SseEvent>) -> Vec<SseEvent> {
    events
        .into_iter()
        .enumerate()
        .map(|(idx, mut event)| {
            event.index = idx + 1;
            event
        })
        .collect()
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
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("netutils sse");
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

fn error_report(input_url: &str, url: &str, error: String, elapsed: Duration) -> SseReport {
    report(
        input_url,
        url,
        SseProxy {
            mode: "not-built".to_string(),
            value: None,
        },
        None,
        None,
        false,
        Vec::new(),
        Some(error),
        elapsed,
    )
}

fn report(
    input_url: &str,
    url: &str,
    proxy: SseProxy,
    status: Option<u16>,
    content_type: Option<String>,
    connected: bool,
    events: Vec<SseEvent>,
    error: Option<String>,
    elapsed: Duration,
) -> SseReport {
    SseReport {
        input_url: input_url.to_string(),
        url: url.to_string(),
        proxy,
        status,
        content_type,
        connected,
        events,
        error,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        notes: vec![
            "SSE parsing stops when --max-events is reached or --max-seconds elapses.".to_string(),
            "Use -H/--header for auth headers and --proxy for HTTP/SOCKS proxy testing."
                .to_string(),
        ],
    }
}

fn output(report: SseReport, mode: OutputMode) {
    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn print_report(report: &SseReport) {
    println!();
    println!("{}", "📡 SSE Stream".bold());
    println!("  URL: {}", report.url);
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
    println!(
        "  Connected: {}",
        if report.connected {
            "yes".green().to_string()
        } else {
            "no".red().to_string()
        }
    );
    println!(
        "  Status: {}",
        report
            .status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "--".to_string())
    );
    println!(
        "  Content-Type: {}",
        report.content_type.as_deref().unwrap_or("--")
    );
    println!("  Time: {:.2}ms", report.elapsed_ms);
    if let Some(error) = &report.error {
        println!("  Error: {}", error);
    }

    if !report.events.is_empty() {
        println!();
        println!("{}", "Events".bold());
        let rows = report
            .events
            .iter()
            .map(|event| {
                vec![
                    event.index.to_string(),
                    event.event.clone().unwrap_or_else(|| "--".to_string()),
                    event.id.clone().unwrap_or_else(|| "--".to_string()),
                    truncate(&event.data, 120),
                ]
            })
            .collect::<Vec<_>>();
        print_table(&["#", "Event", "ID", "Data"], &rows);
    }

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut result = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        result.push_str("...");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sse_events() {
        let mut parser = SseParser::default();
        let events = parser.push(b"id: 1\nevent: tick\ndata: hello\ndata: world\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id.as_deref(), Some("1"));
        assert_eq!(events[0].event.as_deref(), Some("tick"));
        assert_eq!(events[0].data, "hello\nworld");
    }

    #[test]
    fn ignores_comments() {
        let mut parser = SseParser::default();
        let events = parser.push(b": ping\n\ndata: ok\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "ok");
    }
}
