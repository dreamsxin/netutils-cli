//! External plugin management and dispatch.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use colored::*;
use serde::{Deserialize, Serialize};

use crate::output::{print_json, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
struct PluginInfo {
    name: String,
    binary: String,
    crate_name: String,
    platforms: Vec<String>,
    supported: bool,
    installed: bool,
    status: String,
    version: Option<String>,
    source: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct InstallInfo {
    installed: bool,
    name: String,
    root: String,
    binary: Option<String>,
    version: Option<String>,
    source: String,
    source_value: Option<String>,
    lock_path: Option<String>,
    lock_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PluginLock {
    name: String,
    binary: String,
    crate_name: String,
    version: Option<String>,
    source: String,
    source_value: Option<String>,
    installed_at_unix: u64,
    binary_path: String,
    core_version: String,
}

#[derive(Debug, Serialize)]
struct PluginDir {
    dir: String,
}

#[derive(Debug, Serialize)]
struct ScaffoldInfo {
    name: String,
    binary: String,
    crate_name: String,
    template: String,
    path: String,
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ValidationReport {
    path: String,
    ok: bool,
    checks: Vec<ValidationCheck>,
}

#[derive(Debug, Serialize)]
struct ValidationCheck {
    name: String,
    ok: bool,
    message: String,
}

#[derive(Clone, Copy)]
struct KnownPlugin {
    name: &'static str,
    binary: &'static str,
    crate_name: &'static str,
    platforms: &'static [&'static str],
}

impl KnownPlugin {
    fn supports_current_platform(self) -> bool {
        self.platforms.is_empty() || self.platforms.contains(&current_platform())
    }

    fn platform_label(self) -> String {
        if self.platforms.is_empty() {
            "all".to_string()
        } else {
            self.platforms.join(",")
        }
    }
}

const ALL_PLATFORMS: &[&str] = &["windows", "linux", "macos"];

const KNOWN_PLUGINS: &[KnownPlugin] = &[
    KnownPlugin {
        name: "mcp",
        binary: "netutils-mcp",
        crate_name: "netutils-plugin-mcp",
        platforms: ALL_PLATFORMS,
    },
    KnownPlugin {
        name: "sse",
        binary: "netutils-sse",
        crate_name: "netutils-plugin-sse",
        platforms: ALL_PLATFORMS,
    },
    KnownPlugin {
        name: "ws",
        binary: "netutils-ws",
        crate_name: "netutils-plugin-ws",
        platforms: ALL_PLATFORMS,
    },
];

pub fn install(name: &str, path: Option<&str>, force: bool, mode: OutputMode) {
    let Some(plugin) = resolve_known_plugin(name) else {
        print_error(mode, &format!("unknown plugin: {name}"));
        return;
    };
    if !plugin.supports_current_platform() {
        print_error(
            mode,
            &format!(
                "plugin `{}` does not support current platform `{}`; supported platforms: {}",
                plugin.name,
                current_platform(),
                plugin.platform_label()
            ),
        );
        return;
    }

    let root = plugin_root(plugin.name);
    let mut command = Command::new("cargo");
    command.arg("install");
    if force {
        command.arg("--force");
    }
    command.arg("--root").arg(&root);

    let explicit_path = path.map(PathBuf::from);
    let local_path = if explicit_path.is_none() {
        local_plugin_path(plugin.name)
    } else {
        None
    };
    let (source, source_value) = if let Some(path) = &explicit_path {
        ("path".to_string(), Some(path.display().to_string()))
    } else if let Some(path) = &local_path {
        ("local".to_string(), Some(path.display().to_string()))
    } else {
        ("registry".to_string(), Some(plugin.crate_name.to_string()))
    };

    if let Some(path) = explicit_path.or(local_path) {
        command.arg("--path").arg(path);
    } else {
        command.arg(plugin.crate_name);
    }

    if mode == OutputMode::Table {
        println!(
            "{} {} -> {}",
            "Installing plugin".bold(),
            plugin.name,
            root.display()
        );
    }

    match command.status() {
        Ok(status) if status.success() => {
            let binary_path = installed_binary(plugin.name, plugin.binary);
            let version = binary_path.as_ref().and_then(|path| binary_version(path));
            let (lock_path, lock_error) = if let Some(binary_path) = &binary_path {
                let lock = PluginLock {
                    name: plugin.name.to_string(),
                    binary: plugin.binary.to_string(),
                    crate_name: plugin.crate_name.to_string(),
                    version: version.clone(),
                    source: source.clone(),
                    source_value: source_value.clone(),
                    installed_at_unix: unix_now(),
                    binary_path: binary_path.display().to_string(),
                    core_version: env!("CARGO_PKG_VERSION").to_string(),
                };
                match write_plugin_lock(plugin.name, &lock) {
                    Ok(path) => (Some(path.display().to_string()), None),
                    Err(err) => (None, Some(err)),
                }
            } else {
                (
                    None,
                    Some("installed binary was not found after cargo install".to_string()),
                )
            };
            let info = InstallInfo {
                installed: true,
                name: plugin.name.to_string(),
                root: root.display().to_string(),
                binary: binary_path.map(|path| path.display().to_string()),
                version,
                source,
                source_value,
                lock_path,
                lock_error,
            };
            if mode == OutputMode::Json {
                print_json(&info);
            } else {
                println!("  {}", "installed".green());
                if let Some(version) = &info.version {
                    println!("  version: {version}");
                }
                if let Some(lock_path) = &info.lock_path {
                    println!("  lock: {lock_path}");
                }
                if let Some(err) = &info.lock_error {
                    println!("  {}", format!("lock warning: {err}").yellow());
                }
            }
        }
        Ok(status) => print_error(mode, &format!("cargo install failed with status {status}")),
        Err(err) => print_error(mode, &format!("failed to run cargo install: {err}")),
    }
}

pub fn list(mode: OutputMode) {
    let plugins = KNOWN_PLUGINS
        .iter()
        .map(|plugin| {
            let path = installed_binary(plugin.name, plugin.binary);
            let lock = read_plugin_lock(plugin.name);
            let status = plugin_status(path.is_some(), lock.is_some());
            PluginInfo {
                name: plugin.name.to_string(),
                binary: plugin.binary.to_string(),
                crate_name: plugin.crate_name.to_string(),
                platforms: plugin
                    .platforms
                    .iter()
                    .map(|platform| (*platform).to_string())
                    .collect(),
                supported: plugin.supports_current_platform(),
                installed: path.is_some(),
                status,
                version: lock.as_ref().and_then(|lock| lock.version.clone()),
                source: lock.map(|lock| {
                    lock.source_value
                        .map(|value| format!("{}:{value}", lock.source))
                        .unwrap_or(lock.source)
                }),
                path: path.map(|path| path.display().to_string()),
            }
        })
        .collect::<Vec<_>>();

    if mode == OutputMode::Json {
        print_json(&plugins);
    } else {
        let rows = plugins
            .iter()
            .map(|plugin| {
                vec![
                    plugin.name.clone(),
                    plugin.binary.clone(),
                    plugin.crate_name.clone(),
                    if plugin.platforms.is_empty() {
                        "all".to_string()
                    } else {
                        plugin.platforms.join(",")
                    },
                    if plugin.supported { "yes" } else { "no" }.to_string(),
                    if plugin.installed { "yes" } else { "no" }.to_string(),
                    plugin.status.clone(),
                    plugin.version.clone().unwrap_or_else(|| "--".to_string()),
                    plugin.source.clone().unwrap_or_else(|| "--".to_string()),
                    plugin.path.clone().unwrap_or_else(|| "--".to_string()),
                ]
            })
            .collect::<Vec<_>>();
        print_table(
            &[
                "Name",
                "Binary",
                "Crate",
                "Platforms",
                "Supported",
                "Installed",
                "Status",
                "Version",
                "Source",
                "Path",
            ],
            &rows,
        );
    }
}

pub fn update(name: &str, mode: OutputMode) {
    if name == "all" {
        update_all(mode);
    } else if let Some(plugin) = resolve_known_plugin(name) {
        install(plugin.name, None, true, mode);
    } else {
        install(name, None, true, mode);
    }
}

fn update_all(mode: OutputMode) {
    for plugin in KNOWN_PLUGINS {
        install(plugin.name, None, true, mode);
    }
}

pub fn remove(name: &str, mode: OutputMode) {
    let Some(plugin) = resolve_known_plugin(name) else {
        print_error(mode, &format!("unknown plugin: {name}"));
        return;
    };
    let dir = plugin_root(plugin.name);
    if !dir.exists() {
        print_error(mode, &format!("plugin is not installed: {name}"));
        return;
    }
    let Some(dir) = safe_plugin_dir(plugin.name) else {
        print_error(
            mode,
            &format!("refusing to remove unsafe plugin path: {}", dir.display()),
        );
        return;
    };
    match fs::remove_dir_all(&dir) {
        Ok(()) => {
            if mode == OutputMode::Json {
                print_json(&serde_json::json!({
                    "removed": true,
                    "name": plugin.name,
                    "path": dir.display().to_string()
                }));
            } else {
                println!("{} {}", "Removed plugin".bold(), plugin.name);
                println!("  path: {}", dir.display());
            }
        }
        Err(err) => print_error(mode, &format!("failed to remove plugin: {err}")),
    }
}

pub fn print_dir(mode: OutputMode) {
    let dir = plugin_base_dir();
    if mode == OutputMode::Json {
        print_json(&PluginDir {
            dir: dir.display().to_string(),
        });
    } else {
        println!("{}", dir.display());
    }
}

pub fn validate(path: &str, mode: OutputMode) {
    let root = PathBuf::from(path);
    let mut checks = Vec::new();
    push_check(
        &mut checks,
        "directory",
        root.is_dir(),
        if root.is_dir() {
            "plugin directory exists".to_string()
        } else {
            "plugin directory does not exist".to_string()
        },
    );

    let plugin_toml_path = root.join("plugin.toml");
    let cargo_toml_path = root.join("Cargo.toml");
    let readme_path = root.join("README.md");
    let main_rs_path = root.join("src").join("main.rs");

    let plugin_values = read_kv_file(&plugin_toml_path);
    push_check(
        &mut checks,
        "plugin.toml",
        plugin_values.is_some(),
        if plugin_values.is_some() {
            "plugin.toml exists".to_string()
        } else {
            "plugin.toml is missing or unreadable".to_string()
        },
    );

    let cargo_values = read_kv_file(&cargo_toml_path);
    push_check(
        &mut checks,
        "Cargo.toml",
        cargo_values.is_some(),
        if cargo_values.is_some() {
            "Cargo.toml exists".to_string()
        } else {
            "Cargo.toml is missing or unreadable".to_string()
        },
    );

    if let Some(values) = &plugin_values {
        let name = values.get("name").cloned().unwrap_or_default();
        let binary = values.get("binary").cloned().unwrap_or_default();
        let crate_name = values
            .get("crate")
            .or_else(|| values.get("crate_name"))
            .cloned()
            .unwrap_or_default();
        let platforms = values
            .get("platforms")
            .map(|value| parse_toml_string_array(value))
            .unwrap_or_default();

        push_check(
            &mut checks,
            "manifest.name",
            valid_plugin_name(&name),
            if valid_plugin_name(&name) {
                format!("plugin name `{name}` is valid")
            } else {
                "plugin name must use lowercase ASCII letters, digits, and hyphens".to_string()
            },
        );
        push_check(
            &mut checks,
            "manifest.binary",
            binary.starts_with("netutils-") && binary.len() > "netutils-".len(),
            if binary.starts_with("netutils-") {
                format!("binary `{binary}` follows netutils-* convention")
            } else {
                "binary should be named netutils-<plugin>".to_string()
            },
        );
        push_check(
            &mut checks,
            "manifest.crate",
            crate_name.starts_with("netutils-plugin-"),
            if crate_name.starts_with("netutils-plugin-") {
                format!("crate `{crate_name}` follows netutils-plugin-* convention")
            } else {
                "crate should be named netutils-plugin-<plugin>".to_string()
            },
        );
        push_check(
            &mut checks,
            "manifest.platforms",
            platforms.iter().all(|platform| valid_platform(platform)),
            if platforms.is_empty() {
                "platforms omitted; plugin is treated as all-platform".to_string()
            } else if platforms.iter().all(|platform| valid_platform(platform)) {
                format!("platforms `{}` are valid", platforms.join(","))
            } else {
                "platforms must contain only windows, linux, or macos".to_string()
            },
        );

        if let Some(cargo_values) = &cargo_values {
            let package_name = cargo_values.get("name").cloned().unwrap_or_default();
            push_check(
                &mut checks,
                "cargo.package",
                package_name == crate_name,
                if package_name == crate_name {
                    "Cargo package name matches plugin manifest".to_string()
                } else {
                    format!(
                        "Cargo package name `{package_name}` does not match manifest crate `{crate_name}`"
                    )
                },
            );
        }
    }

    push_check(
        &mut checks,
        "README.md",
        readme_path.is_file(),
        if readme_path.is_file() {
            "README.md exists".to_string()
        } else {
            "README.md is recommended for crates.io and users".to_string()
        },
    );
    push_check(
        &mut checks,
        "src/main.rs",
        main_rs_path.is_file(),
        if main_rs_path.is_file() {
            "src/main.rs exists".to_string()
        } else {
            "src/main.rs is missing".to_string()
        },
    );

    let ok = checks.iter().all(|check| check.ok);
    let report = ValidationReport {
        path: root.display().to_string(),
        ok,
        checks,
    };

    if mode == OutputMode::Json {
        print_json(&report);
    } else {
        println!("{}", "Plugin Validation".bold());
        println!("  path: {}", report.path);
        println!(
            "  ok: {}",
            if report.ok { "yes".green() } else { "no".red() }
        );
        let rows = report
            .checks
            .iter()
            .map(|check| {
                vec![
                    check.name.clone(),
                    if check.ok { "ok" } else { "failed" }.to_string(),
                    check.message.clone(),
                ]
            })
            .collect::<Vec<_>>();
        print_table(&["Check", "Status", "Message"], &rows);
    }
}

pub fn new_project(
    name: &str,
    dir: Option<&str>,
    template: &str,
    binary: Option<&str>,
    crate_name: Option<&str>,
    force: bool,
    mode: OutputMode,
) {
    if template != "rust" {
        print_error(mode, &format!("unsupported plugin template: {template}"));
        return;
    }
    if !valid_plugin_name(name) {
        print_error(
            mode,
            "invalid plugin name; use lowercase ASCII letters, digits, and hyphens",
        );
        return;
    }

    let binary = binary
        .map(str::to_string)
        .unwrap_or_else(|| format!("netutils-{name}"));
    let crate_name = crate_name
        .map(str::to_string)
        .unwrap_or_else(|| format!("netutils-plugin-{name}"));
    let base_dir = dir
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let target_dir = base_dir.join(name);

    if target_dir.exists() && !force {
        print_error(
            mode,
            &format!(
                "target already exists: {}\nUse --force to overwrite template files.",
                target_dir.display()
            ),
        );
        return;
    }

    let files = scaffold_files(name, &binary, &crate_name);
    for (relative, contents) in &files {
        let path = target_dir.join(relative);
        if path.exists() && !force {
            print_error(
                mode,
                &format!(
                    "file already exists: {}\nUse --force to overwrite it.",
                    path.display()
                ),
            );
            return;
        }
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                print_error(
                    mode,
                    &format!("failed to create {}: {err}", parent.display()),
                );
                return;
            }
        }
        if let Err(err) = fs::write(&path, contents) {
            print_error(mode, &format!("failed to write {}: {err}", path.display()));
            return;
        }
    }

    let info = ScaffoldInfo {
        name: name.to_string(),
        binary,
        crate_name,
        template: template.to_string(),
        path: target_dir.display().to_string(),
        files: files
            .iter()
            .map(|(relative, _)| relative.display().to_string())
            .collect(),
    };

    if mode == OutputMode::Json {
        print_json(&info);
    } else {
        println!("{} {}", "Created plugin".bold(), info.name);
        println!("  path: {}", info.path);
        println!("  binary: {}", info.binary);
        println!("  crate: {}", info.crate_name);
        println!();
        println!("Next:");
        println!("  cd {}", info.path);
        println!("  cargo run -- --help");
        println!("  netutils install {} --path . --force", info.name);
    }
}

pub fn run_external(args: Vec<OsString>, mode: OutputMode) {
    let Some((command_name, rest)) = args.split_first() else {
        print_error(mode, "empty external command");
        return;
    };
    let command_name = command_name.to_string_lossy().to_string();
    let target = external_command_target(&command_name);
    let binary = installed_binary(&target.plugin_name, &target.binary_name)
        .or_else(|| find_in_path(&target.binary_name))
        .or_else(|| find_in_path(&format!("{}.exe", target.binary_name)));

    let Some(binary) = binary else {
        print_error(
            mode,
            &format!(
                "Command `{command_name}` is not built in and plugin `{}` is not installed.\nInstall it with: netutils install {}",
                target.plugin_name, target.plugin_name
            ),
        );
        return;
    };

    run_plugin_binary(binary, &command_name, rest, mode);
}

struct ExternalCommandTarget {
    plugin_name: String,
    binary_name: String,
}

fn external_command_target(command_name: &str) -> ExternalCommandTarget {
    match command_name {
        "event" => ExternalCommandTarget {
            plugin_name: "sse".to_string(),
            binary_name: "netutils-sse".to_string(),
        },
        "websocket" => ExternalCommandTarget {
            plugin_name: "ws".to_string(),
            binary_name: "netutils-websocket".to_string(),
        },
        _ => ExternalCommandTarget {
            plugin_name: command_name.to_string(),
            binary_name: format!("netutils-{command_name}"),
        },
    }
}

fn run_plugin_binary(binary: PathBuf, command_name: &str, rest: &[OsString], mode: OutputMode) {
    let mut child = Command::new(binary);
    if mode == OutputMode::Json {
        child.arg("--json");
    }
    child
        .env(
            "NETUTILS_OUTPUT",
            if mode == OutputMode::Json {
                "json"
            } else {
                "human"
            },
        )
        .env("NETUTILS_CORE_VERSION", env!("CARGO_PKG_VERSION"))
        .env("NETUTILS_PLUGIN_NAME", command_name)
        .env("NETUTILS_COLOR", "auto");
    child.args(rest);
    match child.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(err) => print_error(
            mode,
            &format!("failed to run plugin `{command_name}`: {err}"),
        ),
    }
}

