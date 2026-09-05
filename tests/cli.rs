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

#[test]
fn dns_help_documents_doh() {
    let output = netutils(&["dns", "--help"]);

    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("--doh"), "stdout: {text}");
    assert!(text.contains("cloudflare"), "presets should be listed");
}

#[test]
fn doh_and_server_are_mutually_exclusive() {
    // 两者走不同传输层，同时给出会让结果无法归因，属于用法错误。
    let output = netutils(&[
        "dns",
        "example.com",
        "--server",
        "8.8.8.8",
        "--doh",
        "cloudflare",
    ]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn doh_proxy_flags_require_doh() {
    let output = netutils(&["dns", "example.com", "--proxy", "http://127.0.0.1:1"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn unknown_doh_preset_fails_before_any_request() {
    let output = netutils(&["--json", "dns", "example.com", "--doh", "cloudfalre"]);

    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("unknown DoH preset"), "stdout: {text}");
    assert!(text.contains("cloudflare"), "should list valid presets");
}

#[test]
fn plaintext_doh_endpoint_is_refused() {
    let output = netutils(&[
        "--json",
        "dns",
        "example.com",
        "--doh",
        "http://example.com/dns-query",
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains("refusing plaintext"),
        "stdout: {}",
        stdout(&output)
    );
}

#[test]
fn dns_help_documents_dot() {
    let output = netutils(&["dns", "--help"]);

    assert!(output.status.success());
    assert!(
        stdout(&output).contains("--dot"),
        "stdout: {}",
        stdout(&output)
    );
}

#[test]
fn dot_conflicts_with_the_other_transports() {
    for conflicting in [["--server", "8.8.8.8"], ["--doh", "cloudflare"]] {
        let output = netutils(&[
            "dns",
            "example.com",
            "--dot",
            "cloudflare",
            conflicting[0],
            conflicting[1],
        ]);

        assert_eq!(
            output.status.code(),
            Some(2),
            "--dot must conflict with {}",
            conflicting[0]
        );
    }
}

#[test]
fn dot_rejects_bare_ip_before_connecting() {
    // DoT 校验服务器证书，只给 IP 无法验证身份，这在离线环境下也能稳定复现。
    let output = netutils(&["--json", "dns", "example.com", "--dot", "1.1.1.1"]);

    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("requires a hostname"), "stdout: {text}");
}

#[test]
fn dot_rejects_url_form() {
    let output = netutils(&[
        "--json",
        "dns",
        "example.com",
        "--dot",
        "tls://dns.example.net",
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains("expected host or host:port"),
        "stdout: {}",
        stdout(&output)
    );
}

#[test]
fn dot_proxy_flags_are_not_accepted() {
    // 代理只对 DoH 有意义，--proxy 依赖 --doh，因此配 --dot 属于用法错误。
    let output = netutils(&[
        "dns",
        "example.com",
        "--dot",
        "cloudflare",
        "--proxy",
        "http://127.0.0.1:1",
    ]);

    assert_eq!(output.status.code(), Some(2));
}
