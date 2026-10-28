# 端口进程查询与管理 — 设计

**Spec**: `process-management`
**对应需求**: `requirements.md` R1–R5
**状态：已批准**

## 1. 命令入口

新增独立的 `process` 命令，示例：

```text
netutils process --port 8080
netutils process kill 1234
netutils process kill 1234 --yes
netutils --json process --port 8080
netutils --json process kill 1234 --yes
```

`process --port` 只查询本机 TCP/UDP 本地端口；不匹配远端对端端口。`process kill` 只接受一个 PID，不提供按端口批量结束进程。查询参数与 `kill` 子命令互斥；二者均缺失时按 CLI 用法错误处理。

表格模式且 stdin 是终端时，`kill` 在发送终止请求前显示 PID 与进程名并要求确认。用户拒绝则不执行。JSON 模式或非交互 stdin 不等待输入，必须提供 `--yes`；缺少时返回结构化的 `confirmation_required` 结果和退出码 2。这样脚本使用者必须显式表达结束进程的意图。

## 2. 模块边界

- 新增 `src/process.rs`，负责端口结果筛选与聚合、表格/JSON 渲染、确认流程、PID 校验及结束进程操作。
- `src/cli.rs` 增加 `Commands::Process` 和 `ProcessAction::Kill`；`src/main.rs` 负责将 CLI 参数分发给 `process::run`。
- `src/connections/mod.rs` 暴露按本地端口过滤的连接采集入口和采集状态类型，继续复用 `ConnectionInfo` 与现有平台解析器。平台命令尽量在采集阶段按端口过滤，避免 Windows 对整机所有端点逐项查询进程导致超过现有 5 秒命令超时；解析后仍以本地端口精确复核。现有 `connections::run` 保持其表格输出、JSON 结构与端口过滤语义不变（当前过滤同时匹配本地与远端端口，见 `src/connections/mod.rs:516-534`）。
- `src/connections/conn_win.rs`、`src/connections/conn_unix.rs` 返回采集状态，而不再只以空 `Vec` 表示所有情况。Windows 继续使用 PowerShell `Get-NetTCPConnection` / `Get-NetUDPEndpoint`（`src/connections/conn_win.rs:9-70`）；Linux 继续使用 `ss` 并回退到 `netstat`，macOS 继续使用 `lsof`（`src/connections/conn_unix.rs:16-70`）。
- `src/i18n.rs` 为新表头、确认提示、无结果、部分信息、采集错误和结束结果增加中英文文案；`README.md` 更新命令索引及示例。现有连接采集平台命令和 i18n 入口可见于 `src/connections/mod.rs:40-49`、`src/i18n.rs:409-435`。

不增加依赖。项目已有跨平台外部命令超时封装：`command_output_timeout` 返回进程状态和 stderr，并在超时或无法启动时返回 `None`（`src/util.rs:322-377`）；Windows 也已有 PowerShell 包装（`src/util.rs:306-320`）。新采集状态从这些结果中建立，不新增系统信息依赖。

## 3. 采集结果与查询规则

端口查询的连接采集返回 `collect_connections(port) -> Result<ConnectionSnapshot, ConnectionReadError>`：

- `ConnectionSnapshot` 保存已解析连接以及采集是否完整。
- 所有可用采集器均无法启动、超时或报告读取失败时返回错误，不能伪装成空结果。
- 命令成功执行但没有匹配本地端口时返回完整空快照；这是正常查询结果。
- 若端点存在但 PID 或进程名不可见，保留可见端点并标记快照不完整，不能将其误报为“端口无人占用”。
- Linux 的 `ss` 不可用或失败时尝试 `netstat`；macOS `lsof` 的“没有匹配项”与权限/执行失败根据退出状态及 stderr 区分；Windows PowerShell 脚本把采集错误显式传回调用方。
- Linux 优先以 `ss` 的本地源端口过滤表达式查询，回退 `netstat` 后在 Rust 中精确过滤；macOS 的 `lsof` 结果在 Rust 中按本地端口精确过滤；Windows 的 TCP/UDP cmdlet 直接接收 `-LocalPort`，并使用 15 秒的独立查询上限以容纳系统 cmdlet 初始化时间。

查询端口时解析 `local_addr` 的末尾数值端口并与 `u16` 精确比较，不用字符串后缀匹配，以免端口 `80` 命中 `8080`。IPv4 与 IPv6 地址沿用采集器给出的原始文本。结果按 PID 聚合；每个进程下保留匹配端点列表，包含协议、本地地址及 TCP 状态。无法映射 PID 的端点单独放入 `unidentified_endpoints`。

