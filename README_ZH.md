# netutils — 本地网络检测工具集

[English](README.md) | 中文

---

一个用 Rust 编写的跨平台命令行网络诊断工具。涵盖网络接口、路由、出口检测、代理检测、Ping、DNS、DNS 缓存、DNS 查询路径、路由决策、TLS 握手与证书诊断、HTTP 请求测试、HTTP 请求路径、Traceroute、端口扫描、连通性测试、连接列表、一键诊断、全链路诊断，以及插件化的 SSE/WebSocket/MCP 诊断。

### 功能

| 子命令 | 说明 | 示例 |
|--------|------|------|
| `(无)` | 显示全部网络信息 | `netutils` |
| `iface` | 网络接口列表 | `netutils iface` |
| `egress` | 流量出口 + 选路逻辑 | `netutils egress` |
| `route` | 路由表 | `netutils route` |
| `route-get` | 查询目标实际选路，判断是否走 TUN/VPN | `netutils route-get google.com` |
| `proxy` | 代理设置 | `netutils proxy` |
| `ping` | Ping 主机 (ICMP/TCP) | `netutils ping baidu.com --count 4` |
| `dns` | DNS 查询 | `netutils dns baidu.com --type mx` |
| `dns-cache` | 检查/清理系统 DNS 缓存 | `netutils dns-cache google.com` |
| `dns-path` | 查看 DNS server 及到 DNS server 的本机路由 | `netutils dns-path google.com` |
| `dns-compare` | 对比系统默认解析与指定 DNS server 直查结果 | `netutils dns-compare google.com --server 8.8.8.8` |
| `dns-leak` | 检测 DNS 是否泄露（查询是否绕过 VPN/TUN/代理） | `netutils dns-leak` |
| `proxy-test` | 检查代理连通性、DNS 行为和请求稳定性 | `netutils proxy-test google.com --proxy socks5h://127.0.0.1:7890 --count 20` |
| `tls` | TLS 握手与证书诊断 | `netutils tls google.com --sni google.com` |
| `trace` | 路由追踪 | `netutils trace baidu.com` |
| `scan` | 端口扫描 | `netutils scan 192.168.1.1 80,443` |
| `check` | 连通性测试 | `netutils check https://baidu.com` |
| `http` | 发起一次 HTTP 请求并显示响应结果 | `netutils http https://example.com --show-headers` |
| `sse` | 插件命令：测试 Server-Sent Events 流 | `netutils install sse && netutils sse https://example.com/events` |
| `ws` | 插件命令：测试 WebSocket 握手和消息收发 | `netutils install ws && netutils ws wss://echo.websocket.events --message ping` |
| `mcp` | 插件命令：测试 MCP Streamable HTTP 初始化和工具列表 | `netutils install mcp && netutils mcp https://example.com/mcp` |
| `subdomain` | 插件命令：基于公开被动源发现子域名 | `netutils install subdomain && netutils subdomain example.com` |
| `chrome-proxy` | 插件命令：通过本地链式代理桥启动 Chrome 测试代理 | `netutils install chrome-proxy && netutils chrome-proxy https://www.google.com/generate_204 --proxy socks5://127.0.0.1:7890` |
| `connections` | 网络连接列表 (TCP/UDP) | `netutils connections --state LISTEN` |
| `diag` | 一键诊断 | `netutils diag` |
| `diagnose` | 全链路诊断 (DNS→Ping→TCP→HTTPS→Trace) | `netutils diagnose baidu.com` |
| `path` | HTTP 请求路径分析 (DNS→代理/出口→Trace→TCP/TLS/HTTP) | `netutils path https://myip.ipipv.com` |

### 安装

```bash
# 从 crates.io 安装（推荐）
cargo install netutils-cli

# 安装后直接使用
netutils --help
```

### 快速开始

```bash
# 从源码编译
git clone https://github.com/dreamsxin/netutils-cli.git
cd netutils-cli
cargo build --release

# 运行
./target/release/netutils

# 查看帮助
./target/release/netutils --help

# 限制整条命令最长执行时间，超时退出码为 124
./target/release/netutils --total-timeout 30 diagnose example.com
```

