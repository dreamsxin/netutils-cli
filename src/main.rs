mod cli;
mod connections;
mod connectivity;
mod diag;
mod diagnose;
mod dns;
mod dns_cache;
mod dns_compare;
mod dns_path;
mod i18n;
mod icmp;
mod info;
mod output;
mod path;
mod ping;
mod portscan;
mod proxy_test;
mod route_get;
mod route_probe;
mod table;
mod tls_probe;
mod traceroute;
mod util;

use std::time::Duration;

use clap::Parser;
use cli::{Cli, Commands};
use output::OutputMode;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // 初始化 i18n
    i18n::init(cli.lang);

    // 确定输出模式
    let mode = if cli.json {
        OutputMode::Json
    } else {
        OutputMode::Table
    };

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
        Some(Commands::Trace { host, max_hops }) => traceroute::run(&host, max_hops, mode).await,
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
        }) => {
            proxy_test::run(
                &target,
                proxy,
                no_system_proxy,
                Duration::from_secs(timeout),
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

    Ok(())
}
