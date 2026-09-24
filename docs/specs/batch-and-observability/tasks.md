# 批量与可观测性 — 任务

**Spec**: `batch-and-observability`
**对应**: `requirements.md` R1–R12，`design.md` §0–§7
**状态**: 实施中

约定：每个任务完成并通过其验收检查后立刻勾选，与代码同一次提交。三道验证门（`cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`）在每个任务结束时都要过。

## 任务切分方式的修订（实施中两次撞门后重排）

原计划是**自底向上分层**：先把 `timestamp` / `targets` / `output` / `batch` 四个基础模块各提交一次，再在阶段三接线。实际连撞两次同一堵墙：

- T1 完成时 `clippy -D warnings` 因 `dead_code` 失败——`now_rfc3339_millis` 在该任务内没有调用方。
- T3/T4/T5（`load_targets`、`set_streaming`、`run_targets`）会以完全相同的方式失败，消费者都在阶段三。

`#[allow(dead_code)]` 临时糊属于本仓明令避免的过渡性残留，放宽门禁更不行。这说明**分层切分与本仓的门禁不兼容**：在 `-D warnings` 下，一个提交必须自成闭环。

因此从 T3 起改成**纵向切片**：每个任务交付一条能跑通的窄功能，基础模块随第一个用到它的切片一起进来。T1/T2 已完成，保留原编号。

---

## 已完成

- [x] **T1 提取时间戳模块**
  - 文件：新增 `src/timestamp.rs`；改 `src/diag/mod.rs`、`src/main.rs`（`mod` 声明）
  - 内容：把 `current_timestamp` / `days_to_date` / `is_leap` 从 `src/diag/mod.rs` 迁入；新增 `now_rfc3339_millis()` 与 `now_display()`；`diag` 改为调用，**不保留第二份日期换算**
  - 验收：`diag` 的时间戳字符串格式与改动前逐字符一致；单测覆盖 1970-01-01、2024-02-29、2023-12-31T23:59:59、毫秒补零
  - 需求：R4 R5 R10

- [x] **T2 探测级与运行级时间戳字段（`ping` / `scan`）**
  - 文件：`src/ping/mod.rs`、`src/portscan/mod.rs`
  - 内容：`ProbeResult.ts`、`PortResult.ts`（取值时机为探测**发起**时）；`PingOutput` / `ScanOutput` 的 `started_at` / `finished_at`（起点在 DNS 解析**之前**）
  - 验收：单目标 `--json` 除新增字段外结构与 0.6.0 一致
  - 需求：R4 R5 R7

  > **范围调整**：`CheckProbe.ts` 原本也在本任务。`CheckProbe` 在 `src/connectivity/mod.rs` 有近 20 处结构体字面量构造点（`:239` `:258` `:277` `:490` `:506`，以及 `--timing` 分解路径 `:617`–`:859` 十余处早返回），而 T3 正要重写这些函数的组织方式；先加字段等于把同一批字面量改两遍。因此挪进 T3。

---

## 纵向切片

- [x] **T3 拆分 `connectivity` 并补 `check` 的时间戳**
  - 文件：`src/connectivity/mod.rs`
  - 内容：先采基线（见下），再拆出 `probe_one(..., mode) -> Option<CheckOutput>` 与 `render(&CheckOutput, OutputMode, concurrency)`；`run()` 对外签名与行为不变，改为两者组合；`finish()` 那个「返回 bool 表示是否已打印」的三重职责签名解开为纯 `finalize()`。同时补 `CheckProbe.ts` 与 `CheckOutput.started_at` / `finished_at`
  - **探测循环内的逐次实时输出留在 `probe_one` 里**，不挪到渲染阶段：那是探测过程的一部分，挪走会变成「全部探完再一次性刷屏」，违反 R9 的实时要求。`mode` 因此仍是 `probe_one` 的参数。`Option` 的 `None` 表示目标格式错误已就地报错，错误结果的结构化表示留给 T4

  - 基线方式：用当前 HEAD 构建的二进制对 `check 127.0.0.1:9`（拒绝，确定性，且不依赖外网）采表格与 JSON 两份输出，`jq` 把 `rtt_ms` / `ts` / `started_at` / `finished_at` 置空；拆分后重采，`diff` 必须为空
  - 验收：基线 `diff` 为空；三道门通过
  - 需求：R4 R5 R7