默认域名解析使用操作系统 resolver，会遵循本地 hosts、VPN/split-DNS 和系统缓存。只有显式传入 `--server` 等参数时才会直查公共 DNS。

### Windows 下跨平台编译测试

如果你在 Windows 上开发，并希望提前验证 Linux / macOS 目标，可以先安装 Rust target：

```bash
rustup target add x86_64-unknown-linux-gnu
rustup target add x86_64-apple-darwin
```

Windows 本机目标建议先跑：

```bash
cargo test
cargo check --target x86_64-pc-windows-msvc
```

Linux 目标有两种常见验证方式：

```bash
# 方式 1：在 Windows 上直接做 target 编译检查
# 需要额外安装 x86_64-linux-gnu-gcc 等交叉 C 工具链
cargo check --target x86_64-unknown-linux-gnu
```

```bash
# 方式 2：使用 WSL 做真实 Linux 编译/测试（推荐）
wsl
curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
cd /mnt/d/your/path/netutils-cli
cargo test
```

如果项目依赖了 `ring`、`aws-lc-sys`、`reqwest` 等包含 C/汇编构建步骤的 crate，只有安装 Rust target 还不够；Windows 直接交叉编译 Linux 目标时，通常还需要 `x86_64-linux-gnu-gcc` 之类的交叉编译器。

macOS 目标同理：

```bash
# 仅安装 target 不足以完成构建
# 还需要 Darwin C toolchain / SDK（例如 osxcross 或等效环境）
cargo check --target x86_64-apple-darwin
```

如果本机没有 Darwin 工具链，常见错误会是 `cc` 不认识 `-arch`、`-mmacosx-version-min` 等参数。这种情况下更适合在 macOS CI 或已配置好 osxcross/zig + Apple SDK 的环境中做最终验证。

### 一键诊断

```bash
$ netutils diag

🔍 网络诊断报告  2026-06-25 14:30:00

  ✅ [出口] 网络连接正常 (出口: 以太网 192.168.50.4)
  ✅ [国内 DNS] DNS 解析正常 (baidu.com → 111.63.65.247, 45ms)
  ✅ [国际 DNS] DNS 解析正常 (google.com → 142.250.69.174, 180ms)
  ✅ [网关] 默认网关可达 (192.168.50.1, 0.5ms)
  ⚠️  [代理] 系统代理已启用 (127.0.0.1:7897)
  ✅ [国内连通] HTTPS 连通正常 (baidu.com → 200, 54ms) [经代理]
  ✅ [国际连通] HTTPS 连通正常 (google.com → 200, 1096ms) [经代理]
  ❌ [IPv6] IPv6 不可用

  诊断耗时: 8.2s
```

### 全链路诊断

对指定目标自动执行完整链路检测（DNS → Ping → TCP → HTTPS → Traceroute），并自动定位断点给出结论：

```bash
$ netutils diagnose google.com

🔍 全链路诊断: google.com

  ✅ [① DNS 解析]
     系统 DNS: google.com → 142.251.188.138 (199ms)
  ❌ [② Ping 探测]
     173.194.43.139 不可达 (100% 丢包)
  ❌ [③ TCP 端口 443]
     连接失败: timeout (3s)
  ✅ [④ HTTPS 请求]
     https://google.com → 200 (807ms) [经代理]
  ⚠️  [⑤ Traceroute (最多 10 跳)]
     未到达目标 (10 跳内)

  📍 诊断结论: 主机不可达，IP 无法 ping 通
  链路: ✅ DNS → ❌ Ping → ❌ TCP → ✅ HTTPS

  耗时: 20.2s
```

自动结论定位：DNS 失败 → "DNS 解析失败" / Ping 失败 → "主机不可达" / TCP 失败 → "端口不通" / HTTPS 失败 → "HTTPS 异常" / 全部正常 → "链路正常"

### 路由与 DNS 排障

当你需要回答“这个请求为什么走这个出口”时，用 `route-get` 查看目标 IP 的内核选路结果：

```bash
# 显示目标解析结果、选中的接口/网关、接口类型和 TUN 判断
netutils route-get google.com

# 只看路由决策，不执行快速 trace
netutils route-get google.com --no-trace
```

