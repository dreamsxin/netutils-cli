# Changelog

本文件记录 netutils-cli 的版本变更。

## [0.3.22] - 2026-07-23

### 新增
- 新增 `dns-leak`（别名 `dl`）命令，检测 DNS 查询是否泄露（是否绕过 VPN/TUN/代理）
  - 本地路由分析：对比每个系统 DNS 服务器的路由接口与流量出口接口是否一致
  - 外部探测（默认开启，`--no-external` 可关闭）：查询随机 `*.ipv4.surfsharkdns.com` 域名获取实际 resolver IP、ISP、地区和供应商 `Leak` 标记
  - 默认并发执行 3 个随机 Surfshark DNS 样本，支持通过 `--count 1..10` 调整探测次数并聚合 resolver 结果
  - 新增 `ip-api-edns` 探针：从固定入口跟随服务端随机域名跳转，获取 resolver IP、国家和组织；每个提供商均执行 `--count` 个样本
  - 使用 `whoami.akamai.net` A 记录辅助观察 resolver，使用 `https://1.1.1.1/cdn-cgi/trace` 独立获取 HTTP 出口公网 IP
  - resolver IP 与 HTTP 出口 IP 分开建模，不再因两者不相等直接判定泄露
  - 外部 HTTP 探针按目标读取显式/系统代理，支持通过 HTTP、SOCKS5H 代理观察代理侧 DNS 行为
  - 合并 Surfshark 与 ip-api-edns 的 resolver 地理结果；远程 DNS 代理跨国家或跨已知城市时判定为 DNS 泄露，样本不完整则标记为证据不足
  - 远程 DNS 模式下 Surfshark 的供应商 `Leak` 标记仅展示，不覆盖 resolver 地理一致性结论
  - 本地环境采集和 DNS 路由查询改为可中断、并发执行，`--total-timeout` 能及时终止命令
  - 支持 `--proxy`/`--no-proxy`/`--timeout` 参数，输出风险等级（none/low/medium/high）和综合结论
- `diag` 一键诊断新增 DNS 泄露快检项（仅本地路由分析，无外部依赖）

## [0.3.18] - 2026-07-08

### 变更
- 将默认 `README.md` 切换为英文，中文文档改为 `README_ZH.md`
- 将 `sse` 和 `ws` 从核心命令拆分为官方插件，核心通过外部插件机制转发

### 新增
- `plugin list` 增加 `sse`、`ws` 已知插件
- `plugin remove/update/install` 支持使用插件名、二进制名或 crate 名，例如 `sse`、`netutils-sse`、`netutils-plugin-sse`
- 保留外部命令别名：`event` 转发到 `sse` 插件，`websocket` 转发到 `ws` 插件

## [0.3.17] - 2026-07-07

### 修复
- 拆分 `plugin` 子命令解析，降低主 CLI 的 clap derive 命令树复杂度，修复 Windows debug 构建下新增插件子命令可能触发栈溢出的问题

### 新增
- 新增 `plugin update-all`，等价于 `plugin update all`

## [0.3.10] - 2026-07-02

### 新增
- 新增 `path` 命令，展示 HTTP 请求的本机视角链路：DNS、多 IP、代理、出口、traceroute、重定向和 DNS/TCP/TLS/TTFB 分阶段耗时
- `path` 支持 `--json`、`--max-hops`、`--timeout`、`--proxy` 和 `--no-proxy`

### 修复
- 修复 `trace myip.ipipv.com` 在大量无响应跳点时长时间无输出的问题，普通输出改为逐跳显示并显式刷新
- 修复 `diagnose` 对多 A 记录域名各步骤解析到不同 IP 导致 TCP/HTTPS 误报的问题
- 修复 macOS `ifconfig` 解析把 `ether`、`media`、`status`、`inet6` 等属性行误当接口的问题
- `scan`、`check --timing`、`diag` 统一使用多 IP 解析，降低 CDN/多后端域名误判
- `dns` 查询增加硬超时，避免不可达 DNS server 长时间等待
- Linux/macOS `connections` 外部命令增加超时，避免系统命令异常导致卡住

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
