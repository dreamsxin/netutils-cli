# netutils — Network Diagnostic Toolkit

English | [中文](README_ZH.md)

---

A cross-platform command-line network diagnostic tool written in Rust. Covers network interfaces, routing, egress detection, proxy detection, Ping, DNS, DNS cache, DNS query path, route decision analysis, TLS handshake and certificate diagnostics, HTTP request testing, HTTP request path analysis, Traceroute, port scanning, connectivity testing, connection listing, one-click diagnostics, full-link diagnostics, and plugin-based SSE/WebSocket/MCP diagnostics.

### Features

| Command | Description | Example |
|---------|-------------|---------|
| `(none)` | Show all network info | `netutils` |
| `iface` | Network interface list | `netutils iface` |
| `egress` | Traffic egress + routing logic | `netutils egress` |
| `route` | Routing table | `netutils route` |
| `route-get` | Show the actual route selected for a target and whether it uses TUN/VPN | `netutils route-get google.com` |
| `proxy` | Proxy settings | `netutils proxy` |
| `ping` | Ping host (ICMP/TCP) | `netutils ping google.com --count 4` |
| `dns` | DNS query | `netutils dns example.com --type mx` |
| `dns-cache` | Inspect or flush system DNS cache | `netutils dns-cache google.com` |
| `dns-path` | Show DNS servers and the local route to each DNS server | `netutils dns-path google.com` |
| `dns-compare` | Compare default resolution with direct queries to specific DNS servers | `netutils dns-compare google.com --server 8.8.8.8` |
| `dns-leak` | Detect DNS leaks (whether DNS queries bypass VPN/TUN/proxy) | `netutils dns-leak` |
| `proxy-test` | Check proxy reachability, DNS behavior, and request stability | `netutils proxy-test google.com --proxy socks5h://127.0.0.1:7890 --count 20` |
| `tls` | TLS handshake and certificate diagnostics | `netutils tls google.com --sni google.com` |
| `trace` | Traceroute | `netutils trace google.com` |
| `mtu` | Path MTU discovery and PMTUD blackhole detection | `netutils mtu google.com` |
| `scan` | Port scan | `netutils scan 192.168.1.1 80,443` |
| `check` | Connectivity test | `netutils check https://example.com` |
| `http` | Send one HTTP request and show the response result | `netutils http https://example.com --show-headers` |
| `sse` | Plugin command: test a Server-Sent Events stream | `netutils install sse && netutils sse https://example.com/events` |
| `ws` | Plugin command: test WebSocket handshake and messages | `netutils install ws && netutils ws wss://echo.websocket.events --message ping` |
| `mcp` | Plugin command: test MCP Streamable HTTP initialization and tools list | `netutils install mcp && netutils mcp https://example.com/mcp` |
| `subdomain` | Plugin command: passively discover subdomains from public sources | `netutils install subdomain && netutils subdomain example.com` |
| `chrome-proxy` | Plugin command: launch Chrome through a local chained proxy bridge | `netutils install chrome-proxy && netutils chrome-proxy https://www.google.com/generate_204 --proxy socks5://127.0.0.1:7890` |
| `connections` | Network connections (TCP/UDP) | `netutils connections --state LISTEN` |
| `diag` | One-click diagnostics | `netutils diag` |
| `diagnose` | Full-link diagnostics (DNS→Ping→TCP→HTTPS→Trace) | `netutils diagnose example.com` |
| `path` | HTTP request path analysis (DNS→proxy/egress→Trace→TCP/TLS/HTTP) | `netutils path https://myip.ipipv.com` |
| `completions` | Generate a shell completion script | `netutils completions bash > netutils.bash` |
| `man` | Generate a roff man page on stdout | `netutils man > netutils.1` |

### Installation

```bash
# Install from crates.io (recommended)
cargo install netutils-cli

# Use directly after install
netutils --help
```

### Quick Start

```bash
# Build from source
git clone https://github.com/dreamsxin/netutils-cli.git
cd netutils-cli
cargo build --release

# Run
./target/release/netutils

# Help
./target/release/netutils --help

# Put a hard limit on the whole command (exit code 124 on timeout)
./target/release/netutils --total-timeout 30 diagnose example.com
```