当你需要回答“DNS 为什么解析成这样”时，用 `dns-compare` 对比系统默认解析和指定 DNS server 的直查结果：

```bash
# 与指定 DNS server 对比
netutils dns-compare google.com --server 8.8.8.8

# 不指定 --server 时，使用系统配置的 DNS servers
netutils dns-compare google.com
```

当怀疑代理/TUN 切换后 DNS 缓存导致访问失败时：

```bash
# 查看系统 DNS 缓存中是否存在目标域名，以及当前解析是否不同
netutils dns-cache google.com

# 尝试清理系统 DNS 缓存后再检查
netutils dns-cache google.com --flush
```

当需要确认 DNS 查询会发往哪些 DNS server、到 DNS server 走哪个网关/接口时：

```bash
netutils dns-path google.com
netutils dns-path google.com --server 8.8.8.8
```

当需要检查 DNS 查询是否绕过 VPN、TUN 或代理路径时：

```bash
# 使用当前系统网络和系统代理设置
netutils dns-leak

# 通过显式远程 DNS 代理观察 resolver
netutils dns-leak --proxy socks5h://127.0.0.1:1080

# 每个 DNS 探针并发执行 5 个样本（默认 3，范围 1-10）
netutils dns-leak --proxy socks5h://127.0.0.1:1080 --count 5

# 只分析本机 DNS server 和路由，不访问外部探测服务
netutils dns-leak --no-external
```

`dns-leak` 会并发执行两组 resolver 探针。Surfshark 探针查询随机的 `*.ipv4.surfsharkdns.com` 域名，显示 resolver IP、ISP、国家、城市和供应商返回的 `Leak` 标记；`ip-api-edns` 探针从 `https://edns.ip-api.com/json` 发起请求，跟随服务端生成的随机域名跳转，显示 resolver IP、国家和组织。`--count 1..10` 控制每个提供商的样本数，因此默认值 3 表示分别执行 3 个 Surfshark 和 3 个 ip-api-edns 样本。同时使用 `whoami.akamai.net` 的 A 记录作为辅助 resolver 观测，并仅使用 Cloudflare trace 获取 HTTP 公网出口。resolver IP 与 HTTP 出口 IP 会分别展示，不再假设两者必须相同。HTTP 探针会遵循显式代理或系统代理，因此 HTTP 和 `socks5h` 代理可以反映代理侧 DNS 行为；使用 `--no-proxy` 可强制直连。

对于 HTTP 和 `socks5h` 远程 DNS 代理，Surfshark 与 ip-api-edns 的 resolver 观测会合并判断。全部样本成功且国家一致时，多个 resolver IP 会被视为正常地址池；同一国家内如果 Surfshark 返回了不同的已知城市，也视为泄露信号。跨国家或跨已知城市时判定为 DNS 泄露。国家信息缺失或部分样本失败时返回 `low`（证据不足）。Surfshark 的 `Leak` 字段仍会展示，但在远程 DNS 代理模式下不会覆盖地理一致性结论。

系统 DNS 列表可能包含未启用网卡、split-DNS、scoped resolver 或本地 stub。DNS server 走不同接口只作为辅助证据，不单独证明泄露。浏览器 DoH/DoT 和应用内置 resolver 仍可能走不同路径。

当你怀疑“本地 DNS 缓存/解析不对，但代理侧 DNS 可以访问”时，用 `proxy-test` 对比本地解析和代理域名请求：

```bash
# 自动读取系统代理
netutils proxy-test google.com

# 指定代理；SOCKS 远端 DNS 建议使用 socks5h://
netutils proxy-test google.com --proxy socks5h://127.0.0.1:7890

# 只使用显式 --proxy，不读取系统代理
netutils proxy-test google.com --proxy http://127.0.0.1:7897 --no-system-proxy

# 请求 100 次、最大并发 5，评估代理稳定性
netutils proxy-test https://www.google.com --proxy socks5h://127.0.0.1:1080 --count 100 --concurrency 5
```

