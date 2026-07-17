//! 输出模式：表格（默认）或 JSON。

use serde::Serialize;
use std::sync::atomic::{AtomicU8, Ordering};

static EXIT_CODE: AtomicU8 = AtomicU8::new(0);

/// 输出模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// 表格 + 颜色（默认）
    Table,
    /// JSON 序列化
    Json,
}

/// JSON 错误响应
#[derive(Serialize)]
struct JsonError {
    error: String,
}

/// 渲染 JSON 输出
pub fn print_json<T: Serialize>(data: &T) {
    match serde_json::to_string_pretty(data) {
        Ok(s) => println!("{}", s),
        Err(e) => {
            mark_failure();
            eprintln!("JSON serialization error: {}", e);
        }
    }
}

/// 渲染 JSON 错误输出（统一错误格式，正确转义）
pub fn print_json_error(msg: &str) {
    mark_failure();
    let err = JsonError {
        error: msg.to_string(),
    };
    match serde_json::to_string_pretty(&err) {
        Ok(s) => println!("{}", s),
        Err(_) => println!("{{\"error\": \"unknown\"}}"),
    }
}

pub fn mark_failure() {
    EXIT_CODE.fetch_max(1, Ordering::Relaxed);
}

pub fn mark_exit_code(code: i32) {
    let code = if code <= 0 {
        1
    } else {
        u8::try_from(code).unwrap_or(u8::MAX)
    };
    EXIT_CODE.fetch_max(code, Ordering::Relaxed);
}

pub fn mark_timeout() {
    EXIT_CODE.store(124, Ordering::Relaxed);
}

pub fn exit_if_failed() {
    let code = EXIT_CODE.load(Ordering::Relaxed);
    if code != 0 {
        std::process::exit(i32::from(code));
    }
}

pub fn print_timeout_error(mode: OutputMode, seconds: u64) {
    mark_timeout();
    let message = format!("command exceeded total timeout of {seconds}s");
    if mode == OutputMode::Json {
        print_json_error(&message);
    } else {
        eprintln!("{message}");
    }
}
