//! 输出模式：表格（默认）或 JSON。

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

static EXIT_CODE: AtomicU8 = AtomicU8::new(0);

/// 是否输出行分隔 JSON（NDJSON）。
///
/// 刻意**不**做成 `OutputMode` 的第三个变体：全仓有大量 `mode == OutputMode::Json`
/// 形态的判断，加变体会让这些分支把 NDJSON 当表格处理，是一类必然发生又难以穷举
/// 的静默漏判。作为正交开关后，既有 `== Json` 判断自动保持正确（NDJSON 本身就是
/// JSON），只有需要区分「逐行还是末尾一坨」的地方才查这里。
static STREAMING: AtomicBool = AtomicBool::new(false);

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

pub fn set_streaming(on: bool) {
    STREAMING.store(on, Ordering::Relaxed);
}

pub fn streaming() -> bool {
    STREAMING.load(Ordering::Relaxed)
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

/// 输出一行紧凑 JSON（NDJSON）。
///
/// 与 [`print_json`] 的区别不只是格式：这里是「一条记录一行、立刻可被下游读取」，
/// 长跑探测因此能边跑边入库，而不是等命令结束才吐一坨。
pub fn print_json_line<T: Serialize>(data: &T) {
    match serde_json::to_string(data) {
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
