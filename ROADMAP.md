# Roadmap

Where `netutils` is going, and what it deliberately will not become.

Current released version: **0.6.0**. Each milestone below links to a spec under
`docs/specs/` once that milestone starts; see `AGENTS.md` for the spec-driven
workflow.

## What this tool is

An **active, local-vantage, single-host** diagnostic toolkit. Every probe
originates from the machine running the command, and every command answers a
question a human is currently asking. That framing decides what belongs here:

- In: probes we can send ourselves, verdicts we can justify from the evidence,
  machine-readable output for automation.
- Out: passive packet capture, device polling (SNMP), traffic generation, and
  anything that needs an agent installed on a remote host.

## v0.7.0 — Batch and observability

**Spec:** `docs/specs/batch-and-observability/`

The tool can diagnose one target well but cannot sweep an inventory, and its
output cannot be correlated with anything else. Three gaps that are really one:

- **Multi-target input.** Every target argument is a single positional string
  (`src/cli.rs:184`, `:196`); nothing reads stdin. Sweeping 200 endpoints needs
  an external loop, and the results cannot be reassembled into one table.
- **Timestamps.** Probe records carried a sequence number and an RTT but no
  time, so output could not be aligned with other telemetry, trended, or used as
  dated evidence. Done for `ping` and `scan`; `check` still lacks it
  (`src/connectivity/mod.rs:24-32`) and picks it up together with the
  probe/render split.
- **Streaming output.** `OutputMode` has only `Table` and `Json`, everything
  goes to stdout via `println!`, and JSON is one pretty-printed blob emitted at
  the end (`src/output.rs`). Line-delimited output exists in exactly one place:
  `ping --count 0` (`src/ping/mod.rs:119-123`).

Also in this milestone, because it is cheap and currently misleading:

- **`--assert p95` is documented but non-functional.** `src/assertion.rs:225`
  normalizes `p95` to `p95_ms` and the module docs advertise `p95<800ms`, but no
  command ever populates percentile metrics — percentiles are computed only in
  `proxy-test` (`src/proxy_test.rs:497-499`), which has no `--assert`.
- **`CHANGELOG.md`**, needed before the first release with breaking JSON
  changes.

## v0.8.0 — Verdict quality

Make the tool say *why* something is unreachable instead of only *that* it is.

- **Tri-state ports: `open` / `closed` / `filtered`.** The information is
  already available and then discarded: `src/connectivity/mod.rs:227-285` keeps
  the raw `io::Error` string without classifying it, and
  `src/portscan/mod.rs:189` collapses refused, timeout and unreachable into a
  single `open: false`. The classification pattern already exists in the
  `socks-probe` plugin (`classify_tcp_error`). Breaking change to the `scan`
  JSON shape.
- **`netutils port <port>` — local and remote vantage combined.** Join the
  LISTEN state and bind address from `connections` with a loopback connect and
  an optional external probe, and emit one verdict:
  `listening_and_reachable` / `listening_but_filtered` /
  `bound_to_loopback_only` / `not_listening`. Today `diagnose` can only say
  "port blocked **or** service not running" (`src/i18n.rs:565-569`), which
  states both mutually exclusive causes because it cannot tell them apart.
- **`scan --timeout` / `--interval`.** The connect timeout is hardcoded to 1s
  (`src/portscan/mod.rs:17`) and not exposed at all (`src/cli.rs:182-190`), so
  high-latency targets are silently reported as closed. Table output also shows
  only open ports (`src/portscan/mod.rs:138-165`).
- **Widen `--assert`.** It exists on `check` and `http` only. Certificate days
  remaining (`tls`) and loss/jitter (`ping`) are the two checks most worth
  running in CI.

## v0.9.0 — Path and protocol depth

- **`trace`: privilege precheck, rDNS, per-hop loss.** A failed raw socket
  (`src/traceroute/mod.rs:268`) currently renders every hop as `*` and reports
  "not reached", hiding the real cause; `diagnose` already does this correctly
  (`src/diagnose/mod.rs:408-418`). No PTR lookup and no MTR-style accumulation
  exist. Under ECMP the three probes of a hop carry different ICMP identities
  (`src/traceroute/mod.rs:273`), so they may land on different routers.
- **`ping`: jitter and percentiles, explicit fallback.** Stats are min/max/avg
  and loss only (`src/util.rs:65-83`). Separately, `Prober::new` silently falls
  back to a TCP connect on port 80 when the ICMP socket cannot be created
  (`src/ping/mod.rs:52-56`), so a host that answers ICMP but filters TCP/80 is
  reported as 100% loss — the method must be explicit in the output.
- **Address family selection (`-4` / `-6`).** `resolve_host` takes the first
  address `getaddrinfo` returns (`src/util.rs:21-23`); `trace` does not support
  IPv6 at all (`src/traceroute/mod.rs:246-250`) and `ping` always builds an
  IPv4-kind client. `mtu` is the reference for how complete v6 support looks.
- **`--source` / `--interface` binding.** With TUN detection and egress
  analysis already present, not being able to pick the outbound path is a real
  limitation on multi-homed and VPN hosts.
- **TLS beyond parsing.** Days-remaining computation and an expiry threshold,
  IP SANs (only `DNSName` is kept today, `src/tls_probe.rs:363-367`), and
  optional per-version probing.
- **`connections`: queue depth.** Recv-Q/Send-Q are read as column offsets and
  discarded (`src/connections/mod.rs:81-86`), so backlog build-up is invisible.

## Release engineering (continuous)

- Add `x86_64-apple-darwin` and `aarch64-unknown-linux-gnu` to the bundle
  matrix (three targets today).
- Record `plugin-lock.json` for bundled plugins so `plugin list` stops
  reporting `integrity: unrecorded` for prebuilt archives.

## Explicitly not planned

- **SYN / half-open scanning.** Needs raw sockets (root or `CAP_NET_RAW` on
  Linux, Npcap on Windows) and buys no verdict a connect scan cannot reach — it
  also only distinguishes SYN/ACK, RST and silence. The cost is privilege plus a
  platform-specific dependency.
- **Passive packet capture, SNMP polling, bandwidth generation, remote agents.**
  Different tool category; see "What this tool is".
- **UDP port scanning.** No response is indistinguishable from an open port, so
  the verdict would be too weak to act on. Reconsider only with a concrete
  protocol to speak (e.g. DNS, NTP, QUIC probes with real payloads).