新增序列化类型：

```text
ProcessLookupOutput {
  port: u16,
  status: "ok" | "partial" | "unavailable",
  processes: [{ pid: u32, process_name: string, endpoints: [{ protocol, local_addr, state }] }],
  unidentified_endpoints: [{ protocol, local_addr, state }],
  total: usize,
  error_code: optional string
}
```

`status: "ok"` 且 `processes` 为空表示查询成功但没有占用者；`partial` 表示至少有一个端点无法关联进程；`unavailable` 表示采集器无法取得可信结果。`error_code` 使用稳定机器值（例如 `process_metadata_incomplete`, `permission_denied`, `collector_unavailable`），表格中的解释文本走 i18n。`state` 对 UDP 为 `null`。新结构只属于 `process`，不改已有 `ConnectionsOutput` 的字段（定义见 `src/connections/mod.rs:23-30`）。

## 4. 结束进程

`ProcessAction::Kill` 先验证 PID 大于 0 且不等于当前 `netutils` 进程 PID，再执行确认规则；拒绝确认、PID 不合法或缺少非交互确认时均不调用平台结束函数。

- Windows 使用 PowerShell `Stop-Process -Id <pid> -ErrorAction Stop`，并检查脚本退出状态；PID 是解析后的整数，不接受 shell 文本。
- Linux/macOS 使用 `kill -TERM <pid>`，通过现有超时命令封装执行。成功表示操作系统接受了终止信号，不承诺目标进程已完成清理退出；不自动升级为强制终止。
- 不存在的 PID、权限不足、工具不可用或命令失败均产生失败结果，不输出成功。命令启动/超时错误与 OS 命令错误映射为稳定 `error_code`，用户可见原因通过中英文文案呈现。

结束结果 JSON：

```text
ProcessKillOutput {
  pid: u32,
  success: bool,
  status: "terminated" | "cancelled" | "confirmation_required" | "failed",
  error_code: optional string
}
```

未提供 `--yes` 的 JSON/非交互请求返回 `confirmation_required` 并标记退出码 2；用户取消或 OS 执行失败标记退出码 1；成功和无匹配查询退出码为 0。退出码与 `src/output.rs:90-110` 的现有聚合约定一致。

## 5. 错误与渲染

表格和 JSON 使用相同结果类型。表格中的说明、错误和确认提示全部从 `t()` 取得；JSON 的 `status`、`error_code` 是语言无关的稳定值。采集错误和结束失败打印结构化结果后设置失败退出码。CLI 参数缺失/组合冲突使用既有用法错误退出码 2（`src/main.rs:466-475`）。

新命令独立于 `connections`，避免在既有连接列表加入有副作用的操作，也避免改变其 JSON 契约。README 的命令目录目前单独列出 `connections`（`README.md:18-43`），新能力在同一目录中列出并提供查询与显式确认结束的示例。

## 6. 被否决的方案

- **给 `connections` 增加 `--kill`**：把只读连接检查和破坏性操作放在同一命令中，容易意外触发，也会扩大既有命令的参数与输出契约改动面。
- **新增 `sysinfo` 等依赖统一枚举进程**：进程列表本身不能可靠映射到 TCP/UDP 本地端口；本仓已有三平台端点采集与 PID 解析，复用它们能少一份数据源和依赖。
- **用端口直接结束所有匹配进程**：端口可能被多个 PID 共享，自动终止全部与 R2 的逐个明确选择相冲突。

## 7. 验证策略

- 纯函数测试覆盖端口精确匹配（含 `80` / `8080`）、IPv4/IPv6 本地地址解析、按 PID 聚合、未知 PID 端点和完整/部分/不可用状态。
- CLI 测试覆盖查询与 kill 参数互斥、缺少操作目标、PID 0/自身 PID 拒绝、非交互缺少 `--yes` 不执行结束函数，以及 JSON 不提示 stdin。
- 平台命令结果测试覆盖无匹配、命令缺失/超时、权限错误和成功/失败状态；终止函数通过注入的执行器测试，不结束真实进程。
- 端到端手工验证在各平台分别查询一个已知监听端口与空闲端口；JSON 结果用 `jq -e` 验证；在明确的测试进程上核对交互拒绝、`--yes` 成功及权限失败路径。
- 任务切片需同时更新 README，并按仓库三道验证门执行 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。
