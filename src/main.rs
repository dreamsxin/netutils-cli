mod cli;
mod connections;
mod connectivity;
mod diag;
mod diagnose;
mod dns;
mod dns_cache;
mod dns_compare;
mod dns_path;
mod http_client;
mod i18n;
mod icmp;
mod info;
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

use std::{env, ffi::OsString, time::Duration};

use clap::Parser;
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
        run_plugin_command(cli.command, mode);
        output::exit_if_failed();
        return Ok(());
    }

    let cli = Cli::parse_from(raw_args);

    // 初始化 i18n
    i18n::init(cli.lang);

    // 确定输出模式
    let mode = output_mode(cli.json);
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
            }) => {
                connectivity::run(
                    &target,
                    count,
                    Duration::from_secs(timeout),
                    timing,
                    proxy,
                    no_proxy,
                    concurrency,
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
            }) => {
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

fn plugin_command_position(args: &[OsString]) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        let arg = args[i].to_string_lossy();
        match arg.as_ref() {
            "--json" => i += 1,
            "--lang" => i += 2,
            "--total-timeout" => i += 2,
            _ if arg.starts_with("--lang=") => i += 1,
            _ if arg.starts_with("--total-timeout=") => i += 1,
            _ => return (arg == "plugin").then_some(i),
        }
    }
    None
}

fn plugin_help_parse_args(args: &[OsString]) -> Option<Vec<OsString>> {
    let mut parsed = vec![OsString::from("netutils plugin")];
    let mut i = 1;
    while i < args.len() {
        let arg = args[i].to_string_lossy();
        match arg.as_ref() {
            "--json" => {
                parsed.push(args[i].clone());
                i += 1;
            }
            "--lang" => {
                if i + 1 >= args.len() {
                    return None;
                }
                parsed.push(args[i].clone());
                parsed.push(args[i + 1].clone());
                i += 2;
            }
            "--total-timeout" => {
                if i + 1 >= args.len() {
                    return None;
                }
                parsed.push(args[i].clone());
                parsed.push(args[i + 1].clone());
                i += 2;
            }
            _ if arg.starts_with("--lang=") => {
                parsed.push(args[i].clone());
                i += 1;
            }
            _ if arg.starts_with("--total-timeout=") => {
                parsed.push(args[i].clone());
                i += 1;
            }
            _ => break,
        }
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