fn scaffold_files(name: &str, binary: &str, crate_name: &str) -> Vec<(PathBuf, String)> {
    vec![
        (
            PathBuf::from("Cargo.toml"),
            cargo_toml_template(name, binary, crate_name),
        ),
        (
            PathBuf::from("plugin.toml"),
            plugin_toml_template(name, binary, crate_name),
        ),
        (
            PathBuf::from("README.md"),
            readme_template(name, binary, crate_name),
        ),
        (PathBuf::from("src").join("main.rs"), main_rs_template(name)),
    ]
}

fn cargo_toml_template(_name: &str, binary: &str, crate_name: &str) -> String {
    format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2021"
description = "netutils plugin"
license = "MIT"
repository = ""
readme = "README.md"

[[bin]]
name = "{binary}"
path = "src/main.rs"

[dependencies]
clap = {{ version = "4.5", features = ["derive"] }}
netutils-plugin-sdk = "0.1"
serde = {{ version = "1", features = ["derive"] }}
"#
    )
}

fn plugin_toml_template(name: &str, binary: &str, crate_name: &str) -> String {
    format!(
        r#"name = "{name}"
binary = "{binary}"
crate = "{crate_name}"
description = "netutils plugin"
commands = ["{name}"]
platforms = ["windows", "linux", "macos"]
"#
    )
}

