# netutils — Network Diagnostic Toolkit

English | [中文](README.md)

---

A cross-platform command-line network diagnostic tool written in Rust. Covers network interfaces, routing, egress detection, proxy detection, Ping, DNS, DNS cache, DNS query path, route decision analysis, TLS handshake and certificate diagnostics, HTTP request path analysis, Traceroute, port scanning, connectivity testing, connection listing, one-click diagnostics, and full-link diagnostics.

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
| `proxy-test` | Check whether a proxy can access a hostname and infer proxy-side DNS availability | `netutils proxy-test google.com --proxy socks5h://127.0.0.1:7890` |
| `tls` | TLS handshake and certificate diagnostics | `netutils tls google.com --sni google.com` |
| `trace` | Traceroute | `netutils trace google.com` |
| `scan` | Port scan | `netutils scan 192.168.1.1 80,443` |
| `check` | Connectivity test | `netutils check https://example.com` |
| `connections` | Network connections (TCP/UDP) | `netutils connections --state LISTEN` |
| `diag` | One-click diagnostics | `netutils diag` |
| `diagnose` | Full-link diagnostics (DNS→Ping→TCP→HTTPS→Trace) | `netutils diagnose example.com` |
| `path` | HTTP request path analysis (DNS→proxy/egress→Trace→TCP/TLS/HTTP) | `netutils path https://myip.ipipv.com` |

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
```

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

When you suspect local DNS cache or local resolution is stale while proxy-side DNS may still work, use `proxy-test`:

```bash
# Auto-detect the system proxy
netutils proxy-test google.com

# Force a proxy; use socks5h:// when you need remote DNS with SOCKS
netutils proxy-test google.com --proxy socks5h://127.0.0.1:7890

# Use only the explicit --proxy value and ignore the system proxy
netutils proxy-test google.com --proxy http://127.0.0.1:7897 --no-system-proxy
```

`proxy-test` checks whether a hostname request through the proxy succeeds, then shows local DNS answers, the local route to the proxy entrypoint, and proxy TCP reachability. Most proxy protocols do not expose the exact IP resolved inside the proxy, so the result is an availability inference rather than a remote DNS answer dump.

When you need to inspect TLS handshake, SNI, certificate chain, ALPN, or certificate validity:

```bash
netutils tls google.com
netutils tls google.com:443 --sni google.com
netutils tls https://google.com --alpn h2,http/1.1
```

`tls` shows DNS, the local route to the target IP, TCP/TLS staged timings, TLS version, cipher suite, ALPN, certificate count, and certificate subject/issuer/validity. The current version performs a direct TLS handshake; proxy/TUN downstream paths may still be hidden by the proxy client.

### HTTP Request Path Analysis

`path` breaks down an HTTP/HTTPS request from the local host perspective: DNS, proxy mode, egress interface, quick trace, and staged TCP/TLS/HTTP timings.

```bash
# Auto-detect system proxy; direct when no proxy is configured
netutils path https://myip.ipipv.com

# Force a specific proxy
netutils path https://myip.ipipv.com --proxy http://127.0.0.1:7897

# Force direct access and ignore system proxy
netutils path https://myip.ipipv.com --no-proxy
```

In proxy mode, `path` also shows `Proxy Connect`, which measures the TCP connect time from the local host to the proxy entrypoint. Downstream DNS, CONNECT, or remote egress phases may be hidden by the proxy or TUN client, so the tool also reports local egress and quick trace for context.

### Key Features

- **i18n**: Auto-detects system language (Chinese/English), `--lang zh|en` to override
- **JSON output**: `--json` flag for all commands, pipe-friendly
- **Color highlighting**: Egress in green, errors in red, virtual adapters in yellow
- **Command aliases**: `a`/`i`/`e`/`r`/`rt`/`p`/`pg`/`d`/`dc`/`dp`/`dcp`/`pt`/`tl`/`t`/`s`/`c`/`co`/`conn`/`dx`/`dg`/`pa`
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
├── README.md           # Chinese
├── README_EN.md        # English (this file)
└── src/
    ├── main.rs              # Entry: CLI dispatch
    ├── cli.rs               # Subcommand definitions (clap)
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
    ├── portscan/mod.rs      # Port scan
    ├── connectivity/mod.rs  # Connectivity test
    ├── connections/mod.rs   # Connection listing
    ├── path.rs              # HTTP request path analysis
    ├── diag/mod.rs          # One-click diagnostics
    └── diagnose/mod.rs      # Full-link diagnostics
```

### Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI parsing |
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
