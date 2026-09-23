# 批量与可观测性 — 任务

**Spec**: `batch-and-observability`
**对应**: `requirements.md` R1–R12，`design.md` §0–§7
**状态**: 实施中

约定：每个任务完成并通过其验收检查后立刻勾选，与代码同一次提交。三道验证门（`cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`）在每个任务结束时都要过。

**顺序修订（实施中发现）**：原计划把「时间戳字段落地」放在阶段三。实际做 T1 时 `clippy -D warnings` 因 `dead_code` 直接失败——新模块的 `now_rfc3339_millis` 在本任务内没有任何调用方。既不能加 `#[allow(dead_code)]` 临时糊（属于本仓明令避免的过渡性残留），也不该为此放宽门禁，因此把字段落地提前为 T2，让新 API 在引入的同时就有真实消费者。字段落地不依赖阶段二的探测/渲染拆分，提前不产生新耦合。

---

## 阶段一：基础设施

- [x] **T1 提取时间戳模块**
  - 文件：新增 `src/timestamp.rs`；改 `src/diag/mod.rs`、`src/main.rs`（`mod` 声明）
  - 内容：把 `current_timestamp` / `days_to_date` / `is_leap` 从 `src/diag/mod.rs` 迁入；新增 `now_rfc3339_millis()`（`2026-09-23T07:54:29.412Z`）与 `now_display()`（保留 diag 现有 `YYYY-MM-DD HH:MM:SS` 格式）；`diag` 改为调用，**不保留第二份日期换算**
  - 验收：`diag` 的时间戳字符串格式与改动前逐字符一致；单测覆盖 1970-01-01、2024-02-29（闰年）、2023-12-31T23:59:59（跨年）、毫秒补零（`.007`）
  - 需求：R4 R5 R10

- [x] **T2 探测级与运行级时间戳字段（`ping` / `scan`）**
  - 文件：`src/ping/mod.rs`、`src/portscan/mod.rs`
  - 内容：`ProbeResult` 加 `ts`、`PortResult` 加 `ts`（取值时机为探测**发起**时）；`PingOutput` / `ScanOutput` 加 `started_at`、`finished_at`
  - 验收：单目标 `--json` 除新增字段外结构与 0.6.0 一致；三道门通过（`now_rfc3339_millis` 此时已有真实调用方）
  - 需求：R4 R5 R7

  > **范围调整（实施中发现）**：原计划把 `connectivity` 的 `CheckProbe.ts` 一并放在本任务。实际 `CheckProbe` 在 `src/connectivity/mod.rs` 有近 20 处结构体字面量构造点（`:239` `:258` `:277` `:490` `:506` 以及 `--timing` 分解路径的 `:617`–`:859` 十余处早返回），而 T7 正要重写这些函数的组织方式。在 T7 之前加字段等于把同一批字面量改两遍，且中间态更容易漏。因此 `CheckProbe.ts` 与 `CheckOutput.started_at/finished_at` 并入 T7 一次完成。`ping` 的时间戳则只需在 `Prober` 的两个探测函数各取一次，无此问题。


- [ ] **T3 目标清单解析**
  - 文件：新增 `src/targets.rs`；`src/main.rs`（`mod` 声明）
  - 内容：`load_targets(spec: &str) -> Result<Vec<String>, String>`；`-` 读 stdin，否则按路径读；去 BOM、去尾部 `\r`、trim、跳过空行与 `#` 起始行；保序不去重；空结果/读失败返回 `Err`
  - 验收：单测覆盖注释行、空行、CRLF、UTF-8 BOM、全注释文件报错、不存在路径报错、重复目标保留、顺序不变
  - 需求：R1 R2

- [ ] **T4 流式输出开关与 `print_json_line` 提升**
  - 文件：`src/output.rs`、`src/ping/mod.rs`
  - 内容：`src/output.rs` 增 `STREAMING: AtomicBool` 与 `set_streaming` / `streaming`；新增泛型 `pub fn print_json_line<T: Serialize>(data: &T)`（紧凑，非 pretty）；删除 `src/ping/mod.rs` 的私有同名函数改用新函数
  - 验收：`ping --count 0 --json` 的逐行输出与改动前一致（R7）；不新增 `OutputMode` 变体
  - 需求：R6 R7

- [ ] **T5 批量编排模块**
  - 文件：新增 `src/batch.rs`；`src/main.rs`（`mod` 声明）
  - 内容：`run_targets(targets, parallel, run_one)`，`Semaphore` + `JoinSet`，按索引回填保序；独立任务监听 Ctrl-C 只置 `AtomicBool` 停止标志，派发前检查，**不用 `select!` 取消在飞任务**；返回值携带 `interrupted` 标记
  - 验收：单测构造乱序完成时间，断言结果顺序等于输入顺序；单测验证同时在飞数不超过 `parallel`
  - 需求：R3 R11

  > T4 / T5 的新 API 同样会触发 `dead_code`。两者的真实消费者在阶段四，因此这两个任务与 T6/T7 之后的接线合并成同一次提交，不单独提交。

## 阶段二：探测与渲染拆分（输出必须逐字节不变）

