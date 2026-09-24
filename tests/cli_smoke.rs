//! 端到端冒烟测试。
//!
//! 为什么需要它：单元测试不启动二进制，因此对「命令行解析阶段就崩」这一整类问题
//! 完全失明。clap derive 递归构建命令树，在 Windows 1 MB 主线程栈上溢出过三次
//! （0.3.17 的 `plugin`、0.5.0 的 `completions`/`man`、0.7.0 新增两个 `--parallel`），
//! 最后一次连 `netutils --version` 都起不来，而当时 233 个单测全绿、三道门全过。
//!
//! 这里只断言「能起来、能解析」，不测网络行为——目标是让那一类静默失明的问题
//! 变成一条红掉的测试。

use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_netutils");

fn run(args: &[&str]) -> Output {
    Command::new(EXE)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {EXE} {args:?}: {e}"))
}

/// 栈溢出时这条最先红：它连命令树都没走完就死了
#[test]
fn reports_its_version() {
    let out = run(&["--version"]);
    assert!(
        out.status.success(),
        "--version exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("netutils"),
        "unexpected --version output: {stdout}"
    );
}

#[test]
fn renders_top_level_help() {
    let out = run(&["--help"]);
    assert!(
        out.status.success(),
        "--help exited {:?}",
        out.status.code()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for expected in ["check", "scan", "ping", "--json"] {
        assert!(stdout.contains(expected), "--help lacks {expected}");
    }
}

/// 逐个子命令走一遍解析：命令树的深度是随子命令和参数增长的，
/// 只测顶层会漏掉「某个子命令自己的参数太多」这种情况。
#[test]
fn renders_help_for_every_subcommand() {
    let subcommands = [
        "all",
        "iface",
        "egress",
        "route",
        "route-get",
        "proxy",
        "ping",
        "dns",
        "dns-cache",
        "dns-path",
        "dns-compare",
        "dns-leak",
        "trace",
        "scan",
        "check",
        "http",
        "install",
        "plugin",
        "connections",
        "diag",
        "diagnose",
        "path",
        "proxy-test",
        "tls",
        "mtu",
        "completions",
        "man",
    ];

    for sub in subcommands {
        let out = run(&[sub, "--help"]);
        assert!(
            out.status.success(),
            "`{sub} --help` exited {:?}\nstderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// 目标来源二选一的用法错误必须在发出任何探测之前就以退出码 2 结束
#[test]
fn rejects_a_missing_target_with_a_usage_code() {
    for args in [vec!["check"], vec!["scan"]] {
        let out = run(&args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "`{}` should be a usage error",
            args.join(" ")
        );
    }
}

/// 位置端口参数在批量模式下会被 clap 当成主机吃掉，必须报互斥而不是静默误解
#[test]
fn rejects_positional_ports_in_batch_mode() {
    let out = run(&["scan", "--targets-from", "-", "80,443"]);
    assert!(
        !out.status.success(),
        "positional ports alongside --targets-from should fail"
    );
}
