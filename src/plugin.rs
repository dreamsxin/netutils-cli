//! External plugin management and dispatch.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use colored::*;
use serde::Serialize;

use crate::output::{print_json, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
struct PluginInfo {
    name: String,
    binary: String,
    crate_name: String,
    installed: bool,
    path: Option<String>,
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

#[derive(Clone, Copy)]
struct KnownPlugin {
    name: &'static str,
    binary: &'static str,
    crate_name: &'static str,
}

const KNOWN_PLUGINS: &[KnownPlugin] = &[KnownPlugin {
    name: "mcp",
    binary: "netutils-mcp",
    crate_name: "netutils-plugin-mcp",
}];

pub fn install(name: &str, path: Option<&str>, force: bool, mode: OutputMode) {
    let Some(plugin) = known_plugin(name) else {
        print_error(mode, &format!("unknown plugin: {name}"));
        return;
    };

    let root = plugin_root(name);
    let mut command = Command::new("cargo");
    command.arg("install");
    if force {
        command.arg("--force");
    }
    command.arg("--root").arg(&root);

    let local_path = local_plugin_path(name);
    if let Some(path) = path.map(PathBuf::from).or(local_path) {
        command.arg("--path").arg(path);
    } else {
        command.arg(plugin.crate_name);
    }

    if mode == OutputMode::Table {
        println!(
            "{} {} -> {}",
            "Installing plugin".bold(),
            name,
            root.display()
        );
    }

    match command.status() {
        Ok(status) if status.success() => {
            if mode == OutputMode::Json {
                print_json(&serde_json::json!({
                    "installed": true,
                    "name": name,
                    "root": root.display().to_string()
                }));
            } else {
                println!("  {}", "installed".green());
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
            PluginInfo {
                name: plugin.name.to_string(),
                binary: plugin.binary.to_string(),
                crate_name: plugin.crate_name.to_string(),
                installed: path.is_some(),
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
                    if plugin.installed { "yes" } else { "no" }.to_string(),
                    plugin.path.clone().unwrap_or_else(|| "--".to_string()),
                ]
            })
            .collect::<Vec<_>>();
        print_table(&["Name", "Binary", "Crate", "Installed", "Path"], &rows);
    }
}

pub fn remove(name: &str, mode: OutputMode) {
    let dir = plugin_root(name);
    if !dir.exists() {
        print_error(mode, &format!("plugin is not installed: {name}"));
        return;
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {
            if mode == OutputMode::Json {
                print_json(&serde_json::json!({ "removed": true, "name": name }));
            } else {
                println!("{} {}", "Removed plugin".bold(), name);
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
    let binary_name = format!("netutils-{command_name}");
    let binary = installed_binary(&command_name, &binary_name)
        .or_else(|| find_in_path(&binary_name))
        .or_else(|| find_in_path(&format!("{binary_name}.exe")));

    let Some(binary) = binary else {
        print_error(
            mode,
            &format!(
                "Command `{command_name}` is not built in and plugin `{command_name}` is not installed.\nInstall it with: netutils install {command_name}"
            ),
        );
        return;
    };

    let mut child = Command::new(binary);
    if mode == OutputMode::Json {
        child.arg("--json");
    }
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

fn known_plugin(name: &str) -> Option<KnownPlugin> {
    KNOWN_PLUGINS
        .iter()
        .copied()
        .find(|plugin| plugin.name == name)
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
}
