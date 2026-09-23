# 批量与可观测性 — 设计

**Spec**: `batch-and-observability`
**对应需求**: `requirements.md` R1–R12
**状态**: 已批准；含一轮自审修订（见 0.5）

## 0.5 自审修订记录

design 批准后自审一轮，发现四个会导致实现走不通或行为与需求不符的问题，已在下文修正。保留记录以便追溯。

1. **`scan` 批量模式下无法指定端口**（阻断级）。`scan` 有两个位置参数 `[HOST] [PORTS]`，把 `HOST` 改成 `Option` 后，`netutils scan --targets-from f.txt 80,443` 会被 clap 把 `80,443` 填进第一个位置参数 `host`，进而触发与 `--targets-from` 的互斥报错——批量模式下没有任何途径传端口。修正见 §4：新增 `-p/--ports` 标志。
2. **Ctrl-C 不能用 `tokio::select!` 包住整个批量 future**（行为错误）。`select!` 在中断时会 drop 未完成的分支，正在飞的目标数据会丢，与「已在飞的目标跑完」的描述矛盾。修正见 §1.3：改用停止标志。
3. **现有 `run()` 无法直接复用于批量**（工作量被低估）。`connectivity::run` 返回 `()`，`finish()` 把断言评估与打印绑在一起（`src/connectivity/mod.rs:137-151`）；`portscan::run` 同样在 `:119-122` 内部直接打印。批量需要先拿到 output 再聚合。修正见 §1.6：新增「探测/渲染」拆分，这是本 spec 最大的改动面。
4. **并发派发受 `'static` 约束**（实现细节缺失）。`JoinSet`/`spawn` 要求 `'static + Send`，而 `check` 现在收 `&[Assertion]`。修正见 §1.3：共享输入用 `Arc`。

另有两处收窄，不影响需求达成：`--show-timestamp` 只加在 `check` / `scan`，`ping` 本轮只加 JSON 的 `ts` 字段（其表格改造属 v0.9.0）；NDJSON 的 `record: "probe"` 对 `scan` 用 `port` 字段替代 `seq`。

## 0. 未决问题的结论

requirements 里留的三个问题在此定稿。

**Q1 目标并发参数命名 → `--parallel <N>`，默认 1。**
`check` 与 `scan` 都已有 `--concurrency`，但含义是「单个目标内部的并发」——`check` 指同一目标的多次探测并发（`src/connectivity/mod.rs:397-422`），`scan` 指同一主机的多端口并发（`src/portscan/mod.rs:88-99`）。复用同一个名字会让「20 并发」这句话无法判断是 20 个目标还是 20 个端口。`--parallel` 只表示同时处理的**目标**数，与 `--concurrency` 正交，两者可同时给出。

**Q2 NDJSON 记录类型字段 → `record`，取值 `probe` / `summary` / `batch_summary` / `error`。**
选 `record` 而非 `type`，因为 `type` 是多数语言的保留字，也与 `dns --type` 的既有语义撞车。取值集合刻意做成通用的，后续命令接 NDJSON 时不必再扩。

**Q3 批量模式下 `--assert` 的语义 → 逐目标独立断言。**
聚合断言的语义无法自洽：`success_rate` 到底是「目标内成功率」还是「成功目标占比」，两种解释都合理，用户无从判断。逐目标独立则与单目标语义完全一致——断言表达的是「这一个端点是否达标」。任一目标断言失败即整体退出码 3（`src/assertion.rs:20` 的既有约定，靠 `fetch_max` 自然聚合）。跨目标的达标率如需断言，属于后续独立需求。

## 1. 关键设计决策

### 1.1 NDJSON 不新增 `OutputMode` 变体

最直觉的做法是 `enum OutputMode { Table, Json, Ndjson }`。**否决**：全仓有大量 `mode == OutputMode::Json` 形态的判断（`src/output.rs:73`、`src/plugin.rs:1199`、各命令的错误分支等），新增变体后这些分支会把 NDJSON 当成 Table 处理，是一类必然发生且难以穷举的静默漏判。

采用的做法：`OutputMode` 保持两态，NDJSON 作为**正交的流式开关**，沿用仓里 `color` 模块已有的「全局 effective 状态」范式（`crate::color::effective()`）：

