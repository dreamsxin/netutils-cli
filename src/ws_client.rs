//! WebSocket handshake and message diagnostics.

use std::time::{Duration, Instant};

use colored::*;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;

use crate::output::{print_json, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
pub struct WsReport {
    pub input_url: String,
    pub url: String,
    pub connected: bool,
    pub status: Option<u16>,
    pub sent: Vec<WsMessage>,
    pub received: Vec<WsMessage>,
    pub error: Option<String>,
    pub elapsed_ms: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsMessage {
    pub index: usize,
    pub kind: String,
    pub bytes: usize,
    pub text: Option<String>,
}

pub async fn run(
    input_url: &str,
    raw_headers: Vec<String>,
    timeout: Duration,
    messages: Vec<String>,
    max_messages: usize,
    max_duration: Duration,
    mode: OutputMode,
) {
    let start = Instant::now();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let url = normalize_url(input_url);
    let mut request = match url.as_str().into_client_request() {
        Ok(request) => request,
        Err(err) => {
            output(
                error_report(
                    input_url,
                    &url,
                    format!("invalid WebSocket URL: {err}"),
                    start.elapsed(),
                ),
                mode,
            );
            return;
        }
    };
    if let Err(err) = apply_headers(request.headers_mut(), &raw_headers) {
        output(error_report(input_url, &url, err, start.elapsed()), mode);
        return;
    }

    let (mut stream, response) = match tokio::time::timeout(timeout, connect_async(request)).await {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => {
            output(
                error_report(input_url, &url, err.to_string(), start.elapsed()),
                mode,
            );
            return;
        }
        Err(_) => {
            output(
                error_report(
                    input_url,
                    &url,
                    "connect timeout".to_string(),
                    start.elapsed(),
                ),
                mode,
            );
            return;
        }
    };

    let mut sent = Vec::new();
    for (idx, message) in messages.into_iter().enumerate() {
        let bytes = message.as_bytes().len();
        match tokio::time::timeout(timeout, stream.send(Message::Text(message.clone().into())))
            .await
        {
            Ok(Ok(())) => sent.push(WsMessage {
                index: idx + 1,
                kind: "text".to_string(),
                bytes,
                text: Some(message),
            }),
            Ok(Err(err)) => {
                output(
                    report(
                        input_url,
                        &url,
                        true,
                        Some(response.status().as_u16()),
                        sent,
                        Vec::new(),
                        Some(format!("send failed: {err}")),
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
                        true,
                        Some(response.status().as_u16()),
                        sent,
                        Vec::new(),
                        Some("send timeout".to_string()),
                        start.elapsed(),
                    ),
                    mode,
                );
                return;
            }
        }
    }

    let deadline = tokio::time::Instant::now() + max_duration;
    let mut received = Vec::new();
    let mut error = None;
    while received.len() < max_messages && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(message))) => {
                let converted = convert_message(received.len() + 1, message);
                let is_close = converted.kind == "close";
                received.push(converted);
                if is_close {
                    break;
                }
            }
            Ok(Some(Err(err))) => {
                error = Some(err.to_string());
                break;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    output(
        report(
            input_url,
            &url,
            true,
            Some(response.status().as_u16()),
            sent,
            received,
            error,
            start.elapsed(),
        ),
        mode,
    );
}

fn normalize_url(input: &str) -> String {
    if let Some(rest) = input.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = input.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if input.starts_with("ws://") || input.starts_with("wss://") {
        input.to_string()
    } else {
        format!("wss://{input}")
    }
}

fn apply_headers(
    headers: &mut tokio_tungstenite::tungstenite::http::HeaderMap,
    raw_headers: &[String],
) -> Result<(), String> {
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
    Ok(())
}

fn convert_message(index: usize, message: Message) -> WsMessage {
    match message {
        Message::Text(text) => {
            let text = text.to_string();
            WsMessage {
                index,
                kind: "text".to_string(),
                bytes: text.as_bytes().len(),
                text: Some(text),
            }
        }
        Message::Binary(bytes) => WsMessage {
            index,
            kind: "binary".to_string(),
            bytes: bytes.len(),
            text: None,
        },
        Message::Ping(bytes) => WsMessage {
            index,
            kind: "ping".to_string(),
            bytes: bytes.len(),
            text: None,
        },
        Message::Pong(bytes) => WsMessage {
            index,
            kind: "pong".to_string(),
            bytes: bytes.len(),
            text: None,
        },
        Message::Close(frame) => WsMessage {
            index,
            kind: "close".to_string(),
            bytes: 0,
            text: frame.map(|frame| format!("{:?} {}", frame.code, frame.reason)),
        },
        Message::Frame(_) => WsMessage {
            index,
            kind: "frame".to_string(),
            bytes: 0,
            text: None,
        },
    }
}

fn error_report(input_url: &str, url: &str, error: String, elapsed: Duration) -> WsReport {
    report(
        input_url,
        url,
        false,
        None,
        Vec::new(),
        Vec::new(),
        Some(error),
        elapsed,
    )
}

fn report(
    input_url: &str,
    url: &str,
    connected: bool,
    status: Option<u16>,
    sent: Vec<WsMessage>,
    received: Vec<WsMessage>,
    error: Option<String>,
    elapsed: Duration,
) -> WsReport {
    WsReport {
        input_url: input_url.to_string(),
        url: url.to_string(),
        connected,
        status,
        sent,
        received,
        error,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        notes: vec![
            "Use --message repeatedly to send text messages after the WebSocket handshake.".to_string(),
            "This command currently tests direct WebSocket connections; proxy tunneling can be added separately.".to_string(),
        ],
    }
}

fn output(report: WsReport, mode: OutputMode) {
    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        print_report(&report);
    }
}

fn print_report(report: &WsReport) {
    println!();
    println!("{}", "🔌 WebSocket Probe".bold());
    println!("  URL: {}", report.url);
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
    println!("  Time: {:.2}ms", report.elapsed_ms);
    if let Some(error) = &report.error {
        println!("  Error: {}", error);
    }

    if !report.sent.is_empty() {
        println!();
        println!("{}", "Sent".bold());
        print_messages(&report.sent);
    }
    if !report.received.is_empty() {
        println!();
        println!("{}", "Received".bold());
        print_messages(&report.received);
    }

    println!();
    for note in &report.notes {
        println!("  {}", note.dimmed());
    }
}

fn print_messages(messages: &[WsMessage]) {
    let rows = messages
        .iter()
        .map(|message| {
            vec![
                message.index.to_string(),
                message.kind.clone(),
                message.bytes.to_string(),
                message
                    .text
                    .as_ref()
                    .map(|text| truncate(text, 120))
                    .unwrap_or_else(|| "--".to_string()),
            ]
        })
        .collect::<Vec<_>>();
    print_table(&["#", "Type", "Bytes", "Text"], &rows);
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
    fn normalizes_websocket_urls() {
        assert_eq!(normalize_url("example.com/ws"), "wss://example.com/ws");
        assert_eq!(
            normalize_url("https://example.com/ws"),
            "wss://example.com/ws"
        );
        assert_eq!(
            normalize_url("http://example.com/ws"),
            "ws://example.com/ws"
        );
        assert_eq!(normalize_url("ws://example.com/ws"), "ws://example.com/ws");
    }

    #[test]
    fn rejects_invalid_header() {
        let mut headers = tokio_tungstenite::tungstenite::http::HeaderMap::new();
        assert!(apply_headers(&mut headers, &["bad".to_string()]).is_err());
    }
}
