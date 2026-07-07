//! External plugin management and dispatch.

use std::env;
use std::ffi::OsString;
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