```rust
// src/output.rs
static STREAMING: AtomicBool = AtomicBool::new(false);
pub fn set_streaming(on: bool);
pub fn streaming() -> bool;
```

`--ndjson` 在 `main.rs` 里同时置 `OutputMode::Json` 与 `set_streaming(true)`。于是所有既有 `== Json` 判断自动保持正确（NDJSON 本身就是 JSON），只有需要区分「逐行还是末尾一坨」的地方才查 `streaming()`。

### 1.2 时间戳自己实现，不引入日期库

`src/diag/mod.rs:399-415` 的 `current_timestamp` 与 `days_to_date`（`:418`）已经把 Unix 秒换算成年月日，注释明确写了「简单时间戳，不依赖 chrono」。本 spec 沿用该决定（R10），做法是**提取而非复制**：

新建 `src/timestamp.rs`，把 `days_to_date` / `is_leap` 原样移入并公开，新增：

```rust
/// RFC 3339 UTC，毫秒精度：2026-09-23T07:54:29.412Z
pub fn now_rfc3339_millis() -> String;
/// diag 现有的 "YYYY-MM-DD HH:MM:SS" 展示格式
pub fn now_display() -> String;
```

`src/diag/mod.rs` 改为调用 `timestamp::now_display()`，删除本地实现。**不保留两份日期换算**——这是提取的主要动机。提取后补齐单测（R10）：1970-01-01 边界、闰年 2024-02-29、跨年 2023-12-31T23:59:59、毫秒补零（如 `.007`）。

时间戳取「探测发起时刻」（R4），即在现有 `let start = Instant::now()` 同一位置取一次墙钟。`Instant` 不能转墙钟，因此是两次独立取值，两者间的偏差在微秒级，可接受。

### 1.3 批量循环抽成独立模块

不把批量循环塞进 `src/main.rs` 的 match 臂——那里已经有 250 行分发逻辑（`:67-320`）。新建 `src/batch.rs`：

```rust
/// 按 parallel 并发跑完 targets，结果按输入顺序回填。
///
/// 顺序回填而非完成顺序，是因为 --json 汇总必须与清单对齐（R3）；
/// NDJSON 的逐行输出仍然是完成即输出。
pub async fn run_targets<T, F, Fut>(targets: Vec<String>, parallel: usize, run_one: F) -> Vec<T>
where
    F: Fn(usize, String) -> Fut,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
```

实现用 `Semaphore` + `JoinSet`，按索引写回 `Vec<Option<T>>`，与 `src/portscan/mod.rs:88-99` 同一范式，不引入新的并发原语。

**`'static` 约束**（自审修订 4）：`JoinSet::spawn` 要求任务 `Send + 'static`，而 `check` 现在接收 `&[Assertion]`（`src/connectivity/mod.rs:67`）。因此跨目标共享的输入（断言列表、代理设置、端口列表）在调用 `run_targets` 前一律包成 `Arc`，闭包只捕获 `Arc` 克隆。这也是把 `run_one` 定义成 `Fn`（可多次调用）而非 `FnOnce` 的原因。

**中断处理**（R11，自审修订 2）：**不**用 `tokio::select!` 包住整个批量 future——`select!` 中断时会 drop 未完成的分支，正在飞的目标会连同已采集的数据一起丢掉。改用停止标志：

```rust
// 独立任务监听 Ctrl-C，只置标志，不取消任何在飞任务
let stop = Arc::new(AtomicBool::new(false));
tokio::spawn({ let stop = stop.clone(); async move {
    let _ = tokio::signal::ctrl_c().await;
    stop.store(true, Ordering::Relaxed);
}});
```

`run_targets` 在派发下一个目标前检查 `stop`；已派发的目标自然跑完。未派发的目标不出现在结果里，`batch_summary` 的 `targets` 计数据实反映实际执行数，并额外带 `interrupted: true`。

### 1.6 现有命令必须先拆成「探测」与「渲染」两段

（自审修订 3）批量编排需要拿到每个目标的结构化结果再聚合，而现在两个命令都把探测与输出绑死：

- `connectivity::run` 返回 `()`（`src/connectivity/mod.rs:58-87`），`finish()` 同时做三件事：`mark_failure`、评估断言、`print_json` 并返回「是否已输出」（`:137-151`）。
- `portscan::run` 在 `:119-122` 直接 `print_json` 后 `return`。