`proxy-test` 能判断“通过代理用域名访问是否成功”，并显示本地 DNS 结果、代理入口路由和代理入口 TCP 连通性。使用 `--count` 后还会统计成功率、HTTP 状态/错误次数以及最小、平均、P50、P95、P99、最大请求耗时。少于 5 个样本时不判定稳定性；样本足够时，成功率至少 99% 且 P95 不超过 `2 * P50 + 250ms` 才判定为 `stable`。HTTP 407 和 5xx 响应计为失败。大多数代理协议不会把代理内部最终解析出的 IP 返回给客户端，因此这里的结论是代理侧 DNS 可用性推断，而不是读取代理 DNS 的精确返回值。表格和 JSON 输出中的代理凭据会被脱敏。

系统代理会按目标协议分别选择 HTTP/HTTPS 配置，并遵循 `NO_PROXY` 和系统 bypass 列表。`netutils proxy` 会显示 PAC/WPAD 配置，但内置 HTTP client 暂不执行 PAC 脚本。

当需要排查 TLS 握手、SNI、证书链、ALPN 或证书有效期时：

```bash
netutils tls google.com
netutils tls google.com:443 --sni google.com
netutils tls https://google.com --alpn h2,http/1.1
```

`tls` 会显示 DNS、到目标 IP 的本机路由、TCP/TLS 分阶段耗时、TLS 版本、Cipher Suite、ALPN、证书链数量和证书主题/签发者/有效期。当前版本执行直连 TLS 握手；代理/TUN 下的远端链路可能仍由代理客户端隐藏。

### HTTP 请求路径分析

当你需要模拟一次 HTTP 请求并查看返回结果时：

```bash
netutils http https://example.com
netutils http https://example.com --show-headers
netutils http https://example.com --method POST --body '{"a":1}' -H "Content-Type: application/json"
netutils http https://api.ipify.org --proxy socks5h://127.0.0.1:7890
```

`http` 会显示最终 URL、状态码、总耗时、响应体预览和可选响应头。默认自动读取系统代理；使用 `--proxy` 可指定代理，使用 `--no-proxy` 可强制直连。

当需要测试流式接口或 WebSocket 时：

```bash
netutils install sse
netutils install ws
netutils sse https://example.com/events --max-events 5 --max-seconds 30
netutils sse https://example.com/events -H "Authorization: Bearer xxx" --proxy socks5h://127.0.0.1:7890
netutils ws wss://echo.websocket.events --message ping --max-messages 1
netutils ws https://example.com/socket -H "Authorization: Bearer xxx"
```

`sse` 和 `ws` 由官方插件 `netutils-sse`、`netutils-ws` 提供，安装后核心命令会把 `netutils sse ...` 和 `netutils ws ...` 转发给对应插件。`sse` 会连接 `text/event-stream` 并解析 `event/id/retry/data` 字段；`ws` 会执行 WebSocket 握手，发送可选文本消息并接收前若干条消息。当前 `ws` 先支持直连测试，代理隧道可后续增强。

当需要测试 MCP Streamable HTTP endpoint 时：

```bash
netutils install mcp
netutils mcp https://example.com/mcp
netutils mcp https://example.com/mcp -H "Authorization: Bearer xxx"
netutils mcp https://example.com/mcp --protocol-version 2025-11-25 --listen
netutils mcp https://example.com/mcp --tool tabs --args '{"action":"list"}'
```

`mcp` 由外部插件 `netutils-mcp` 提供。核心命令会把 `netutils mcp ...` 转发给已安装插件。插件会按 Streamable HTTP 传输执行 `initialize`、读取 `MCP-Session-Id`、发送 `notifications/initialized`，并默认执行 `tools/list`。服务端返回 `application/json` 或 `text/event-stream` 都会解析；`--listen` 会额外用 GET 打开 server-to-client SSE 流。

需要测试某个工具调用时，使用 `--tool` 和 `--args`：

```bash
netutils mcp https://example.com/mcp --tool search --args '{"query":"netutils"}'
netutils mcp https://example.com/mcp --tool search --args '{"query":"netutils"}' --require-tool
```

`--args` 必须是 JSON object，默认是 `{}`。`--require-tool` 会先检查 `tools/list` 中是否存在该工具，找不到则不执行 `tools/call`；如果使用 `--no-tools` 跳过工具列表，就不要同时使用 `--require-tool`。