Default hostname resolution uses the operating-system resolver, including local hosts, VPN/split-DNS, and system cache behavior. Public resolvers are contacted only when explicitly selected with options such as `--server`.

### One-Click Diagnostics

```bash
$ netutils diag

🔍 Network Diagnostics  2026-06-25 14:30:00

  ✅ [Egress] Network connected (egress: Ethernet 192.168.50.4)
  ✅ [Domestic DNS] DNS OK (baidu.com → 111.63.65.247, 45ms)
  ✅ [Global DNS] DNS OK (google.com → 142.250.69.174, 180ms)
  ✅ [Gateway] Gateway reachable (192.168.50.1, 0.5ms)
  ⚠️  [Proxy] System proxy enabled (127.0.0.1:7897)
  ✅ [Domestic HTTP] HTTPS OK (baidu.com → 200, 54ms) [via proxy]
  ✅ [Global HTTP] HTTPS OK (google.com → 200, 1096ms) [via proxy]
  ❌ [IPv6] IPv6 unavailable

  Time: 8.2s
```

### Full-Link Diagnostics

Automatically runs a complete link check (DNS → Ping → TCP → HTTPS → Traceroute) on a target host and pinpoints the failure:

```bash
$ netutils diagnose google.com

🔍 Link Diagnostics: google.com

  ✅ [① DNS Resolution]
     System DNS: google.com → 142.251.188.138 (199ms)
  ❌ [② Ping Probe]
     173.194.43.139 unreachable (100% loss)
  ❌ [③ TCP Port 443]
     Connection failed: timeout (3s)
  ✅ [④ HTTPS Request]
     https://google.com → 200 (807ms) [via proxy]
  ⚠️  [⑤ Traceroute (max 10 hops)]
     Not reached (10 hops)

  📍 Conclusion: Host unreachable
  Chain: ✅ DNS → ❌ Ping → ❌ TCP → ✅ HTTPS

  Time: 20.2s
```

Auto-conclusion: DNS fail → "DNS resolution failed" / Ping fail → "Host unreachable" / TCP fail → "TCP port unreachable" / HTTPS fail → "HTTPS failed" / All OK → "Link healthy"

### Route And DNS Troubleshooting

When you need to answer "why does this request leave through that interface?", use `route-get` to inspect the kernel route decision for the resolved target IP:

```bash
# Show resolution results, selected interface/gateway, interface type, and TUN detection
netutils route-get google.com

# Only show route selection, skip quick trace
netutils route-get google.com --no-trace
```

When you need to answer "why does DNS resolve like this?", use `dns-compare` to compare the default system resolution path with direct queries to chosen DNS servers:

```bash
# Compare against a specific DNS server
netutils dns-compare google.com --server 8.8.8.8

# Without --server, use the DNS servers configured on the system
netutils dns-compare google.com
```

When you suspect stale DNS cache after switching proxy or TUN mode:

```bash
# Check whether the target exists in system DNS cache and whether it differs from current resolution
netutils dns-cache google.com

# Flush system DNS cache, then inspect again
netutils dns-cache google.com --flush
```

When you need to see which DNS servers the system will query and which local gateway/interface is used to reach them:

```bash
netutils dns-path google.com
netutils dns-path google.com --server 8.8.8.8
```

When you need to check whether DNS resolution escapes a VPN, TUN interface, or proxy path:

```bash
# Use the current system network and system proxy settings
netutils dns-leak

# Observe resolver behavior through an explicit remote-DNS proxy
netutils dns-leak --proxy socks5h://127.0.0.1:1080

# Run five samples per DNS probe provider (default: 3, range: 1-10)
netutils dns-leak --proxy socks5h://127.0.0.1:1080 --count 5

# Perform only local DNS server and route analysis
netutils dns-leak --no-external
```