- [x] **T4 `check --targets-from`（串行）**
  - 文件：新增 `src/targets.rs`；改 `src/cli.rs`、`src/main.rs`、`src/connectivity/mod.rs`、`src/i18n.rs`
  - 内容：`load_targets(spec) -> Result<Vec<String>, String>`（`-` 读 stdin，去 BOM 与尾部 `\r`，trim，跳过空行与 `#` 起始行，保序不去重，空结果报错）；`check.target` 改 `Option<String>` 并与 `--targets-from` 互斥；新增 `BatchOutput`；**串行**遍历清单，逐目标独立断言；表格模式加分段标识与末尾总览
  - 验收：`load_targets` 单测覆盖注释/空行/CRLF/BOM/全注释报错/不存在路径报错/重复保留/保序；3 行清单 `--json` 的 `results` 与清单同序；单个格式错误目标不中断整批；无参与双给均为退出码 2；部分失败退出码 1、断言失败 3
  - 需求：R1 R2 R7 R8 R9 R12

- [x] **T5 拆分 `portscan` + `scan --targets-from`**
  - 文件：`src/portscan/mod.rs`、`src/cli.rs`、`src/main.rs`
  - 内容：拆出 `probe_host(...) -> ScanOutput` 与 `render(...)`（同样先采基线：`scan 127.0.0.1 80,443`）；`scan.host` 改 `Option<String>`；新增 `-p/--ports`（与位置参数 `PORTS` 互斥）——**没有它批量模式就无法传端口**，clap 会把端口串当成第一个位置参数；复用 `load_targets` 做串行批量
  - 验收：基线 `diff` 为空；`scan --targets-from f 80` 报互斥错误而非把 `80` 当主机；清单内每台主机用同一份端口集合
  - 需求：R1 R7 R8 R9

- [x] **T6 `scan --parallel`**
  - 文件：新增 `src/batch.rs`；改 `src/cli.rs`、`src/portscan/mod.rs`、`src/main.rs`
  - 内容：`run_targets(targets, parallel, run_one)`，`Semaphore` + `JoinSet`，按索引回填保序；先拿许可再派发（限制在飞数并提供背压）；独立任务监听 Ctrl-C 只置 `AtomicBool`，派发前检查，**不用 `select!` 取消在飞任务**；`scan` 表格模式 `parallel > 1` 时每台主机完成即打印一行带完成计数器的结果（**不缓冲**，见 design §1.4）
  - 验收：`run_targets` 3 个单测（乱序完成时间下结果仍按输入序；同时在飞数不超过 N；`parallel == 0` 视作串行）；实测 4 台主机 `--parallel 4` 时表格按完成顺序逐行出现而 JSON `results` 仍为清单顺序
  - 需求：R3 R9 R9b R11

  > **范围拆分（实施中决定）**：`check --parallel` 另起一个切片。`scan` 的所有输出都在 `render` 里，并发只需换一层编排；而 `check` 在探测循环内部就有逐次实时行（`src/connectivity/mod.rs` 五处 `mode == OutputMode::Table` 门），并发时必须先把这些行抑制掉，否则多目标交错不可读。抑制需要一个贯穿 `probe_one` / `probe_tcp` / `probe_http` 的开关，而这三个签名已经长到要 `#[allow(clippy::too_many_arguments)]`，值得单独一次改动来做，不混在本切片里。