fn readme_template(name: &str, binary: &str, crate_name: &str) -> String {
    format!(
        r#"# {crate_name}

External plugin for `netutils-cli`.

## Usage

```bash
netutils install {name}
netutils {name} example.com
```

During local development:

```bash
cargo run -- example.com
netutils install {name} --path . --force
netutils {name} example.com
```

Binary: `{binary}`
"#
    )
}

fn main_rs_template(name: &str) -> String {
    MAIN_RS_TEMPLATE.replace("__PLUGIN_NAME__", name)
}

const MAIN_RS_TEMPLATE: &str = r#"use clap::Parser;
use netutils_plugin_sdk::{print_json, status_text, ColorMode, OutputMode};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(name = "netutils-__PLUGIN_NAME__", version, about = "netutils plugin")]
struct Cli {
    /// JSON output
    #[arg(long)]
    json: bool,

    /// Target to inspect
    target: Option<String>,
}

#[derive(Serialize)]
struct Report {
    plugin: String,
    target: Option<String>,
    ok: bool,
    summary: Vec<String>,
}

fn main() {
    let cli = Cli::parse();
    let report = Report {
        plugin: "__PLUGIN_NAME__".to_string(),
        target: cli.target.clone(),
        ok: true,
        summary: vec!["scaffold plugin executed".to_string()],
    };

    match OutputMode::from_json_flag(cli.json) {
        OutputMode::Json => print_json(&report),
        OutputMode::Human => {
            let color = ColorMode::from_env();
            println!("__PLUGIN_NAME__ plugin");
            println!("  status: {}", status_text(report.ok, color));
            println!(
                "  target: {}",
                report.target.as_deref().unwrap_or("--")
            );
        }
    }
}
"#;

