# 端口进程查询与管理 — 任务

**Spec**: `process-management`
**对应需求**: `requirements.md` R1–R5
**对应设计**: `design.md` §1–§7
**状态：已完成**

约定：按顺序实施。每个任务交付一条可运行的用户功能，README 与行为同一切片更新。任务通过验收后勾选；每个任务均运行三道验证门：

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## 任务

- [x] **T1 按本机端口查询进程**
  - 文件：新增 `src/process.rs`；改 `src/cli.rs`、`src/main.rs`、`src/connections/mod.rs`、`src/connections/conn_win.rs`、`src/connections/conn_unix.rs`、`src/i18n.rs`、`README.md`、`tests/cli_smoke.rs`
  - 内容：新增 `process --port <PORT>` 命令及表格/JSON 查询；扩展连接采集 API，使查询能区分完整采集、部分进程信息不可见和采集器不可用，同时保持 `connections` 原参数、输出字段及本地/远端端口过滤行为不变；只按本地地址精确匹配端口，按 PID 聚合全部匹配端点，未知 PID 的端点单独保留；在 README 命令索引和示例中记录查询方式
  - 验收：单测覆盖 `80` 不匹配 `8080`、IPv4/IPv6 地址、TCP/UDP、同一端口多个 PID、未知 PID、无匹配与采集失败状态；CLI smoke 覆盖新命令启动及帮助；手工对本机已知监听端口和空闲端口分别验证表格与 JSON；确认既有 `connections` JSON 字段和端口过滤回归测试通过；三道验证门通过
  - 需求：R1、R3、R4、R5

- [x] **T2 确认后结束指定进程**
  - 文件：改 `src/process.rs`、`src/cli.rs`、`src/main.rs`、`src/i18n.rs`、`README.md`、`tests/cli_smoke.rs`
  - 内容：新增 `process kill <PID>`；拒绝 PID 0 和当前进程 PID；仅在交互表格模式下提示并读取确认，取消时不执行；JSON 或非交互调用缺少 `--yes` 时立即返回 `confirmation_required`，不读取 stdin、不等待；Windows 用 PowerShell `Stop-Process`，Linux/macOS 发送 `SIGTERM`，统一输出稳定 JSON 状态并映射退出码；错误原因和确认提示补齐中英文；README 加入先查询再明确确认结束单个 PID 的示例
  - 验收：通过可注入的执行器测试确认/取消/成功/不存在/权限不足/工具失败分支，不结束测试宿主进程；验证 PID 0、自身 PID 和缺少 `--yes` 均不会调用结束函数；JSON `status` 与 `error_code` 可区分取消、需确认和失败；手工使用专用测试进程验证交互拒绝与 `--yes` 成功，不以生产进程作测试目标；三道验证门通过
  - 需求：R2、R3、R4、R5