- [x] **T6b `check --parallel`**
  - 文件：`src/connectivity/mod.rs`、`src/cli.rs`、`src/main.rs`、`src/i18n.rs`
  - 内容：复用 `batch::run_targets`；断言列表与代理以 `Arc` 共享（`JoinSet` 要求 `'static`）；并发时通过模块级 `LIVE_PROBES` 抑制单目标内部的逐次行，改为每目标完成即打印一行；`BatchOutput` 补 `interrupted`，与 `scan` 的批量结构对齐
  - 验收：实测 `--parallel 4` 一行一个目标、无逐次行；串行仍保留逐次行与统计表；单目标 JSON 基线一致；`--parallel` 下 `--interval` 仍在目标内部生效（并发 4 个目标的总耗时 6.1s ≈ 单目标耗时，说明目标间并发而目标内未跳过间隔）
  - 需求：R3 R9 R9b R11

  > **踩到仓库的老地雷并做了根治**：加上两个 `--parallel` 后 clap derive 的命令树超过 Windows 主线程 1 MB 栈，**任何**调用（含 `--version`）启动即栈溢出。这是同一问题第三次出现（0.3.17 拆 `plugin` 解析器、0.5.0 把 `completions`/`man` 移到大栈线程）。这次不再逐点规避：`main` 改为在显式 16 MB 栈的线程上跑整个 tokio 运行时，以后新增参数不必每次想起这件事。
  >
  > **顺带发现一个既有 bug（非本次引入）**：`check --interval 2` 在 `--count` 为 1/2/3 时耗时 2.1s / 6.1s / 10.1s，即睡了 `2n-1` 次而应为 `n-1` 次。用 `dist/` 里已发布的 0.6.0 二进制实测得到完全相同的数字，确认是既有问题。已记入 ROADMAP，不在本 spec 范围内修。

- [ ] **T7 `--ndjson`**
  - 文件：`src/output.rs`、`src/ping/mod.rs`、`src/connectivity/mod.rs`、`src/portscan/mod.rs`、`src/cli.rs`、`src/main.rs`
  - 内容：`output.rs` 增 `STREAMING: AtomicBool` 与 `set_streaming` / `streaming`（**不加 `OutputMode` 变体**，否则全仓 `== Json` 判断会静默漏判）；泛型 `print_json_line`（紧凑），`ping` 的私有同名函数删除改用之；`--ndjson` 加到 `Cli` / `PluginCli` / `GLOBAL_FLAGS` 三处；输出 `record` 为 `probe` / `summary` / `batch_summary` / `error` 的行（`scan` 用 `port` 替代 `seq`），中断时 `batch_summary` 带 `interrupted: true`
  - 验收：`ping --count 0 --json` 逐行输出与改动前一致；每行 `jq -c .` 通过且自带 `target` 与 `ts`；`--json` 与 `--ndjson` 同给时以 NDJSON 生效不报错
  - 需求：R6 R7 R12

- [x] **T8 文档（随切片滚动更新）**
  - 文件：`README.md`、`CHANGELOG.md`、`AGENTS.md`、`ROADMAP.md`
  - 内容：README 增 `Sweeping An Inventory` 与 `Timestamps` 两节（清单格式、批量 JSON 形状、退出码、`--parallel` 的降粒度输出、`ts`/`started_at`/`finished_at`）；`CHANGELOG.md` 补 `[0.7.0] - 未发布`
  - 验收：README 中 `--targets-from` / `-p` / `--parallel` / `ts` 均可检索到；示例可直接复制运行
  - 需求：R12

  > **复核发现两处文档滞后与一处事实错误**（已修）：
  > 1. README 在 T3–T6 四个切片期间零更新——`--targets-from`、`-p/--ports`、`--parallel`、`ts` 全部检索不到。文档改为随切片滚动更新，不再堆到最后。
  > 2. **`CHANGELOG.md` 本来就存在**（280 行，中文，条目解释「为什么」并常带实测数据）。ROADMAP 与本文件此前都写成「新增 CHANGELOG.md」，是我没读就下的结论。已改为在既有文件前置 `[0.7.0]` 段并沿用其风格；`AGENTS.md` 的语言约定补上「CHANGELOG 用中文」。
  > 3. `--show-timestamp`（design §4、requirements R4 的表格开关）**尚未实现**，另立 T9。

- [ ] **T9 `--show-timestamp`**
  - 文件：`src/cli.rs`、`src/connectivity/mod.rs`、`src/portscan/mod.rs`
  - 内容：表格模式下默认不打时间戳（避免刷屏），由该开关开启。R4 的字段部分已在 T2/T3 完成，开关部分未做
  - 需求：R4

---

## 提交节奏

每个切片一次提交。T3 与 T5 的拆分部分必须**先过基线对比再叠加其他改动**——这是 R7（单目标契约不变）的主要保障手段。
