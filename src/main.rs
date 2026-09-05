mod assertion;
mod cli;
mod color;
mod connections;
mod connectivity;
mod diag;
mod diagnose;
mod dns;
mod dns_cache;
mod dns_compare;
mod dns_leak;
mod dns_path;
mod http_client;
mod i18n;
mod icmp;
mod info;
mod mtu;
mod output;
mod path;
mod ping;
mod plugin;
mod portscan;
mod proxy_test;
mod route_get;
mod route_probe;
mod table;
mod tls_probe;
mod traceroute;
mod util;

use std::{env, ffi::OsString, io, time::Duration};

use clap::{CommandFactory, Parser};
use cli::{Cli, Commands, PluginCli, PluginCommands};
use output::OutputMode;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let raw_args: Vec<OsString> = env::args_os().collect();
    if let Some(args) = plugin_help_parse_args(&raw_args) {
        PluginCli::parse_from(args);
        return Ok(());
    }
    if let Some(plugin_pos) = plugin_command_position(&raw_args) {
        let cli = PluginCli::parse_from(plugin_parse_args(&raw_args, plugin_pos));
        i18n::init(cli.lang);
        let mode = output_mode(cli.json);
        color::init(cli.color, mode);
        run_plugin_command(cli.command, mode);
        output::exit_if_failed();
        return Ok(());
    }

    let cli = Cli::parse_from(raw_args);

    // 初始化 i18n
    i18n::init(cli.lang);

    // 确定输出模式
    let mode = output_mode(cli.json);
    color::init(cli.color, mode);
    let total_timeout = cli.total_timeout;

    let command = async move {
        match cli.command {
            None | Some(Commands::All) => info::print_all(mode),
            Some(Commands::Iface) => info::print_interfaces(mode),
            Some(Commands::Egress) => info::print_egress(mode),
            Some(Commands::Route) => info::print_routes(mode),
            Some(Commands::RouteGet {
                target,
                max_hops,
                no_trace,
            }) => route_get::run(&target, max_hops, no_trace, mode).await,
            Some(Commands::Proxy) => info::print_proxy(mode),
            Some(Commands::Ping {
                host,
                count,
                timeout,
                interval,
            }) => {
                ping::run(
                    &host,
                    count,
                    Duration::from_secs(timeout),
                    Duration::from_secs(interval),
                    mode,
                )
                .await
            }
            Some(Commands::Dns {
                domain,
                r#type,
                server,
            }) => dns::run(&domain, r#type, server, mode).await,
            Some(Commands::DnsCache {
                domain,
                flush,
                limit,
            }) => dns_cache::run(domain, flush, limit, mode).await,
            Some(Commands::DnsPath { domain, server }) => dns_path::run(domain, server, mode).await,
            Some(Commands::DnsCompare { domain, servers }) => {
                dns_compare::run(&domain, servers, mode).await
            }
            Some(Commands::DnsLeak {
                proxy,
                no_proxy,
                no_external,
                timeout,
                count,
            }) => dns_leak::run(proxy, no_proxy, no_external, timeout, count, mode).await,
            Some(Commands::Trace { host, max_hops }) => {
                traceroute::run(&host, max_hops, mode).await
            }
            Some(Commands::Scan {
                host,
                ports,
                concurrency,
            }) => {
                let port_list = ports.as_ref().map(|s| util::parse_ports(s));
                let port_ref = port_list
                    .as_ref()
                    .filter(|v| !v.is_empty())
                    .map(|v| v.as_slice());
                portscan::run(&host, port_ref, concurrency, mode).await
            }
            Some(Commands::Check {
                target,
                count,
                timeout,
                timing,
                proxy,
                no_proxy,
                concurrency,
                assertions,
            }) => {
                let Some(assertions) = parse_assertions(&assertions, mode) else {
                    return;
                };
                connectivity::run(
                    &target,
                    count,
                    Duration::from_secs(timeout),
                    timing,
                    proxy,
                    no_proxy,
                    concurrency,
                    &assertions,
                    mode,
                )
                .await
            }
            Some(Commands::Http {
                url,
                method,
                headers,
                body,
                timeout,
                proxy,
                no_proxy,
                show_headers,
                body_limit,
                assertions,
            }) => {
                let Some(assertions) = parse_assertions(&assertions, mode) else {
                    return;
                };
                http_client::run(
                    &url,
                    &method,
                    headers,
                    body,
                    Duration::from_secs(timeout),
                    proxy,
                    no_proxy,
                    show_headers,
                    body_limit,
                    &assertions,
                    mode,
                )
                .await
            }
            Some(Commands::Install { name, path, force }) => {
                plugin::install(&name, path.as_deref(), force, mode)
            }
            Some(Commands::Plugin) => {
                let cli = PluginCli::parse_from([OsString::from("netutils plugin")]);
                run_plugin_command(cli.command, mode);
            }
            Some(Commands::External(args)) => plugin::run_external(args, mode).await,
            Some(Commands::Connections {
                state,
                port,
                process,
                proto,
            }) => {
                let filter = connections::ConnFilter {
                    state,
                    port,
                    process,
                    proto,
                };
                connections::run(filter, mode)
            }
            Some(Commands::Diag) => diag::run(mode).await,
            Some(Commands::Diagnose { host }) => diagnose::run(&host, mode).await,
            Some(Commands::Path {
                url,
                max_hops,
                timeout,
                proxy,
                no_proxy,
            }) => {
                path::run(
                    &url,
                    max_hops,
                    Duration::from_secs(timeout),
                    proxy,
                    no_proxy,
                    mode,
                )
                .await
            }
            Some(Commands::ProxyTest {
                target,
                proxy,
                no_system_proxy,
                timeout,
                count,
                concurrency,
            }) => {
                proxy_test::run(
                    &target,
                    proxy,
                    no_system_proxy,
                    Duration::from_secs(timeout),
                    count,
                    concurrency,
                    mode,
                )
                .await
            }
            Some(Commands::Tls {
                target,
                port,
                sni,
                timeout,
                alpn,
            }) => {
                tls_probe::run(
                    &target,
                    port,
                    sni,
                    Duration::from_secs(timeout),
                    &alpn,
                    mode,
                )
                .await
            }
            Some(Commands::Mtu {
                target,
                min_mtu,
                max_mtu,
                timeout,
            }) => {
                mtu::run(
                    &target,
                    min_mtu,
                    max_mtu,
                    Duration::from_secs(timeout),
                    mode,
                )
                .await
            }
            Some(Commands::Completions { shell }) => {
                let mut command = Cli::command();
                clap_complete::generate(shell, &mut command, "netutils", &mut io::stdout());
            }
            Some(Commands::Man) => {
                if let Err(err) = clap_mangen::Man::new(Cli::command()).render(&mut io::stdout()) {
                    output::mark_failure();
                    eprintln!("failed to render man page: {err}");
                }
            }
        }
    };

    if let Some(seconds) = total_timeout {
        if tokio::time::timeout(Duration::from_secs(seconds), command)
            .await
            .is_err()
        {
            output::print_timeout_error(mode, seconds);
        }
    } else {
        command.await;
    }

    output::exit_if_failed();
    Ok(())
}

