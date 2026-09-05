# Changelog

本文件记录 netutils-cli 的版本变更。

## [0.5.0] - 2026-09-05

### 新增
- `dns` 支持 DoH（DNS over HTTPS，RFC 8484），补上此前代码里自己标注的盲区
  - `--doh <PRESET|URL>`：预设 `cloudflare`/`google`/`quad9`/`adguard`/`alidns`/`dnspod`，或直接传 https URL
  - 基于 `reqwest` 而非 trust-dns 内建 DoH 实现，因此 DoH 查询能复用本项目的代理选择；
    这是排查「代理侧 DNS 行为」的前提，trust-dns 内建 DoH 无法接入这里的代理配置
  - `--proxy` / `--no-proxy` 控制 DoH 请求的代理，未选定代理时显式禁用环境代理
  - 拒绝 `http://` 明文 endpoint：明文 DoH 没有隐私意义，静默接受只会给出虚假的安全感
  - `--doh` 与 `--server` 互斥：两者分别走 HTTPS 和 UDP/53，同时给出会让结果无法归因
  - 报文 ID 固定为 0，遵循 RFC 8484 关于 HTTP 缓存的建议
  - 输出 DNS 响应码、实际 endpoint、代理模式，并提示 DoH 完全绕过系统 resolver
  - JSON 输出带 `transport: "doh"` 字段，便于脚本区分链路
- `dns` 支持 DoT（DNS over TLS，RFC 7858），与 DoH 一起覆盖完整的加密 DNS 传输
  - `--dot <PRESET|HOST[:PORT]>`，预设与 DoH 同名，默认端口 853
  - 沿用 RFC 1035 的 TCP 封装（2 字节大端长度前缀），复用 DoH 侧的报文编解码
  - 输出协商出的 TLS 版本，用于确认查询确实被加密
  - 拒绝纯 IP：DoT 依赖证书校验，只给 IP 无法验证服务器身份，那样的「加密」没有意义
  - 明确不支持代理：DoT 是 853 端口上的裸 TLS 流，穿代理需要 CONNECT 隧道
  - JSON 输出带 `transport: "dot"` 字段
- `dns-leak` 新增加密 DNS 观察：`--doh <PRESET|URL>` / `--dot <PRESET|HOST>`
  - 手法与既有的 `whoami.akamai.net` 探针相同（该域名的 A 记录返回执行查询的 resolver IP），
    区别在于这次走 DoH/DoT，因此能回答「应用自己走加密 DNS 时，解析从哪里出去」
  - 与系统解析路径的 resolver 观察对比，输出 `differs_from_system`；任一侧缺数据时为 `null`，
    部分重合不算「走的是别处」
  - **不参与风险判定**：`risk_level` 仍只评估系统 resolver 路径。加密路径是另一条链路，
    把它塞进现有判定会让结论含义不清；报告里明确说明这一点
  - DoT 的 `proxy_mode` 标为 `direct-not-proxyable`，如实说明它不走代理而非假装应用了代理
  - 未传 `--doh`/`--dot` 时行为与之前完全一致；`--no-external` 同时跳过加密探针

### 修复
- `dns --dot X --proxy Y` 此前能通过参数解析并**静默丢弃** `--proxy`：
  clap 的 `--proxy requires doh` 规则在 `--dot` 同时出现时被冲突规则抵消。
  现在显式拦下并以退出码 2 报用法错误，而不是让用户误以为查询走了代理
- 修复 `completions` 与 `man` 在 Windows 上栈溢出：`clap_complete`/`clap_mangen` 会深度遍历
  整棵命令树，新增参数后超出默认 1MB 主线程栈（0.3.17 曾因同类原因拆分 `plugin` 解析器）。
  现在生成动作在显式给足栈空间的独立线程上执行，不再随命令树增长而踩线

## [0.4.0] - 2026-09-05

### 新增
- 新增 `mtu`（别名 `m`）命令：路径 MTU 发现与 PMTUD 黑洞检测
  - 借助系统 `ping` 的 DF（Don't Fragment）能力二分查找路径 MTU，无需提权和原始套接字
  - 区分「明确收到 ICMP fragmentation-needed」与「大包被静默丢弃」，后者判定为 PMTUD 黑洞
  - 解析路径设备通告的 MTU 并优先验证该值，减少探测轮次
  - 对比出口接口本地 MTU，直接给出隧道开销字节数
  - 支持 `--min-mtu`、`--max-mtu`、`--timeout` 和 `--json`
- `http` 与 `check` 新增 `--assert <EXPR>` CI 断言，可重复传入
  - 运算符：`=`/`==`、`!=`、`<`、`<=`、`>`、`>=`、`*=`（包含子串）
  - 数值支持 `ms`、`s`、`%` 单位，时延指标省略单位时按毫秒解释
  - 指标别名：`latency`→`latency_ms`、`p95`→`p95_ms`、`code`→`status` 等
  - `http` 指标：`status`、`latency_ms`、`body`、`body_bytes`、`final_url`、`error`、`ok`
  - `check` 指标：`success_rate`、`latency_ms`、`min_ms`、`max_ms`、`status`、`total`、`success`、`failed`
  - 断言失败使用独立退出码 `3`，与「探测失败」(`1`) 和「用法错误」(`2`) 区分
  - 区分「指标名写错」与「本次运行取不到该指标」，后者附带具体原因
- 新增全局 `--color auto|always|never`，并遵循 `NO_COLOR`、`CLICOLOR_FORCE`、`CLICOLOR` 环境变量
  - JSON 输出模式强制关闭颜色，避免 ANSI 转义破坏解析
  - 未指定时按 stdout 是否为终端自动判断，重定向到文件不再写入转义序列
  - 解析结果通过 `NETUTILS_COLOR` 传递给插件子进程，核心与插件行为一致
- 新增 `completions <shell>` 生成 bash/zsh/fish/powershell/elvish 补全脚本
- 新增 `man` 生成 roff 格式 man page

### 变更
- 全局开关解析表化，`plugin` 子命令前的 `--color` 等开关不再被误判为子命令

### 测试
- 新增 `tests/cli.rs` 集成测试：覆盖帮助、版本、补全、man、颜色开关、断言语法错误和退出码约定，全部离线运行
- 新增 `Cli::command().debug_assert()` 校验，CLI 定义冲突在测试阶段即暴露

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