拆分方案，两个命令一致：

```rust
// 逐目标语义留在探测侧：mark_failure 与断言评估都是「这一个目标是否达标」
pub(crate) async fn probe_one(/* 原参数，去掉 mode */) -> CheckOutput;
fn render(output: &CheckOutput, mode: OutputMode);

// 对外入口保持现有签名与行为不变，只是变成两段的组合
pub async fn run(/* 不变 */) { let o = probe_one(..).await; render(&o, mode); }
```

`portscan` 同样拆 `probe_host(...) -> ScanOutput` + `render(...)`。

这是本 spec 改动面最大的一项，验收标准是**拆分后单目标的表格与 JSON 输出逐字节不变**（R7），因此该任务先独立提交、先跑一次输出对比，再叠加批量逻辑。


### 1.4 表格模式与 `--parallel > 1`：实时进度而非缓冲

表格模式下 `check` 会逐次探测打行（`src/connectivity/mod.rs:229-238`）。多目标并发时这些行会交错，不可读。

**最初的决定是按目标缓冲、目标完成后整段输出，已被推翻。** 缓冲意味着并发跑 200 个目标时，屏幕上很长时间什么都没有——一旦某个目标卡住或整批出错，用户看不出卡在哪一步，而排障恰恰是这个工具的唯一用途。这是硬要求，不接受「有序但滞后」。

采用的做法是**降低输出粒度，保持实时**：

- `--parallel == 1`（默认）：完全保持现状，逐次探测实时打行。
- `--parallel > 1`：抑制单个目标内部的逐次行（并发下它们必然交错到不可读），改为**每个目标完成时立刻打印一行**带计数器的结果：

```text
Sweeping 200 targets, parallel 16
  [  1/200] api.example.com:443      ✓ 4/4   12.34ms
  [  2/200] web.example.com:443      ✗ 0/4   请求超时
  [  3/200] 10.0.0.10:8080           ✓ 3/4   87.10ms
```

计数器是完成序号而非清单序号，因为并发下完成顺序本就不确定；每行自带目标名，乱序也能读。`--json` 汇总仍按清单顺序（R3），两者互不影响。

这条粒度规则同样适用于 `scan` 的批量模式。


### 1.5 单目标契约冻结的实现手段

R7 要求单目标 JSON 顶层结构不变。实现上不靠「小心不要改」，而是靠**结构分离**：批量结果是一个**新的**顶层类型 `BatchOutput`，内含原样的 `CheckOutput` / `ScanOutput` 数组。单目标路径完全不经过 `BatchOutput`，因此不可能被批量改动影响。

## 2. 模块改动清单

| 文件 | 改动 |
|---|---|
| `src/timestamp.rs` | **新增**。`days_to_date` / `is_leap` 从 diag 迁入；新增 `now_rfc3339_millis()`、`now_display()` |
| `src/targets.rs` | **新增**。清单解析 |
| `src/batch.rs` | **新增**。`run_targets` 并发编排 + 中断处理 |
| `src/output.rs` | 新增 `STREAMING` 状态与 `set_streaming/streaming`；新增泛型 `print_json_line` |
| `src/ping/mod.rs` | 删除私有 `print_json_line`（`:172`）改用 `output::` 版本；`ProbeResult` 加 `ts` |
| `src/connectivity/mod.rs` | `CheckProbe` 加 `ts`；`CheckOutput` 加 `started_at`/`finished_at`；流式输出；新增批量入口 |
| `src/portscan/mod.rs` | `PortResult` 加 `ts`；`ScanOutput` 加 `started_at`/`finished_at`；流式输出；新增批量入口 |
| `src/diag/mod.rs` | 删除本地 `current_timestamp`/`days_to_date`/`is_leap`，改调用 `timestamp::` |
| `src/cli.rs` | `Cli`/`PluginCli` 加 `--ndjson`；`check.target`、`scan.host` 改 `Option<String>`；两者加 `--targets-from`、`--parallel` |
| `src/main.rs` | `GLOBAL_FLAGS` 加 `("--ndjson", 1)`；`--ndjson` 生效逻辑；check/scan 分发臂区分单目标/批量 |
| `src/i18n.rs` | 新增字符串的 zh/en |
| `src/lib.rs` 或 `main.rs` 的 `mod` 声明 | 注册三个新模块 |
| `README.md` | 批量巡检与 NDJSON 消费示例 |
| `CHANGELOG.md` | **新增**，记录 0.7.0 新增字段 |

