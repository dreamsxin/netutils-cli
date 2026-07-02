# Changelog

本文件记录 netutils-cli 的版本变更。

## [0.3.9] - 2026-07-02

### 优化
- 默认出口优先通过系统路由查询实际目标出口接口，TUN/utun 场景识别更准确
- `egress` 输出增加 TUN 模式状态
- 接口类型识别补充 `utun*`/`tun*`/`tap*`

## [0.3.8] - 2026-07-02

### 优化
- macOS 代理检测显示更多系统代理相关设置：FTP、PAC、自动发现、简单主机排除和例外列表
- macOS 接口跃点改用 `networksetup -listnetworkserviceorder` 的网络服务顺序，避免大量接口硬显示为 `0`
- macOS 路由表 metric 使用网络服务顺序，并优先展示默认路由

## [0.3.7] - 2026-07-02

### 新增
- macOS 读取“系统设置/系统偏好设置”的网络代理配置（HTTP/HTTPS/SOCKS/PAC/自动发现），用于 `proxy`、`diag`、`diagnose` 和 HTTP 连通性检测
- Linux 读取 GNOME 系统代理配置（manual/auto），并继续保留环境变量兜底
- 统一系统代理解析入口，Windows/macOS/Linux 自动代理检测使用同一套逻辑

## [0.3.6] - 2026-07-02

### 修复
- 修复 macOS 接口解析分支中 `current_name` 被 move 后再次使用导致安装编译失败的问题

## [0.3.5] - 2026-07-02

### 修复
- Windows `iface` 改用 CIM 采集接口信息，避免 `Get-NetAdapter` 长时间阻塞导致命令无响应
- Windows PowerShell 采集命令增加非交互模式和进程级超时，避免外部命令卡住主程序
- 修复 `scan --concurrency 0` 永久等待的问题
- 为 DNS、ICMP ping、traceroute、HTTPS timing 等网络探测补充超时边界
- `diag` 按任务完成顺序输出结果，避免慢任务阻塞已完成检测项

## [0.3.1] - 未发布

### 新增
- 命令参数增强：ping `--timeout`/`--interval`，dns `--server`，trace `--max-hops`，scan `--concurrency`，check `--timeout`
- connections `--proto tcp|udp` 过滤
- connections 代理标注列（自动识别代理相关连接）
- 共享 ICMP 模块 `src/icmp.rs`（traceroute 和 diagnose 共用）
- CHANGELOG.md

### 优化
- diagnose: traceroute 步骤并行化（5 步全部 `tokio::join!`）
- diagnose: 结论逻辑包含 trace 步骤
- diagnose: IP 提取改为结构化返回（不再字符串解析）
- 代码去重：ICMP 函数提取到共享模块
- 清理死代码：删除 `i18n::t3`、`util::fmt_ms`

## [0.3.0] - 2026-06-26

### 新增
- `diagnose <host>` 全链路诊断（DNS→Ping→TCP→HTTPS→Traceroute），自动定位断点
- `connections` 网络连接列表（TCP/UDP + 进程信息，跨平台）
- HTTP 检测自动走系统代理，标注 `[经代理]`/`[直连]`
- reqwest 加 `rustls-tls-native-roots` 修复代理 TLS 证书

## [0.2.2] - 2026-06-26

### 优化
- diag 增加国内+国际 DNS 和 HTTP 连通性检测（baidu.com + google.com）
- diag 全部 8 项检测并行执行（`tokio::join!`）
- diag 标签显示翻译后的名称
- HTTP 超时统一 5s

## [0.2.1] - 2026-06-25

### 修复
- 表格 ANSI 颜色码导致对齐错位（`display_width` 剥离 ANSI 转义码）
- banner 左右边框颜色统一

## [0.2.0] - 2026-06-25

### 新增
- 跨平台支持（Linux/macOS）：interface/route 模块 `#[cfg]` 条件编译
- egress 多候选探测（8.8.8.8/1.1.1.1/114.114.114.114/223.5.5.5）
- i18n Windows 检测改用 `GetACP` API（消除 PowerShell 启动延迟）
- `util.rs` 公共工具模块（resolve_host/compute_stats/parse_ports）
- `anyhow` 统一 error handling，main 返回 Result
- 统一 `print_json_error` JSON 错误处理
- 端口范围语法：`parse_ports` 支持 `80-100,443` 混合格式
- 13 个单元测试

### 修复
- connectivity 表头硬编码中文 → i18n
- egress.logic_selected `{2}` 占位符未替换 → `t4()`
- IPv6 `host:port` 解析 → `parse_host_port` 支持 `[::1]:443`
- JSON 错误未转义 → 统一 `print_json_error`
- traceroute ident/seq 未校验 → 恢复校验
- diag gateway fallback 字符串手术 → `t1()`
- i18n: `AtomicU8` 替代 `static mut`（消除 unsafe）

## [0.1.0] - 2026-06-25

### 初始发布
- 网络接口列表（物理/虚拟/VPN/TUN 类型识别）
- 流量出口检测 + 选路逻辑
- 路由表
- 代理设置（环境变量 + Windows 注册表）
- Ping（ICMP/TCP 回退）
- DNS 查询（A/AAAA/MX/CNAME/NS/TXT）
- Traceroute（TTL 递增）
- 端口扫描（并发 TCP connect）
- 连通性测试（TCP/HTTP）
- 一键诊断（diag）
- 国际化（中英文自动切换）
- `--json` 输出
- 颜色高亮
- 命令别名