fn output_mode(json: bool) -> OutputMode {
    if json {
        OutputMode::Json
    } else {
        OutputMode::Table
    }
}

/// 解析 `--assert` 表达式。语法错误属于 CLI 用法错误，直接以退出码 2 结束。
fn parse_assertions(raw: &[String], mode: OutputMode) -> Option<Vec<assertion::Assertion>> {
    match assertion::parse_all(raw) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            if mode == OutputMode::Json {
                output::print_json_error(&err);
            } else {
                eprintln!("{err}");
            }
            output::mark_exit_code(2);
            None
        }
    }
}

/// 全局开关表：值为该开关连带消耗的参数个数（含开关自身）。
/// `plugin` 子命令走独立解析器，因此必须在这里手工跳过全局开关。
const GLOBAL_FLAGS: [(&str, usize); 4] = [
    ("--json", 1),
    ("--lang", 2),
    ("--color", 2),
    ("--total-timeout", 2),
];

fn global_flag_width(arg: &str) -> Option<usize> {
    for (flag, width) in GLOBAL_FLAGS {
        if arg == flag {
            return Some(width);
        }
        // `--lang=zh` 形式只占一个参数位。
        if width == 2 && arg.starts_with(flag) && arg.as_bytes().get(flag.len()) == Some(&b'=') {
            return Some(1);
        }
    }
    None
}

fn plugin_command_position(args: &[OsString]) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        let arg = args[i].to_string_lossy();
        match global_flag_width(arg.as_ref()) {
            Some(width) => i += width,
            None => return (arg == "plugin").then_some(i),
        }
    }
    None
}