fn valid_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn plugin_status(installed: bool, has_lock: bool) -> String {
    match (installed, has_lock) {
        (true, true) => "ok".to_string(),
        (true, false) => "untracked".to_string(),
        (false, true) => "stale-lock".to_string(),
        (false, false) => "not-installed".to_string(),
    }
}

fn safe_plugin_dir(name: &str) -> Option<PathBuf> {
    let base = plugin_base_dir();
    let dir = plugin_root(name);
    let base = if base.exists() {
        fs::canonicalize(base).ok()?
    } else {
        base
    };
    let dir = fs::canonicalize(dir).ok()?;
    if dir.starts_with(&base) && dir.file_name().and_then(|value| value.to_str()) == Some(name) {
        Some(dir)
    } else {
        None
    }
}

fn plugin_lock_path(name: &str) -> PathBuf {
    plugin_root(name).join("plugin-lock.json")
}

fn read_plugin_lock(name: &str) -> Option<PluginLock> {
    let text = fs::read_to_string(plugin_lock_path(name)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_plugin_lock(name: &str, lock: &PluginLock) -> Result<PathBuf, String> {
    let path = plugin_lock_path(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let text = serde_json::to_string_pretty(lock).map_err(|err| err.to_string())?;
    fs::write(&path, text).map_err(|err| err.to_string())?;
    Ok(path)
}

fn binary_version(path: &PathBuf) -> Option<String> {
    let output = Command::new(path).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn push_check(checks: &mut Vec<ValidationCheck>, name: &str, ok: bool, message: String) {
    checks.push(ValidationCheck {
        name: name.to_string(),
        ok,
        message,
    });
}

fn read_kv_file(path: &PathBuf) -> Option<std::collections::BTreeMap<String, String>> {
    let text = fs::read_to_string(path).ok()?;
    let mut values = std::collections::BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = parse_toml_scalar(value.trim());
        values.entry(key).or_insert(value);
    }
    Some(values)
}

fn parse_toml_scalar(value: &str) -> String {
    let value = value.trim();
    if let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        value.to_string()
    } else if let Some(value) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        value.to_string()
    } else {
        value.to_string()
    }
}

fn parse_toml_string_array(value: &str) -> Vec<String> {
    let value = value.trim();
    let Some(value) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Vec::new();
    };
    value
        .split(',')
        .map(parse_toml_scalar)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn current_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        std::env::consts::OS
    }
}

