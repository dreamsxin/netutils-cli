# AGENTS.md

Working agreement for AI agents and humans in this repository. Read this before
touching code; it encodes decisions that are not obvious from the source.

## Spec-driven development (SDD)

Non-trivial work goes through a spec before any code is written. Specs live in
`docs/specs/<feature-slug>/` and consist of three files, written in that order:

1. **`requirements.md`** — what problem, for whom, and the acceptance criteria.
   Written in Chinese. Each requirement is testable and numbered (`R1`, `R2`…).
   No implementation detail here.
2. **`design.md`** — how. Modules touched, new types, data/JSON contracts,
   alternatives considered and rejected with reasons. Cite existing code as
   `path:line`. Written in Chinese.
3. **`tasks.md`** — an ordered checklist. Every task names the files it touches,
   its acceptance check, and which requirement it satisfies. Written in Chinese.

**Gates — do not skip:**

- Do not write `design.md` until `requirements.md` is approved by the user.
- Do not write `tasks.md` until `design.md` is approved.
- **Do not write production code until `tasks.md` is approved.**
- Work tasks in order. Tick the checkbox in `tasks.md` immediately after a task
  passes its acceptance check, in the same commit as the code.
- If implementation reveals the design was wrong, stop and update `design.md`
  first. Do not let code and spec diverge silently.

A spec is done when every box in `tasks.md` is ticked and the verification gates
below pass. Keep finished specs in the tree — they are the design record.

Trivial work (typo, one-line fix, dependency bump) skips SDD.

## Verification gates

These are the same gates CI runs. Run all three before reporting work as done.

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

In `netutils-plugins` the clippy and test commands take `--workspace`.

Notes:

- `-D warnings` means clippy suggestions are hard errors. A new lint from a
  toolchain bump can fail an untouched branch; that is why `ci.yml` has a
  non-blocking `latest-stable` job.
- The toolchain is pinned to **1.98.1** by `rust-toolchain.toml` in both repos.
  Do not bump it as a side effect of another change.
- Cargo.lock is committed in both repos and the release workflow builds with
  `--locked`. If you change dependencies, commit the updated lock.

## The two repositories

- **`netutils-cli`** — the `netutils` binary. A single package, **not** a cargo
  workspace, one `[[bin]]`.
- **`netutils-plugins`** — a cargo workspace: the `netutils-plugin-sdk` crate
  plus one crate per plugin. `cargo build --release --workspace` produces every
  plugin binary in one directory.

Plugins are **separate child processes**, not linked in. The core resolves
`netutils-<command>` under the plugin directory first, then `PATH`
(`src/plugin.rs:755-778`). The plugin directory is `NETUTILS_PLUGIN_DIR`, else
`~/.netutils/plugins` (`src/plugin.rs:1173-1188`), and a plugin binary lives at
`<dir>/<plugin>/bin/netutils-<plugin>[.exe]`. `ws` ships two binaries and the
`websocket` alias maps to the same `ws` plugin directory
(`external_command_target`, `src/plugin.rs:785-800`).

Consequence for release packaging: the bundle layout must match that lookup, and
the plugin list must be **derived** (from `cargo metadata` on the plugins
workspace), never hand-written — a hand-written list already silently dropped a
plugin once.

## Conventions

**Language.** Code comments and spec documents in Chinese. `README.md`,
`ROADMAP.md`, this file, and commit messages in English. **`CHANGELOG.md` is in
Chinese** — it predates the rest and its entries explain *why* a change was
needed, often with the measurement that motivated it; match that style rather
than writing terse bullets.

**Comments explain why, not what.** The valuable comments in this codebase
record a decision and the failure that motivated it — e.g. why `plugin` gets its
own clap parser (Windows 1 MB stack overflow on clap's recursive command-tree
walk, `src/main.rs:346-351`), or why the crates.io publish check uses the sparse
index instead of `cargo info`. Match that. Do not narrate the code.

**Commit messages.** Imperative subject under ~70 chars, then a body explaining
*why* the change was needed — what was broken, what it would have caused. No
emoji, no trailers, no tool attribution.

**No new dependencies without a reason in the design doc.** In particular
**there is a standing decision not to depend on `chrono` or `time`**: the
calendar arithmetic lives in `src/timestamp.rs`, hand-rolled, originally carrying
the comment "简单时间戳，不依赖 chrono" in `diag`. Extend that module rather than
adding a date crate.

**JSON output is a public contract.** Scripts and CI assertions consume it.
Adding fields is fine; renaming or changing the type of a field is a breaking
change that needs a minor version bump and a `CHANGELOG.md` entry. Single-target
output shapes must stay stable when multi-target support is added.

**Exit codes** (`src/output.rs`, one `AtomicU8` combined with `fetch_max` so the
worst outcome wins): `0` ok, `1` probe failed, `2` CLI usage error, `3`
assertion failure, `124` `--total-timeout` expired. Plugin child codes
propagate.

**Global flags are declared in three places.** `Cli` (`src/cli.rs:14-33`),
`PluginCli` (`src/cli.rs:475-494`), and the `GLOBAL_FLAGS` table used by the
hand-rolled plugin dispatcher (`src/main.rs:384-389`). Adding one means editing
all three.

**i18n.** Every user-facing string goes through `t()` / `t2()` in
`src/i18n.rs`, with both `zh` and `en` entries. No bare literals in output.

## Git

- Commit only when asked. Stage specific paths, not `git add .`.
- Never force-push, reset --hard, or amend a pushed commit.
- `dist/`, `.plugins-src/` and `netutils-v*.{zip,tar.gz,sha256}` are build
  artifacts and gitignored.

## Release process

Tag-triggered (`.github/workflows/release.yml`), five jobs:

`resolve` → `publish` → `create-release` → `bundle` (3-target matrix) →
`finalize-release`

- `resolve` is the single source of truth: it asserts the tag matches
  `Cargo.toml`, resolves the tag's commit so the manual-dispatch path builds the
  tag and not branch HEAD, and pins `PLUGINS_REF` to a concrete commit so all
  three platforms bundle the same plugin source.
- The GitHub Release is created as a **draft** and only flipped public by
  `finalize-release` after all three matrix legs succeed — otherwise a partial
  failure would publish a release with missing assets.
- A workflow-level `concurrency: { group: release }` serializes runs. The key
  must not be `github.ref`: tag pushes and manual dispatches have different
  refs, so that key would fail to serialize the exact case it is meant to guard.
- Publishing to crates.io in `netutils-plugins` requires the SDK first and all
  workspace crate versions aligned; the publish matrix is derived from workspace
  members.

## Environment quirks

- **SSH to GitHub is intermittent here.** `git push` may fail with
  `Connection reset by ... port 443` and succeed seconds later. Retry before
  diagnosing anything else.
- **Windows is the primary development host**; the shell is PowerShell. It has
  no heredoc — pass multi-line commit messages with backtick-n escapes or
  `-F <file>`. Bash is available through WSL.
- The plugins workspace and the core have separate `target/` directories.
