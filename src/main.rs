mod cli;
mod connectivity;
mod diag;
mod dns;
mod i18n;
mod info;
mod output;
mod ping;
mod portscan;
mod table;
mod traceroute;
mod util;

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
        Some(Commands::Proxy) => info::print_proxy(mode),
        Some(Commands::Ping { host, count }) => ping::run(&host, count, mode).await,
        Some(Commands::Dns { domain, r#type }) => dns::run(&domain, r#type, mode).await,
        Some(Commands::Trace { host }) => traceroute::run(&host, mode).await,
        Some(Commands::Scan { host, ports }) => {
            let port_list = ports.as_ref().map(|s| util::parse_ports(s));
            let port_ref = port_list
                .as_ref()
                .filter(|v| !v.is_empty())
                .map(|v| v.as_slice());
            portscan::run(&host, port_ref, mode).await;
        }
        Some(Commands::Check { target, count }) => connectivity::run(&target, count, mode).await,
        Some(Commands::Diag) => diag::run(mode).await,
    }

    Ok(())
}