fn valid_platform(value: &str) -> bool {
    matches!(value, "windows" | "linux" | "macos")
}

#[cfg(test)]
fn known_plugin(name: &str) -> Option<KnownPlugin> {
    KNOWN_PLUGINS
        .iter()
        .copied()
        .find(|plugin| plugin.name == name)
}

fn resolve_known_plugin(value: &str) -> Option<KnownPlugin> {
    KNOWN_PLUGINS
        .iter()
        .copied()
        .find(|plugin| plugin.name == value || plugin.binary == value || plugin.crate_name == value)
}

fn installed_binary(name: &str, binary: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_string()
    };
    let path = plugin_root(name).join("bin").join(exe);
    path.exists().then_some(path)
}

fn plugin_base_dir() -> PathBuf {
    env::var_os("NETUTILS_PLUGIN_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("USERPROFILE")
                .map(|home| PathBuf::from(home).join(".netutils").join("plugins"))
        })
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".netutils").join("plugins"))
        })
        .unwrap_or_else(|| PathBuf::from(".netutils").join("plugins"))
}

fn plugin_root(name: &str) -> PathBuf {
    plugin_base_dir().join(name)
}

fn local_plugin_path(name: &str) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    let candidate = cwd
        .parent()
        .unwrap_or(&cwd)
        .join("netutils-plugins")
        .join("plugins")
        .join(name);
    candidate.exists().then_some(candidate)
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|path| path.join(binary))
        .find(|path| path.exists())
}

