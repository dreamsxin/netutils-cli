# Changelog

本文件记录 netutils-cli 的版本变更。

## [0.7.0] - 2026-09-24

主题：工具能把单个目标诊断得很细，但无法用于巡检，输出也无法与其他遥测关联。
设计过程见 `docs/specs/batch-and-observability/`。

### 新增
- `check` 与 `scan` 支持 `--targets-from <FILE|->` 读取目标清单，每行一个，`-` 表示标准输入。
  此前每个目标参数都是单个位置字符串，全仓没有任何 `io::stdin()` 调用，
  巡检 200 个端点必须靠外层 shell 循环，而每份输出除单个 `target` 字段外
  没有可关联标识，结果拼不回一张表
  - 清单格式刻意宽容（这类文件通常人手维护）：整行 `#` 注释、空行、CRLF、
    记事本留下的 UTF-8 BOM 全部容错；目标字符串内的 `#` **不**当注释
  - 保序且不去重——同一目标多采几次是合理用法
  - 无法探测的目标（格式错误、主机解析不了）记成一条带原因的失败结果并继续，
    且仍占一个结果位，因此结果数恒等于清单行数，消费方可以按行对齐
  - 断言逐目标独立评估：聚合的 `success_rate` 既可理解成「目标内成功率」
    也可理解成「成功目标占比」，用户无从判断是哪个
- `scan -p/--ports <LIST>`，与位置参数 `PORTS` 等价且互斥。批量模式下必须用它：
  `HOST` 变成可选后 clap 按顺序填充位置参数，`scan --targets-from list 80,443`
  会把端口串填进 `HOST`，于是批量模式下没有任何途径传端口
- `scan --parallel <N>` 并发扫多台主机。大于 1 时**不缓冲**输出，改为降低粒度：
  每台主机完成即打印一行，带主机名与完成计数器。排障是本工具的唯一用途，
  输出憋到结束就无法判断卡在哪一步；JSON 的 `results` 仍按清单顺序
- 探测记录带时间戳。`ping`、`scan`、`check` 的每条探测新增 `ts`（探测**发起**时刻），
  每次运行新增 `started_at` / `finished_at`，均为 RFC 3339 UTC 毫秒精度。
  此前探测记录只有 `seq`/`rtt_ms`，输出无法与交换机日志、应用日志对齐时间线，
  也无法作为带日期的工单证据
  - `scan` 的时间戳取自第一次 connect 之前而非返回时：一个端口会串行重试最多
    8 个候选 IP，取返回时刻会误报该端口是什么时候开始探的
- `check --parallel <N>` 同时探测多个目标，语义与 `--concurrency`（目标内部并发）正交。
  大于 1 时抑制单目标内部的逐次行、改为每目标完成即打印一行——并发下逐次行必然交错
- `--ndjson` 全局开关：一条记录一行、紧凑格式、完成即输出，可直接管进日志采集。
  此前 JSON 是 `to_string_pretty` 的单个大对象且在全部探测结束后才吐出，长跑无法边跑边入库
  - 记录带 `record` 字段区分 `probe`（逐次探测，带 `target` 与 `seq`）、
    `summary`（目标汇总）、`batch_summary`（整批收尾，自带 `ts` 与 `interrupted`）
  - 隐含 `--json`，两个都传不报错
  - 刻意**不**做成 `OutputMode` 的第三个变体：全仓大量 `mode == OutputMode::Json` 判断
    会因此把 NDJSON 当表格处理，属于必然发生又难以穷举的静默漏判
- 批量 JSON 为独立的顶层结构（`mode` / `started_at` / `finished_at` / `stats` / `results`），
  仅在传清单时出现，另带 `interrupted`。
  做成独立类型而非改造单目标结构，是为了让单目标契约在结构上不可能被批量改动波及
- `check --show-timestamp` 在表格模式的逐次行前显示 RFC 3339 时刻。默认关闭：
  时间戳每行占 24 个字符，长跑时会把真正要看的延迟和错误挤到一边；JSON 不受影响，
  `ts` 字段一直都在

### 修复
- `main` 改为在显式 16 MB 栈的线程上运行整个 tokio 运行时。clap derive 构建命令树是
  递归的，Windows 主线程栈只有 1 MB，新增两个 `--parallel` 参数后**任何**调用（含
  `--version`）启动即栈溢出。这是同一问题第三次出现（0.3.17 拆 `plugin` 解析器、
  0.5.0 把 `completions`/`man` 移到大栈线程），这次不再逐点规避

### 变更
- `check` 的两处代理错误（代理不可达、代理地址无效）在 JSON 里也带上 `❌` 前缀，
  与表格路径一致（格式错误此前两边都有）。原因是报错从探测函数里移了出来——
  批量路径必须把错误转成结果而不是中途打印，同一个字符串无法在两种模式下
  渲染成两种样子