`dns-leak` runs two resolver probes concurrently. Surfshark queries random `*.ipv4.surfsharkdns.com` hostnames and reports resolver IP, ISP, country, city, and its provider-defined `Leak` flag. The `ip-api-edns` probe starts at `https://edns.ip-api.com/json`, follows the service-generated random-host redirect, and reports resolver IP, country, and organization. `--count 1..10` controls the number of samples run by each provider, so the default value of 3 produces three Surfshark samples and three ip-api-edns samples. The command also queries the `whoami.akamai.net` A record as a secondary resolver observation and uses Cloudflare trace only for the HTTP egress IP. Resolver IPs and HTTP egress IPs are reported separately; they are not expected to be identical. Explicit and system proxies are honored for the HTTP probes, so HTTP and `socks5h` proxies can reveal proxy-side DNS behavior. Use `--no-proxy` to force direct requests.

For HTTP and `socks5h` remote-DNS proxies, resolver observations from Surfshark and ip-api-edns are evaluated together. Multiple resolver IPs are considered a normal resolver pool when all samples are successful and their countries agree; differing known Surfshark cities within the same country are also treated as a leak signal. Resolver IPs spanning different countries or known cities are classified as a DNS leak. Missing country data or a partial sample failure produces an inconclusive `low` result. Surfshark's `Leak` field remains visible but does not by itself override the geographic consistency result for a remote-DNS proxy.

Local DNS server lists can include inactive adapters, split-DNS entries, scoped resolvers, and loopback stubs. A different local interface is therefore treated as supporting evidence rather than proof by itself. Browser DoH/DoT and application-specific resolvers may still use a different path.

When you suspect local DNS cache or local resolution is stale while proxy-side DNS may still work, use `proxy-test`:

```bash
# Auto-detect the system proxy
netutils proxy-test google.com

# Force a proxy; use socks5h:// when you need remote DNS with SOCKS
netutils proxy-test google.com --proxy socks5h://127.0.0.1:7890

# Use only the explicit --proxy value and ignore the system proxy
netutils proxy-test google.com --proxy http://127.0.0.1:7897 --no-system-proxy

# Sample 100 requests with at most 5 in flight to assess stability
netutils proxy-test https://www.google.com --proxy socks5h://127.0.0.1:1080 --count 100 --concurrency 5
```

`proxy-test` checks whether a hostname request through the proxy succeeds, then shows local DNS answers, the local route to the proxy entrypoint, and proxy TCP reachability. With `--count` it also reports success rate, HTTP status/error counts, and min/average/P50/P95/P99/max request latency. Fewer than 5 samples are not classified; otherwise a sample is `stable` when the success rate is at least 99% and P95 is no more than `2 * P50 + 250ms`. HTTP 407 and 5xx responses count as failures. Most proxy protocols do not expose the exact IP resolved inside the proxy, so the result is an availability inference rather than a remote DNS answer dump. Proxy credentials are redacted from table and JSON output.

System proxy selection is target-aware: HTTP and HTTPS settings are selected separately, while `NO_PROXY` and platform bypass lists are honored. PAC/WPAD settings are displayed by `netutils proxy`, but are not executed by the built-in HTTP client.

When you need to inspect TLS handshake, SNI, certificate chain, ALPN, or certificate validity:

```bash
netutils tls google.com
netutils tls google.com:443 --sni google.com
netutils tls https://google.com --alpn h2,http/1.1
```

`tls` shows DNS, the local route to the target IP, TCP/TLS staged timings, TLS version, cipher suite, ALPN, certificate count, and certificate subject/issuer/validity. The current version performs a direct TLS handshake; proxy/TUN downstream paths may still be hidden by the proxy client.

### HTTP Request Path Analysis

When you need to simulate an HTTP request and inspect the response:

```bash
netutils http https://example.com
netutils http https://example.com --show-headers
netutils http https://example.com --method POST --body '{"a":1}' -H "Content-Type: application/json"
netutils http https://api.ipify.org --proxy socks5h://127.0.0.1:7890
```

`http` shows the final URL, status code, total time, response body preview, and optional response headers. It auto-detects the system proxy by default; use `--proxy` to force a proxy or `--no-proxy` to force direct access.

