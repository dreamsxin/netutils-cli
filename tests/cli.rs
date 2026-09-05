//! CLI 层集成测试：直接运行编译产物，覆盖参数契约与退出码约定。
//!
//! 这些用例全部离线，不依赖网络，可以在 CI 的三个平台上稳定运行。

use std::process::{Command, Output};

fn netutils(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_netutils"))
        .args(args)
        // 固定语言与颜色，避免宿主环境影响断言。
        .env("NETUTILS_LANG", "en")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run netutils binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn help_lists_core_commands() {
    let output = netutils(&["--help"]);

    assert!(output.status.success());
    let text = stdout(&output);
    for command in ["iface", "dns", "check", "http", "mtu", "diagnose", "plugin"] {
        assert!(text.contains(command), "help should mention `{command}`");
    }
}

#[test]
fn version_flag_prints_version() {
    let output = netutils(&["--version"]);

    assert!(output.status.success());
    assert!(stdout(&output).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn unknown_flag_exits_with_usage_code() {
    let output = netutils(&["--definitely-not-a-flag"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn completions_generate_shell_script() {
    let output = netutils(&["completions", "bash"]);

    assert!(output.status.success());
    let script = stdout(&output);
    assert!(script.contains("netutils"));
    assert!(script.contains("complete"));
}

#[test]
fn completions_support_every_documented_shell() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = netutils(&["completions", shell]);

        assert!(
            output.status.success(),
            "completions for {shell} should succeed"
        );
        assert!(
            !stdout(&output).is_empty(),
            "completions for {shell} should not be empty"
        );
    }
}

#[test]
fn completions_reject_unknown_shell() {
    let output = netutils(&["completions", "nushell-but-not-really"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn man_page_renders_roff() {
    let output = netutils(&["man"]);

    assert!(output.status.success());
    let page = stdout(&output);
    assert!(page.contains(".TH"), "man output should be roff");
    assert!(page.contains("netutils"));
}

#[test]
fn color_flag_accepts_documented_values() {
    for value in ["auto", "always", "never"] {
        let output = netutils(&["--color", value, "--help"]);

        assert!(
            output.status.success(),
            "--color {value} should be accepted"
        );
    }
}

#[test]
fn color_flag_rejects_unknown_value() {
    let output = netutils(&["--color", "rainbow", "--help"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn malformed_assertion_is_a_usage_error() {
    let output = netutils(&[
        "http",
        "https://example.com",
        "--assert",
        "no-operator-here",
    ]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("invalid assertion"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn malformed_assertion_reports_json_error_in_json_mode() {
    let output = netutils(&[
        "--json",
        "check",
        "https://example.com",
        "--assert",
        "latency<fast",
    ]);

    assert_eq!(output.status.code(), Some(2));
    let text = stdout(&output);
    assert!(text.contains("\"error\""), "stdout: {text}");
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("json mode must emit valid JSON");
    assert!(parsed["error"].is_string());
}

#[test]
fn malformed_target_fails_without_network() {
    // 端口缺失属于格式错误，可在无网络环境下稳定复现。
    let output = netutils(&["check", "not-a-host-port"]);

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn plugin_list_runs_through_the_dedicated_parser() {
    let output = netutils(&["--color", "never", "plugin", "list"]);

    // 未安装任何插件时仍应正常列出已知插件而非报参数错误。
    assert_ne!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("mcp"));
}

#[test]
fn help_plugin_reaches_plugin_parser() {
    let output = netutils(&["help", "plugin"]);

    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("validate"), "stdout: {text}");
}

#[test]
fn mtu_help_documents_search_bounds() {
    let output = netutils(&["mtu", "--help"]);

    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("--min-mtu"));
    assert!(text.contains("--max-mtu"));
}