## 3. 数据契约

### 3.1 清单格式（R2）

```text
# 生产环境入口
api.example.com:443
web.example.com:443

# 内网
10.0.0.10:8080
```

解析规则：按行读 → 去 UTF-8 BOM（Windows 记事本保存的清单会带）→ 去尾部 `\r`（CRLF）→ trim → 跳过空行与 `#` 开头行。行内 `#` 不视为注释。不去重、保序。

`load_targets` 返回 `Result<Vec<String>, String>`，以下情况返回 `Err`（调用方转退出码 2）：路径不存在、读失败、有效目标数为 0。

### 3.2 单目标 `--json`（R7：仅新增字段）

```json
{
  "target": "example.com:443",
  "check_type": "tcp",
  "started_at": "2026-09-23T07:54:29.412Z",
  "finished_at": "2026-09-23T07:54:32.530Z",
  "probes": [
    { "success": true, "rtt_ms": 12.34, "ts": "2026-09-23T07:54:29.412Z",
      "status_code": null, "error": null, "timing": null }
  ],
  "stats": { "...": "不变" },
  "assertions": null
}
```

新增字段只有 `started_at`、`finished_at`、`probes[].ts` 三个。

### 3.3 批量 `--json`

```json
{
  "mode": "batch",
  "started_at": "2026-09-23T07:54:29.412Z",
  "finished_at": "2026-09-23T07:55:01.008Z",
  "stats": { "targets": 3, "succeeded": 2, "failed": 1 },
  "results": [ { "…单目标 CheckOutput 原样…" } ]
}
```

`results` 按清单顺序。目标自身失败（如格式错误）也占一个位置，其 `CheckOutput` 带错误信息（R2 末条）。

### 3.4 NDJSON（R6）

每行紧凑 JSON，自带上下文：

```json
{"record":"probe","target":"api.example.com:443","seq":0,"ts":"2026-09-23T07:54:29.412Z","success":true,"rtt_ms":12.34}
{"record":"summary","target":"api.example.com:443","ts":"2026-09-23T07:54:32.530Z","stats":{"total":4,"success":4,"success_rate":100.0},"assertions":null}
{"record":"probe","target":"web.example.com:443","seq":0,"ts":"2026-09-23T07:54:32.531Z","success":false,"error":"请求超时"}
{"record":"batch_summary","ts":"2026-09-23T07:55:01.008Z","stats":{"targets":3,"succeeded":2,"failed":1}}
```

- `probe` 行在每次探测完成时立即输出，不等目标结束。`scan` 的 `probe` 行用 `port` 字段替代 `seq`（端口不是时间序列），其余字段同构。
- `summary` 每目标一行。
- `batch_summary` 仅批量模式输出，全部结束后一行；被 Ctrl-C 中断时额外带 `"interrupted": true`。
- 单目标 + NDJSON 只有 `probe` 与 `summary`，无 `batch_summary`。
- 同时给 `--json` 与 `--ndjson` 时 NDJSON 生效，不报错（R6）。

## 4. CLI 变更

```
全局：
  --ndjson                  行分隔 JSON 输出，逐条即时输出（隐含 --json）

check [TARGET]
  --targets-from <FILE|->   从文件或标准输入读取目标清单，与 TARGET 互斥
  --parallel <N>            同时探测的目标数（默认 1）。> 1 时表格模式按目标缓冲输出
  --show-timestamp          表格模式下每条探测带时间戳

scan [HOST] [PORTS]
  -p, --ports <LIST>        端口列表；与位置参数 PORTS 互斥
  --targets-from <FILE|->   从文件或标准输入读取主机清单，与 HOST 互斥
  --parallel <N>            同上
  --show-timestamp          同上
```