When you need to test streaming APIs or WebSocket endpoints:

```bash
netutils install sse
netutils install ws
netutils sse https://example.com/events --max-events 5 --max-seconds 30
netutils sse https://example.com/events -H "Authorization: Bearer xxx" --proxy socks5h://127.0.0.1:7890
netutils ws wss://echo.websocket.events --message ping --max-messages 1
netutils ws https://example.com/socket -H "Authorization: Bearer xxx"
```

`sse` and `ws` are provided by the official `netutils-sse` and `netutils-ws` plugins. After installation, the core CLI forwards `netutils sse ...` and `netutils ws ...` to the corresponding plugin. `sse` connects to `text/event-stream` and parses `event/id/retry/data` fields. `ws` performs a WebSocket handshake, sends optional text messages, and receives the first messages. `ws` currently tests direct WebSocket connections; proxy tunneling can be added separately.

When you need to test an MCP Streamable HTTP endpoint:

```bash
netutils install mcp
netutils mcp https://example.com/mcp
netutils mcp https://example.com/mcp -H "Authorization: Bearer xxx"
netutils mcp https://example.com/mcp --protocol-version 2025-11-25 --listen
```

`mcp` is provided by the external `netutils-mcp` plugin. The core CLI forwards `netutils mcp ...` to the installed plugin. The plugin performs `initialize`, captures `MCP-Session-Id`, sends `notifications/initialized`, and runs `tools/list` by default. It handles both `application/json` and `text/event-stream` responses; `--listen` additionally opens a GET server-to-client SSE stream.

Plugin management:

```bash
# Show known installable plugins and local install status
netutils plugin list

# Install, update, and remove plugins
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

`plugin new` creates a Rust plugin scaffold under `<name>/` in the current directory by default:

```bash
netutils plugin new whois
netutils plugin new whois --dir ./plugins --binary netutils-whois --crate netutils-plugin-whois
```

`plugin list` shows the known plugins built into the core, which are the plugins currently installable with `netutils install <name>`, together with supported platforms, current-host support, local install status, version, source, and binary path. The current known plugins are `chrome-proxy`, `mcp`, `sse`, `subdomain`, and `ws`. Registry installation is always used unless `--path <plugin-crate>` is explicitly supplied. After a successful install, `netutils` writes `plugin-lock.json` under the plugin install directory. It records the source, version, binary path, and core version used for installation. When dispatching a plugin command, the core also passes `NETUTILS_EFFECTIVE_PROXY` when a target-specific system proxy was selected.

`path` breaks down an HTTP/HTTPS request from the local host perspective: DNS, proxy mode, egress interface, quick trace, and staged TCP/TLS/HTTP timings.

```bash
# Auto-detect system proxy; direct when no proxy is configured
netutils path https://myip.ipipv.com

# Force a specific proxy
netutils path https://myip.ipipv.com --proxy http://127.0.0.1:7897

# Force direct access and ignore system proxy
netutils path https://myip.ipipv.com --no-proxy
```

In proxy mode, `path` traces the local path to the proxy entrypoint and labels proxy-side DNS and downstream hops as hidden. It no longer presents a locally resolved target trace as the actual proxy path.

Command exit codes are `0` for a successful probe, `1` for a completed failure, `2` for CLI usage errors, `3` for a failed `--assert`, and `124` when `--total-timeout` expires. Authentication headers, cookies, API keys, tokens, and proxy credentials are redacted from reports by default.

### Path MTU And PMTUD Blackholes

Small packets working while large transfers stall is the classic tunnel MTU failure: the path MTU dropped below the local interface MTU, and some device on the way discards the ICMP "fragmentation needed" reply, so Path MTU Discovery never converges.

```bash
# Binary-search the path MTU and classify the failure mode
netutils mtu google.com

# Narrow the search window when you already know the tunnel MTU
netutils mtu google.com --min-mtu 1200 --max-mtu 1500