### 兼容性
- 单目标输出未变。`ping`、`scan`、`check` 的 `--json` 顶层结构只新增上述时间戳字段，
  没有任何字段被改名、改类型或改嵌套层级。做法是拆分前先采基线、改完归一化后
  逐行 `diff` 验证，而不是靠人工通读

### 工程
- 新增 `tests/cli_smoke.rs`：启动二进制、跑通 `--version` / `--help` / 每个子命令的
  `--help`、以及两条用法错误的退出码。此前 `cargo test` 从不启动二进制，因此对
  「命令行解析阶段就崩」这一整类问题完全失明——0.7.0 那次栈溢出连 `--version`
  都起不来，而 233 个单测全绿、三道门全过。把栈临时改小到 512 KB 实测过，
  5 条里有 4 条会红，确认这层网不是空的
- 新增 `ROADMAP.md`（里程碑与明确不做的事）、`AGENTS.md`（SDD 流程与门禁、
  两仓关系、不依赖日期库等不体现在源码里的决定，以及三类门禁盖不住的失败）
- 发布流程重构为 `resolve → publish → create-release → bundle → finalize-release`：
  单点解析版本与标签提交、把插件仓 ref 钉成具体 commit、Release 先建 draft
  待三平台产物齐全再转正、顶层 `concurrency` 串行化发布

## [0.6.0] - 2026-09-06

### 新增
- 插件安装收紧供应链假设，对齐 `gh extension` / `krew` 等现代插件体系的做法
  - 默认传 `cargo install --locked`，使用 crate 发布时携带的 `Cargo.lock`。
    此前每次安装都会重新解析传递依赖，同一条命令在不同时间可能装出不同的依赖树
  - `install --version <REQ>` 可把安装钉在具体版本；此前只能无条件取最新版，
    上游一旦发布被投毒的新版本会被直接安装
  - `install --no-locked` 作为逃生阀：crate 未随包发布 `Cargo.lock` 时 `--locked` 会失败，
    错误信息里会给出该提示
  - `plugin-lock.json` 记录二进制的 SHA-256，`plugin list` 新增 `Integrity` 列，
    可发现插件二进制被替换或改动（`ok` / `changed` / `unrecorded` / `unreadable` / `--`）
  - 字段用 `#[serde(default)]` 兼容旧 lock：读到 `None` 只表示"未记录过"，
    不会被误报成校验失败

### 说明
- 需要澄清此前一处不准确的表述：cargo 本身会用索引中的 `cksum` 校验下载的 `.crate`，
  所以并非"完全没有完整性校验"。真正的缺口是**版本不受约束**、**依赖每次重解析**、
  以及**安装后无法发现二进制被改动**——本次针对的是这三点

### 修复
- `ping --interval` 此前不生效。探测是先全部跑完再逐条打印的，`--interval`
  只延迟了**打印**，实际 ICMP 包仍然背靠背发出；`--json` 模式下更是完全没有间隔。
  现在改为「探测 → 输出 → 等待」逐轮进行，间隔作用于实际发包。
  实测 `--json ping -c 3 --interval 2` 从约 0s 变为 4.1s

### 新增
- `ping --count 0` 持续探测直到 Ctrl-C，补上此前完全缺失的连续观测能力
  （对应 `ping -t` / gping 的基础用法；间歇性故障正是一次性探测最容易漏掉的）
  - Ctrl-C 会正常收尾并打印统计，而不是让进程被直接杀掉、丢弃已采集的数据
  - 表格模式下逐行实时输出
  - JSON 模式下按 NDJSON 逐行输出每次探测，末尾补一份统计对象；
    **有界 `--count` 的单个 JSON 对象契约保持不变**，不影响既有脚本
- `check --interval <SECS>` 控制串行探测间隔（默认 1，0 表示不等待）。
  此前间隔硬编码为 1 秒且无法调整，`check -c 60` 必然耗时 60 秒以上；
  现代压测/探测工具（vegeta、oha、httpx）都提供速率控制

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

### 工程
- 新增 `rust-toolchain.toml` 固定工具链到 1.98.1，CI 阻塞门与本地开发使用同一版本。
  此前 CI 跟随 `stable`，新版 clippy 加一条 lint 就能让**没有任何代码改动**的分支变红
  （`clippy::result_large_err` 在 1.98 就这样打断过一次插件仓库的 CI）
- CI 增加不阻塞的 `latest-stable` 任务，跟随最新 stable 提前暴露新 lint，
  但不再让工具链升级阻塞合并
- 固定工具链与 `.github/` 从发布包中排除，不强迫 crate 用户下载特定版本的 rustc

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
