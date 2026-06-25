//! 输出模式：表格（默认）或 JSON。

use serde::Serialize;

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
        Err(e) => eprintln!("JSON serialization error: {}", e),
    }
}

/// 渲染 JSON 错误输出（统一错误格式，正确转义）
pub fn print_json_error(msg: &str) {
    let err = JsonError {
        error: msg.to_string(),
    };
    match serde_json::to_string_pretty(&err) {
        Ok(s) => println!("{}", s),
        Err(_) => println!("{{\"error\": \"unknown\"}}"),
    }
}