# Machine-readable output for dashboards
netutils --json mtu google.com
```

`mtu` drives the system `ping` with the DF (Don't Fragment) bit set, so it needs no elevated privileges and no raw sockets. It probes the search floor first to confirm the target answers DF pings at all, then binary-searches upward. When a router advertises an exact MTU in its ICMP reply, that value is verified directly instead of being searched for.

The verdict distinguishes two very different failures:

- `reduced` — an explicit ICMP fragmentation-needed reply came back, so PMTUD works and the path MTU is simply lower than the local MTU. The report states how many bytes the tunnel consumes.
- `blackhole` — oversized packets vanish with no ICMP reply at all. PMTUD is broken and large transfers will hang. Lowering the tunnel MTU or enabling TCP MSS clamping is the usual fix.
- `inconclusive` — the target never answered a DF ping, so ICMP is filtered end to end and this method cannot measure the path.

Proxied traffic is out of scope: the proxy establishes its own path to the target, which the local host cannot probe.

### CI Assertions

`http` and `check` accept repeatable `--assert <EXPR>` conditions so a probe can gate a pipeline directly, without post-processing JSON:

```bash
# Fail the build unless the endpoint returns 200 within 500ms
netutils http https://api.example.com/health --assert status=200 --assert latency<500ms

# Require a stable success rate across 50 samples
netutils check https://api.example.com --count 50 --assert success_rate>=99% --assert latency<800ms

# Assert on the response body
netutils http https://api.example.com/health --assert 'body*="ok"'
```

Expressions are `<metric><op><value>`. Operators are `=`/`==`, `!=`, `<`, `<=`, `>`, `>=`, and `*=` for substring containment. Values accept `ms`, `s`, and `%` suffixes; latency metrics without a suffix are read as milliseconds. Metric aliases are accepted, so `latency` resolves to `latency_ms`, `p95` to `p95_ms`, and `code` to `status`.

- `http` metrics: `status`, `latency_ms`, `body`, `body_bytes`, `final_url`, `error`, `ok`
- `check` metrics: `success_rate`, `latency_ms`, `min_ms`, `max_ms`, `status`, `total`, `success`, `failed`, `check_type`, `target`

A failed assertion exits with `3`, distinct from a failed probe (`1`) and a usage error (`2`), so a pipeline can tell "the service is down" apart from "the service is up but out of budget". A malformed expression is a usage error and exits `2` before any network traffic is sent. When a metric is supported but unavailable for a given run — for example `status` after a connection error, or `latency_ms` when every probe failed — the report says so explicitly instead of claiming the metric name is unknown.

In JSON mode the results are attached to the report under `assertions`:

```bash
netutils --json http https://api.example.com --assert status=200 | jq '.assertions'
```

### Color Control

Color is enabled only when it is useful and safe:

```bash
netutils --color never iface     # force plain text
netutils --color always iface    # force color even when piped
NO_COLOR=1 netutils iface        # honored per https://no-color.org
```

Resolution order is `--color`, then JSON mode (always plain, so ANSI escapes cannot corrupt parsing), then `NO_COLOR`, `CLICOLOR_FORCE`, `NETUTILS_COLOR`, `CLICOLOR`, and finally terminal detection on stdout. Redirecting output to a file therefore produces clean text by default. The resolved choice is passed to plugin subprocesses through `NETUTILS_COLOR` so plugins behave the same as the core.

### Shell Completions And Man Page

```bash
# bash
netutils completions bash > /etc/bash_completion.d/netutils

# zsh
netutils completions zsh > "${fpath[1]}/_netutils"

# fish
netutils completions fish > ~/.config/fish/completions/netutils.fish

# PowerShell
netutils completions powershell | Out-String | Invoke-Expression