插件管理：

```bash
# 查询可安装的已知插件及本机安装状态
netutils plugin list

# 安装、更新和删除插件
netutils install mcp
netutils install sse
netutils install ws
netutils install subdomain
netutils install chrome-proxy
netutils plugin new whois
netutils plugin validate ./whois
netutils plugin update mcp
netutils plugin update all
netutils plugin update-all
netutils plugin dir
netutils plugin remove mcp
netutils plugin remove sse
netutils plugin remove ws
```

`plugin new` 会生成一个 Rust 插件项目骨架，默认输出到当前目录下的 `<name>/`：

```bash
netutils plugin new whois
netutils plugin new whois --dir ./plugins --binary netutils-whois --crate netutils-plugin-whois
```

`plugin list` 会列出核心内置的已知插件。除非显式传入 `--path <插件 crate>`，`netutils install <name>` 始终从 crates.io 安装，不再自动查找相邻本地仓库。安装成功后会写入 `plugin-lock.json`。核心转发插件命令时还会在已选中目标系统代理时设置 `NETUTILS_EFFECTIVE_PROXY`。

`path` 用于从本机视角拆解一次 HTTP/HTTPS 请求路径：DNS、代理模式、出口接口、快速 trace、TCP/TLS/HTTP 分阶段耗时。

```bash
# 自动读取系统代理；未配置代理时直连
netutils path https://myip.ipipv.com

# 指定代理
netutils path https://myip.ipipv.com --proxy http://127.0.0.1:7897

# 强制直连，忽略系统代理
netutils path https://myip.ipipv.com --no-proxy
```

代理模式下，`path` 的 trace 明确表示“本机到代理入口”，目标 DNS 和代理后的跳点会标为不可见，不再把本地解析目标的 trace 当作实际代理链路。

命令退出码约定：成功为 `0`，检测完成但失败为 `1`，CLI 参数错误为 `2`，`--total-timeout` 到期为 `124`。报告默认脱敏认证头、Cookie、API key、token 和代理凭据。

### 核心特性

- **国际化**: 自动检测系统语言（中英文），`--lang zh|en` 可覆盖
- **JSON 输出**: `--json` 全局参数，所有子命令支持，便于脚本处理
- **颜色高亮**: 出口绿色、错误红色、虚拟网卡黄色
- **命令别名**: `a`/`i`/`e`/`r`/`rt`/`p`/`pg`/`d`/`dc`/`dp`/`dcp`/`pt`/`tl`/`t`/`s`/`c`/`h`/`event`/`websocket`/`co`/`conn`/`dx`/`dg`/`pa`
- **跨平台**: Windows (PowerShell)、Linux (`ip`/`resolvectl`)、macOS (`ifconfig`/`scutil`/`networksetup`)
- **系统代理感知**: HTTP 检测自动读取系统代理，支持 `--proxy` 和 `--no-proxy`
- **出口检测**: UDP 探测识别实际流量出口 + 解释选路逻辑
- **TUN/VPN 识别**: 结合接口类型、路由结果和出口接口判断是否走虚拟网卡
- **DNS 排障**: 支持 DNS 缓存检查、DNS server 路由、默认解析与直查对比、代理侧 DNS 可用性推断
- **超时保护**: 外部系统命令统一带超时，降低系统命令卡住导致工具无响应的风险
- **端口范围语法**: `netutils scan host 80-100,443,8080-8090`

### 系统选路原理

系统选择出口接口时通常按以下顺序决策：

1. **最长前缀匹配**：目标 IP 先匹配最精确的路由条目
2. **路由优先级/跃点比较**：Windows 上常见为 `有效跃点 = RouteMetric + InterfaceMetric`，越低越优先；Linux/macOS 也会结合路由前缀、metric、服务顺序等信息
3. **接口优先级**：跃点相同时按接口绑定顺序决定

TUN/VPN 工具（如 Mihomo、Clash、WireGuard、OpenVPN）通常会插入更精确的路由、默认路由或低 metric 路由，确保目标流量优先走虚拟网卡。`route-get` 可以直接查看某个目标最终命中的接口和网关。

### 许可

MIT