fn print_error(mode: OutputMode, message: &str) {
    if mode == OutputMode::Json {
        print_json(&serde_json::json!({ "error": message }));
    } else {
        eprintln!("{}", message.red());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_plugin_names() {
        assert!(valid_plugin_name("mcp"));
        assert!(valid_plugin_name("dns-extra"));
        assert!(valid_plugin_name("x1"));
        assert!(!valid_plugin_name(""));
        assert!(!valid_plugin_name("Mcp"));
        assert!(!valid_plugin_name("-mcp"));
        assert!(!valid_plugin_name("mcp-"));
        assert!(!valid_plugin_name("mcp_extra"));
    }

    #[test]
    fn scaffold_uses_expected_names() {
        let files = scaffold_files("whois", "netutils-whois", "netutils-plugin-whois");
        let cargo = files
            .iter()
            .find(|(path, _)| path == &PathBuf::from("Cargo.toml"))
            .map(|(_, contents)| contents)
            .unwrap();
        assert!(cargo.contains("name = \"netutils-plugin-whois\""));
        assert!(cargo.contains("name = \"netutils-whois\""));
    }

    #[test]
    fn parses_simple_toml_scalars() {
        assert_eq!(parse_toml_scalar("\"mcp\""), "mcp");
        assert_eq!(parse_toml_scalar("'mcp'"), "mcp");
        assert_eq!(parse_toml_scalar("[\"mcp\"]"), "[\"mcp\"]");
    }

    #[test]
    fn parses_toml_string_arrays() {
        assert_eq!(
            parse_toml_string_array("[\"windows\", \"linux\"]"),
            vec!["windows".to_string(), "linux".to_string()]
        );
    }

    #[test]
    fn known_plugins_support_current_platform() {
        assert!(KNOWN_PLUGINS
            .iter()
            .all(|plugin| plugin.supports_current_platform()));
    }

    #[test]
    fn plugin_platform_support_checks_current_platform() {
        static UNSUPPORTED_ON_WINDOWS: &[&str] = &["linux"];
        static UNSUPPORTED_OFF_WINDOWS: &[&str] = &["windows"];

        let current_only = KnownPlugin {
            name: "current-only",
            binary: "netutils-current-only",
            crate_name: "netutils-plugin-current-only",
            platforms: ALL_PLATFORMS,
        };
        let unsupported = KnownPlugin {
            name: "unsupported",
            binary: "netutils-unsupported",
            crate_name: "netutils-plugin-unsupported",
            platforms: if current_platform() == "windows" {
                UNSUPPORTED_ON_WINDOWS
            } else {
                UNSUPPORTED_OFF_WINDOWS
            },
        };

        assert!(current_only.supports_current_platform());
        assert!(!unsupported.supports_current_platform());
    }

    #[test]
    fn reports_plugin_status() {
        assert_eq!(plugin_status(true, true), "ok");
        assert_eq!(plugin_status(true, false), "untracked");
        assert_eq!(plugin_status(false, true), "stale-lock");
        assert_eq!(plugin_status(false, false), "not-installed");
    }

    #[test]
    fn all_is_reserved_for_update_all() {
        assert!(valid_plugin_name("all"));
        assert!(known_plugin("all").is_none());
    }

    #[test]
    fn resolves_known_plugin_by_name_binary_or_crate() {
        assert_eq!(
            resolve_known_plugin("sse").map(|plugin| plugin.name),
            Some("sse")
        );
        assert_eq!(
            resolve_known_plugin("netutils-sse").map(|plugin| plugin.name),
            Some("sse")
        );
        assert_eq!(
            resolve_known_plugin("netutils-plugin-sse").map(|plugin| plugin.name),
            Some("sse")
        );
    }

    #[test]
    fn external_command_aliases_resolve_to_official_plugins() {
        let event = external_command_target("event");
        assert_eq!(event.plugin_name, "sse");
        assert_eq!(event.binary_name, "netutils-sse");

        let websocket = external_command_target("websocket");
        assert_eq!(websocket.plugin_name, "ws");
        assert_eq!(websocket.binary_name, "netutils-websocket");

        let custom = external_command_target("whois");
        assert_eq!(custom.plugin_name, "whois");
        assert_eq!(custom.binary_name, "netutils-whois");
    }
}