# man page
netutils man > /usr/local/share/man/man1/netutils.1
```


### Key Features

- **i18n**: Auto-detects system language (Chinese/English), `--lang zh|en` to override
- **JSON output**: `--json` flag for all commands, pipe-friendly
- **Color highlighting**: Egress in green, errors in red, virtual adapters in yellow; `--color` and `NO_COLOR` respected, and JSON output is always plain
- **CI assertions**: `--assert` on `http` and `check` with a dedicated exit code, no JSON post-processing required
- **Shell integration**: `completions` for bash/zsh/fish/powershell/elvish, plus a generated `man` page
- **Command aliases**: `a`/`i`/`e`/`r`/`rt`/`p`/`pg`/`d`/`dc`/`dp`/`dcp`/`pt`/`tl`/`t`/`m`/`s`/`c`/`h`/`event`/`websocket`/`co`/`conn`/`dx`/`dg`/`pa`
- **Cross-platform**: Windows (PowerShell), Linux (`ip`/`resolvectl`), macOS (`ifconfig`/`scutil`/`networksetup`)
- **System proxy aware**: HTTP checks auto-detect system proxy and support `--proxy` and `--no-proxy`
- **Egress detection**: UDP probe identifies actual traffic egress + explains routing logic
- **TUN/VPN detection**: Combines interface type, route result, and egress selection to explain whether traffic uses a virtual adapter
- **DNS troubleshooting**: Includes DNS cache inspection, DNS server routing, default-vs-direct resolution comparison, and proxy-side DNS availability inference
- **Timeout protection**: External system commands run with timeouts to reduce the chance of the tool hanging
- **Port range syntax**: `netutils scan host 80-100,443,8080-8090`

### Project Structure

```
netutils/
├── Cargo.toml
├── README.md           # English (default)
├── README_ZH.md        # Chinese
├── tests/
│   └── cli.rs          # Offline CLI integration tests
└── src/
    ├── main.rs              # Entry: CLI dispatch
    ├── cli.rs               # Subcommand definitions (clap)
    ├── assertion.rs         # --assert expression parsing and evaluation
    ├── color.rs             # Color resolution (--color / NO_COLOR / TTY)
    ├── i18n.rs              # Internationalization
    ├── table.rs             # Table rendering (unicode-width)
    ├── output.rs            # Output mode (Table/JSON)
    ├── util.rs              # Shared utilities
    ├── info/                # Network info detection
    │   ├── mod.rs           #   Orchestrator
    │   ├── interface.rs     #   Interface types + classification
    │   ├── interface_win.rs #   Windows (PowerShell)
    │   ├── interface_unix.rs#   Linux/macOS
    │   ├── route.rs         #   Route structures
    │   ├── route_win.rs     #   Windows routes
    │   ├── route_unix.rs    #   Linux/macOS routes
    │   ├── egress.rs        #   Egress detection (UDP probe)
    │   └── proxy.rs         #   Proxy detection
    ├── ping/mod.rs          # Ping (ICMP/TCP)
    ├── dns/mod.rs           # DNS query
    ├── dns_cache.rs         # DNS cache inspection
    ├── dns_path.rs          # DNS server path inspection
    ├── dns_compare.rs       # DNS result comparison
    ├── proxy_test.rs        # Proxy hostname/DNS availability inference
    ├── tls_probe.rs         # TLS handshake and certificate diagnostics
    ├── route_probe.rs       # Route lookup helper
    ├── route_get.rs         # Route decision analysis
    ├── traceroute/mod.rs    # Traceroute
    ├── mtu.rs               # Path MTU discovery + PMTUD blackhole detection
    ├── portscan/mod.rs      # Port scan
    ├── connectivity/mod.rs  # Connectivity test
    ├── connections/mod.rs   # Connection listing
    ├── path.rs              # HTTP request path analysis
    ├── http_client.rs       # Single HTTP request diagnostics
    ├── diag/mod.rs          # One-click diagnostics
    └── diagnose/mod.rs      # Full-link diagnostics
```

### Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI parsing |
| `clap_complete` | Shell completion generation |
| `clap_mangen` | Man page generation |
| `tokio` | Async runtime |
| `surge-ping` | ICMP ping |
| `trust-dns-resolver` | DNS queries |
| `socket2` | Raw sockets (traceroute) |
| `reqwest` | HTTP connectivity |
| `serde` / `serde_json` | JSON output |
| `colored` | Terminal colors |
| `unicode-width` | CJK table alignment |
| `anyhow` | Error handling |
| `winreg` (Windows) | Registry proxy settings |

### License

MIT
