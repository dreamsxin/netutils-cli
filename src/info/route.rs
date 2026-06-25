//! 路由表信息模块（公共结构）。

use serde::Serialize;

/// 路由条目
#[derive(Debug, Clone, Serialize)]
pub struct RouteEntry {
    pub destination: String,
    pub gateway: String,
    pub interface: String,
    pub metric: String,
}