fn plugin_help_parse_args(args: &[OsString]) -> Option<Vec<OsString>> {
    let mut parsed = vec![OsString::from("netutils plugin")];
    let mut i = 1;
    while i < args.len() {
        let arg = args[i].to_string_lossy();
        let Some(width) = global_flag_width(arg.as_ref()) else {
            break;
        };
        if i + width > args.len() {
            return None;
        }
        parsed.extend(args[i..i + width].iter().cloned());
        i += width;
    }

    if args.get(i).map(|arg| arg.to_string_lossy())? != "help"
        || args.get(i + 1).map(|arg| arg.to_string_lossy())? != "plugin"
    {
        return None;
    }

    parsed.extend(args.iter().skip(i + 2).cloned());
    parsed.push(OsString::from("--help"));
    Some(parsed)
}

fn plugin_parse_args(args: &[OsString], plugin_pos: usize) -> Vec<OsString> {
    let mut parsed = Vec::with_capacity(args.len());
    parsed.push(OsString::from("netutils plugin"));
    parsed.extend(
        args.iter()
            .enumerate()
            .skip(1)
            .filter(|(idx, _)| *idx != plugin_pos)
            .map(|(_, arg)| arg.clone()),
    );
    parsed
}

fn run_plugin_command(command: PluginCommands, mode: OutputMode) {
    match command {
        PluginCommands::New {
            name,
            dir,
            template,
            binary,
            crate_name,
            force,
        } => plugin::new_project(
            &name,
            dir.as_deref(),
            &template,
            binary.as_deref(),
            crate_name.as_deref(),
            force,
            mode,
        ),
        PluginCommands::List => plugin::list(mode),
        PluginCommands::Update { name } => plugin::update(&name, mode),
        PluginCommands::UpdateAll => plugin::update("all", mode),
        PluginCommands::Validate { path } => plugin::validate(&path, mode),
        PluginCommands::Remove { name } => plugin::remove(&name, mode),
        PluginCommands::Dir => plugin::print_dir(mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn finds_plugin_after_global_options() {
        let args = args(&["netutils", "--json", "--lang", "zh", "plugin", "list"]);

        assert_eq!(plugin_command_position(&args), Some(4));
    }

    #[test]
    fn finds_plugin_after_total_timeout() {
        let args = args(&["netutils", "--total-timeout", "5", "plugin", "list"]);

        assert_eq!(plugin_command_position(&args), Some(3));
    }

    #[test]
    fn finds_plugin_after_color_flag() {
        let args = args(&["netutils", "--color", "never", "plugin", "list"]);

        assert_eq!(plugin_command_position(&args), Some(3));
    }

    #[test]
    fn finds_plugin_after_inline_color_flag() {
        let args = args(&["netutils", "--color=never", "plugin", "list"]);

        assert_eq!(plugin_command_position(&args), Some(2));
    }

    #[test]
    fn plugin_position_is_none_when_flag_value_missing() {
        let args = args(&["netutils", "--color"]);

        assert_eq!(plugin_command_position(&args), None);
    }

    #[test]
    fn global_flag_width_rejects_unrelated_prefix() {
        // `--colorful` 不是全局开关，不能被当成 `--color` 吞掉。
        assert_eq!(global_flag_width("--colorful"), None);
        assert_eq!(global_flag_width("--color"), Some(2));
        assert_eq!(global_flag_width("--color=auto"), Some(1));
        assert_eq!(global_flag_width("--json"), Some(1));
    }

    #[test]
    fn plugin_help_parse_args_keeps_color_flag() {
        let raw = args(&["netutils", "--color", "never", "help", "plugin", "list"]);

        assert_eq!(
            plugin_help_parse_args(&raw),
            Some(args(&[
                "netutils plugin",
                "--color",
                "never",
                "list",
                "--help"
            ]))
        );
    }

    #[test]
    fn plugin_parse_args_remove_plugin_and_keep_globals() {
        let raw = args(&["netutils", "--json", "plugin", "list", "--lang=en"]);

        assert_eq!(
            plugin_parse_args(&raw, 2),
            args(&["netutils plugin", "--json", "list", "--lang=en"])
        );
    }

    #[test]
    fn plugin_help_parse_args_targets_plugin_parser() {
        let raw = args(&["netutils", "--lang", "zh", "help", "plugin", "update-all"]);

        assert_eq!(
            plugin_help_parse_args(&raw),
            Some(args(&[
                "netutils plugin",
                "--lang",
                "zh",
                "update-all",
                "--help"
            ]))
        );
    }
}