**为什么 `scan` 要新增 `-p/--ports`**（自审修订 1）：`scan` 有两个位置参数，把 `HOST` 改成 `Option<String>` 后，clap 按顺序填充位置参数，`netutils scan --targets-from f.txt 80,443` 会把 `80,443` 填进 `host`，触发与 `--targets-from` 的互斥报错。用户在批量模式下就没有任何途径指定端口了。因此批量模式必须用 `--ports`；位置参数 `PORTS` 保留，仅为单主机的既有用法兼容。两者同时给出时报退出码 2。

**`--show-timestamp` 的范围收窄**：只加在 `check` 与 `scan`。`ping` 本轮只补 JSON 的 `ts` 字段（R4 的字段要求），表格输出不动——`ping` 的表格与统计改造（抖动、百分位、显式降级）属 v0.9.0，届时一并处理，避免本 spec 触碰 `ping` 的渲染路径。不做成全局开关，因为它只对有逐条探测输出的命令有意义，而全局开关要同步改三处（`Cli` / `PluginCli` / `GLOBAL_FLAGS`）。


## 5. 被否决的替代方案

- **加 `chrono`/`time` 依赖** — 违背既有决定（AGENTS.md），且手写换算已存在，只差 RFC3339 格式化与毫秒位。
- **`OutputMode` 增加第三变体** — 见 1.1，会造成全仓 `== Json` 判断的静默漏判。
- **`--output <FILE>` 写文件** — NDJSON + shell 重定向已满足需求，且会迫使 `src/output.rs` 从 `println!` 改成 writer 抽象，影响面远大于收益。README:521 已把重定向作为既有设计说明。
- **复用 `--concurrency` 表示目标并发** — 见 Q1。
- **批量结果复用单目标顶层结构（如 `results` 直接做顶层数组）** — 会让消费方无法区分「单目标」与「一个目标的批量」，也丢掉 `batch_summary`。
- **CIDR 展开** — 需求已列为非目标，留待后续 spec。

## 6. 风险与缓解

- **`--parallel` 与 `--interval` 的交互**：`check --concurrency > 1` 现在会完全绕过 `--interval`（`src/connectivity/mod.rs:397-422`）。`--parallel` 必须不重复这个坑——它只并行**目标**，每个目标内部仍严格遵守 `--interval`。需在实现中有测试固定该行为。
- **批量下 `mark_failure` 的粒度**：现有 `check` 在 0 成功时调 `mark_failure()`（`:138-140`）。批量时每个目标各自调用，靠 `fetch_max` 聚合，语义天然正确，不需要额外逻辑。但要确认「部分目标成功」不会被误判成整体成功。
- **stdin 与 `--parallel`**：stdin 一次性读完再分发，不做流式读取，避免边读边探测时的背压问题。清单规模按「万行以内」设计，不做分页。
- **`diag` 时间戳格式回归**：提取 `current_timestamp` 时必须保持 `diag` 输出的字符串格式逐字符不变，否则会破坏 `diag` 的既有 JSON 契约。

## 7. 测试策略

- `timestamp`：纯函数单测（闰年、跨年、毫秒补零、1970 边界）。
- `targets`：解析单测（注释、空行、CRLF、BOM、全注释文件报错、不存在的路径报错、保序不去重）。
- `batch`：`run_targets` 的顺序回填单测（构造乱序完成时间，断言结果按输入序）。
- `connectivity` / `portscan`：拆分后的输出回归（见下）、新增字段的序列化断言、`--parallel > 1` 时 `--interval` 仍生效的行为测试。
- 端到端手工验证：一份 3 行清单分别跑 `--json`、`--ndjson`、表格三种模式，核对退出码与输出结构。

**探测/渲染拆分的回归方式**（§1.6）：拆分不能靠肉眼确认。做法是在拆分**之前**用当前二进制对固定目标采集基线输出（表格与 JSON 各一份，重定向到文件），拆分后用同样命令再采一次，`diff` 必须为空。时延字段会变化，因此基线对比只针对结构——用 `jq` 把易变字段（`rtt_ms`、`ts`、`started_at`、`finished_at`）置空后再比。

**关于 `STREAMING` 全局状态的测试**：同一进程内的并行测试共享该全局，直接在测试里 `set_streaming(true)` 会串扰到其他测试。因此单测只覆盖底层的 `print_json_line` 与各命令的「给定 streaming 值应走哪条分支」的纯逻辑，`set_streaming` 仅由 `main` 在启动时调用一次，不在测试中修改。