- [ ] **T6 采集输出基线**
  - 文件：无（临时文件，验证后删除）
  - 内容：用当前二进制对固定目标采集 `check`（表格 + JSON）与 `scan`（表格 + JSON）输出基线，用 `jq` 把 `rtt_ms` / `ts` / `started_at` / `finished_at` 等易变字段置空
  - 验收：同一命令连跑两次归一化后 `diff` 为空（证明基线本身稳定）
  - 需求：R7

- [ ] **T7 拆分 `connectivity` 并补时间戳**
  - 文件：`src/connectivity/mod.rs`
  - 内容：拆出 `probe_one(...) -> CheckOutput`（含 `mark_failure` 与断言评估，属逐目标语义）与 `render(&CheckOutput, OutputMode)`；`run()` 对外签名与行为不变，改为两者组合；`finish()` 的三重职责随之解开。同时补 `CheckProbe.ts` 与 `CheckOutput.started_at/finished_at`（从 T2 挪来，见 T2 下的范围调整说明）
  - 验收：与 T6 基线归一化后 `diff` 为空（`ts` 等新增字段在归一化时置空）；`cargo test` 全绿
  - 需求：R4 R5 R7

- [ ] **T8 拆分 `portscan`**
  - 文件：`src/portscan/mod.rs`
  - 内容：拆出 `probe_host(...) -> ScanOutput` 与 `render(...)`；去掉在探测路径里直接 `print_json` 的写法
  - 验收：与 T6 基线归一化后 `diff` 为空
  - 需求：R7

## 阶段三：CLI 与批量接线

- [ ] **T9 CLI 参数**
  - 文件：`src/cli.rs`、`src/main.rs`
  - 内容：`--ndjson` 加到 `Cli`、`PluginCli`、`GLOBAL_FLAGS`（三处）；`check.target` 与 `scan.host` 改 `Option<String>`；两命令加 `--targets-from`、`--parallel`、`--show-timestamp`；`scan` 加 `-p/--ports`（与位置参数 `PORTS` 互斥）；互斥与必填用 clap 的 `conflicts_with` / `required_unless_present`
  - 验收：`netutils check`（无参）退出码 2；`check a:1 --targets-from f` 退出码 2；`scan --targets-from f 80` 报互斥错误而非把 `80` 当主机；`netutils --ndjson plugin list` 不因全局开关解析失败
  - 需求：R1 R6 R12

- [ ] **T10 `check` 批量**
  - 文件：`src/connectivity/mod.rs`、`src/main.rs`
  - 内容：新增 `BatchOutput`（`mode` / `started_at` / `finished_at` / `stats` / `results`）；用 `batch::run_targets` 驱动 `probe_one`，断言列表以 `Arc` 共享；逐目标独立断言；表格模式 `--parallel > 1` 时按目标缓冲输出，`== 1` 保持实时；单目标路径不经过 `BatchOutput`
  - 验收：3 行清单 `--json` 的 `results` 顺序等于清单顺序；单个格式错误目标不中断整批；退出码符合 R8；`--parallel > 1` 时 `--interval` 仍生效（回归测试）
  - 需求：R1 R3 R7 R8 R9

- [ ] **T11 `scan` 批量**
  - 文件：`src/portscan/mod.rs`、`src/main.rs`
  - 内容：同 T10 的接线方式；端口集合以 `Arc` 共享
  - 验收：清单内每个主机使用同一份端口集合；`results` 保序；表格模式有分段标识与末尾总览
  - 需求：R1 R3 R8 R9

- [ ] **T12 NDJSON 记录输出**
  - 文件：`src/connectivity/mod.rs`、`src/portscan/mod.rs`
  - 内容：`record` 取值 `probe` / `summary` / `batch_summary` / `error`；`probe` 在每次探测完成时即输出（`scan` 用 `port` 替代 `seq`）；`summary` 每目标一行；`batch_summary` 收尾一行，被中断时带 `interrupted: true`；单目标 + NDJSON 无 `batch_summary`；`--json` 与 `--ndjson` 同时给出时以 NDJSON 生效
  - 验收：每行均为合法紧凑 JSON（`jq -c .` 逐行通过）；每行自带 `target` 与 `ts`
  - 需求：R6

## 阶段四：文档与收尾

- [ ] **T13 i18n**
  - 文件：`src/i18n.rs`
  - 内容：新增字符串（批量分段标题、批量总览、清单错误、互斥错误等）补齐 `zh` / `en`
  - 验收：新增用户可见输出无裸字面量；两种语言下跑一遍批量命令无占位符残留
  - 需求：R12

- [ ] **T14 文档**
  - 文件：`README.md`、新增 `CHANGELOG.md`
  - 内容：README 增批量巡检与 NDJSON 消费示例（含 `--targets-from -` 与管道到 `jq`）；建 `CHANGELOG.md` 记录 0.7.0 的新增字段与新参数
  - 验收：示例命令可直接复制运行
  - 需求：R12

- [ ] **T15 全量验证**
  - 文件：无
  - 内容：三道门全过；3 行清单在 `--json` / `--ndjson` / 表格三种模式下各跑一次，核对退出码与输出结构；删除 T6 的基线临时文件
  - 验收：三道门通过；工作区无临时文件残留
  - 需求：全部

---

## 提交节奏

T1 / T2 / T3 各自一次提交。T4 / T5 的新 API 在阶段三才有消费者，因此与 T9–T12 合并提交。T7 / T8 各自独立提交，且必须先过 T6 基线对比再叠加后续改动——这是 R7 的主要保障手段。
