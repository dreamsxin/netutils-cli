//! 按本机网络端口查询占用进程。

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};

use colored::Colorize;
use serde::Serialize;

use crate::cli::ProcessAction;
use crate::connections::{self, ConnectionInfo, ConnectionSnapshot};
use crate::i18n::t;
use crate::output::{self, print_json, OutputMode};
use crate::table::print_table;

#[derive(Debug, Serialize)]
struct ProcessEndpoint {
    protocol: String,
    local_addr: String,
    state: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProcessEntry {
    pid: u32,
    process_name: String,
    endpoints: Vec<ProcessEndpoint>,
}

#[derive(Debug, Serialize)]
struct ProcessLookupOutput {
    port: u16,
    status: &'static str,
    processes: Vec<ProcessEntry>,
    unidentified_endpoints: Vec<ProcessEndpoint>,
    total: usize,
    error_code: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProcessKillOutput {
    pid: u32,
    success: bool,
    status: &'static str,
    error_code: Option<String>,
}

struct KillRequest {
    pid: u32,
    yes: bool,
    mode: OutputMode,
    terminal: bool,
    self_pid: u32,
}

pub fn run(port: Option<u16>, action: Option<ProcessAction>, mode: OutputMode) {
    match (port, action) {
        (Some(_), Some(ProcessAction::Kill { .. })) => {
            report_usage_error("process.action_conflict", mode);
        }
        (Some(port), None) => run_lookup(port, mode),
        (None, Some(ProcessAction::Kill { pid, yes })) => run_kill(pid, yes, mode),
        (None, None) => report_usage_error("process.action_required", mode),
    }
}

fn run_lookup(port: u16, mode: OutputMode) {
    let result = match connections::collect_connections(port) {
        Ok(snapshot) => lookup(port, snapshot),
        Err(error) => unavailable(port, error.code),
    };

    if result.status != "ok" {
        output::mark_failure();
    }
    if mode == OutputMode::Json {
        print_json(&result);
    } else {
        render(&result);
    }
}

fn run_kill(pid: u32, yes: bool, mode: OutputMode) {
    let terminal = std::io::stdin().is_terminal();
    let result = execute_kill(
        KillRequest {
            pid,
            yes,
            mode,
            terminal,
            self_pid: std::process::id(),
        },
        lookup_process_name,
        confirm_termination,
        terminate_process,
    );

    match result.status {
        "terminated" => {}
        "confirmation_required" => output::mark_exit_code(2),
        _ => output::mark_failure(),
    }
    if mode == OutputMode::Json {
        print_json(&result);
    } else {
        render_kill(&result);
    }
}

fn report_usage_error(key: &str, mode: OutputMode) {
    let message = t(key);
    if mode == OutputMode::Json {
        output::print_json_error(&message);
    } else {
        eprintln!("{message}");
    }
    output::mark_exit_code(2);
}

fn unavailable(port: u16, code: &'static str) -> ProcessLookupOutput {
    ProcessLookupOutput {
        port,
        status: "unavailable",
        processes: Vec::new(),
        unidentified_endpoints: Vec::new(),
        total: 0,
        error_code: Some(code.to_string()),
    }
}

fn lookup(port: u16, snapshot: ConnectionSnapshot) -> ProcessLookupOutput {
    let matching: Vec<&ConnectionInfo> = snapshot
        .connections
        .iter()
        .filter(|connection| local_port(&connection.local_addr) == Some(port))
        .collect();
    let mut grouped = BTreeMap::<u32, ProcessEntry>::new();
    let mut unidentified_endpoints = Vec::new();
    let mut metadata_incomplete = false;

    for connection in &matching {
        if connection.pid == 0 {
            metadata_incomplete = true;
            unidentified_endpoints.push(endpoint(connection));
            continue;
        }
        if connection.process_name == "N/A" || connection.process_name.is_empty() {
            metadata_incomplete = true;
        }
        let entry = grouped
            .entry(connection.pid)
            .or_insert_with(|| ProcessEntry {
                pid: connection.pid,
                process_name: connection.process_name.clone(),
                endpoints: Vec::new(),
            });
        if entry.process_name == "N/A" && connection.process_name != "N/A" {
            entry.process_name.clone_from(&connection.process_name);
        }
        entry.endpoints.push(endpoint(connection));
    }

    let processes: Vec<ProcessEntry> = grouped.into_values().collect();
    let total = matching.len();
    let error_code = if let Some(issue) = snapshot.issue {
        Some(issue.code.to_string())
    } else if metadata_incomplete {
        Some("process_metadata_incomplete".to_string())
    } else {
        None
    };
    let status = if error_code.is_some() {
        "partial"
    } else {
        "ok"
    };

    ProcessLookupOutput {
        port,
        status,
        processes,
        unidentified_endpoints,
        total,
        error_code,
    }
}

fn endpoint(connection: &ConnectionInfo) -> ProcessEndpoint {
    ProcessEndpoint {
        protocol: connection.protocol.clone(),
        local_addr: connection.local_addr.clone(),
        state: (connection.protocol != "UDP").then(|| connection.state.clone()),
    }
}

fn local_port(address: &str) -> Option<u16> {
    address.rsplit_once(':')?.1.parse().ok()
}

fn render(result: &ProcessLookupOutput) {
    println!();
    println!("{}", t("process.title").bold());

    if result.total == 0 && result.status == "ok" {
        println!("  {}", t("process.no_result").yellow());
    } else {
        let headers = [
            t("connections.proto"),
            t("connections.local"),
            t("connections.state"),
            t("connections.pid"),
            t("connections.process"),
        ];
        let mut rows = Vec::new();
        for process in &result.processes {
            rows.extend(process.endpoints.iter().map(|endpoint| {
                vec![
                    endpoint.protocol.clone(),
                    endpoint.local_addr.clone(),
                    endpoint.state.clone().unwrap_or_else(|| "*".to_string()),
                    process.pid.to_string(),
                    process.process_name.clone(),
                ]
            }));
        }
        let unknown = t("process.unknown");
        rows.extend(result.unidentified_endpoints.iter().map(|endpoint| {
            vec![
                endpoint.protocol.clone(),
                endpoint.local_addr.clone(),
                endpoint.state.clone().unwrap_or_else(|| "*".to_string()),
                "?".to_string(),
                unknown.clone(),
            ]
        }));
        print_table(
            &headers.iter().map(String::as_str).collect::<Vec<_>>(),
            &rows,
        );
    }

    if let Some(code) = &result.error_code {
        println!("  {}", t(issue_message_key(code)).yellow());
    }
    println!(
        "  {}",
        t("process.summary")
            .replace("{0}", &result.total.to_string())
            .replace("{1}", &result.port.to_string())
    );
}

fn issue_message_key(code: &str) -> &'static str {
    match code {
        "permission_denied" => "process.permission_denied",
        "process_metadata_incomplete" => "process.metadata_incomplete",
        "collector_incomplete" => "process.collector_incomplete",
        _ => "process.collector_unavailable",
    }
}

fn execute_kill<N, C, T>(
    request: KillRequest,
    mut name_lookup: N,
    mut confirm: C,
    mut terminate: T,
) -> ProcessKillOutput
where
    N: FnMut(u32) -> Result<String, &'static str>,
    C: FnMut(u32, &str) -> bool,
    T: FnMut(u32) -> Result<(), &'static str>,
{
    let KillRequest {
        pid,
        yes,
        mode,
        terminal,
        self_pid,
    } = request;
    if pid == 0 {
        return kill_failure(pid, "invalid_pid");
    }
    if pid == self_pid {
        return kill_failure(pid, "self_pid");
    }
    if !yes && !(mode == OutputMode::Table && terminal) {
        return ProcessKillOutput {
            pid,
            success: false,
            status: "confirmation_required",
            error_code: Some("confirmation_required".to_string()),
        };
    }

    let process_name = match name_lookup(pid) {
        Ok(name) => name,
        Err(code) => return kill_failure(pid, code),
    };
    if !yes && !confirm(pid, &process_name) {
        return ProcessKillOutput {
            pid,
            success: false,
            status: "cancelled",
            error_code: None,
        };
    }

    match terminate(pid) {
        Ok(()) => ProcessKillOutput {
            pid,
            success: true,
            status: "terminated",
            error_code: None,
        },
        Err(code) => kill_failure(pid, code),
    }
}

fn kill_failure(pid: u32, code: &'static str) -> ProcessKillOutput {
    ProcessKillOutput {
        pid,
        success: false,
        status: "failed",
        error_code: Some(code.to_string()),
    }
}

fn confirm_termination(pid: u32, process_name: &str) -> bool {
    let prompt = t("process.confirm")
        .replace("{0}", process_name)
        .replace("{1}", &pid.to_string());
    println!("{prompt}");
    print!("{}", t("process.confirm_input"));
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .is_ok_and(|_| matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn render_kill(result: &ProcessKillOutput) {
    let message = match result.status {
        "terminated" => t("process.kill_success").replace("{0}", &result.pid.to_string()),
        "cancelled" => t("process.kill_cancelled").replace("{0}", &result.pid.to_string()),
        "confirmation_required" => t("process.confirmation_required"),
        _ => t(kill_message_key(
            result.error_code.as_deref().unwrap_or("termination_failed"),
        ))
        .replace("{0}", &result.pid.to_string()),
    };
    if result.success {
        println!("{}", message.green());
    } else {
        println!("{}", message.yellow());
    }
}

fn kill_message_key(code: &str) -> &'static str {
    match code {
        "invalid_pid" => "process.pid_invalid",
        "self_pid" => "process.pid_self",
        "process_not_found" => "process.pid_not_found",
        "permission_denied" => "process.kill_permission_denied",
        "process_lookup_failed" => "process.pid_lookup_failed",
        "terminator_unavailable" => "process.kill_unavailable",
        "termination_failed" => "process.kill_failed",
        _ => "process.kill_failed",
    }
}

#[cfg(windows)]
fn lookup_process_name(pid: u32) -> Result<String, &'static str> {
    let script = format!(
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Get-Process -Id {pid} -ErrorAction Stop | ForEach-Object {{ $_.ProcessName }}"
    );
    let output = crate::util::powershell_output(&script, std::time::Duration::from_secs(5))
        .ok_or("process_lookup_failed")?;
    if !output.status.success() {
        return Err(classify_process_error(
            &output.stderr,
            "process_lookup_failed",
        ));
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        Err("process_not_found")
    } else {
        Ok(name)
    }
}

#[cfg(unix)]
fn lookup_process_name(pid: u32) -> Result<String, &'static str> {
    let pid_arg = pid.to_string();
    let output = std::process::Command::new("ps")
        .args(["-p", pid_arg.as_str(), "-o", "comm="])
        .output()
        .map_err(|_| "process_lookup_failed")?;
    if !output.status.success() {
        return Err(classify_process_error(
            &output.stderr,
            "process_lookup_failed",
        ));
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        Err("process_not_found")
    } else {
        Ok(name)
    }
}

#[cfg(not(any(windows, unix)))]
fn lookup_process_name(_pid: u32) -> Result<String, &'static str> {
    Err("process_lookup_failed")
}

#[cfg(windows)]
fn terminate_process(pid: u32) -> Result<(), &'static str> {
    let script = format!(
        "$ErrorActionPreference = 'Stop'; Stop-Process -Id {pid} -ErrorAction Stop; Write-Output 'DONE'"
    );
    let output = crate::util::powershell_output(&script, std::time::Duration::from_secs(5))
        .ok_or("terminator_unavailable")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(classify_process_error(&output.stderr, "termination_failed"))
    }
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> Result<(), &'static str> {
    let pid_arg = pid.to_string();
    let output = std::process::Command::new("kill")
        .args(["-TERM", pid_arg.as_str()])
        .output()
        .map_err(|_| "terminator_unavailable")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(classify_process_error(&output.stderr, "termination_failed"))
    }
}

#[cfg(not(any(windows, unix)))]
fn terminate_process(_pid: u32) -> Result<(), &'static str> {
    Err("terminator_unavailable")
}

fn classify_process_error(stderr: &[u8], fallback: &'static str) -> &'static str {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if text.contains("permission denied")
        || text.contains("operation not permitted")
        || text.contains("access is denied")
        || text.contains("accessdenied")
        || text.contains("unauthorizedaccess")
    {
        "permission_denied"
    } else if text.contains("no such process")
        || text.contains("cannot find a process")
        || text.contains("process not found")
    {
        "process_not_found"
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{ConnectionReadIssue, ConnectionSnapshot};

    fn connection(
        protocol: &str,
        local_addr: &str,
        state: &str,
        pid: u32,
        process_name: &str,
    ) -> ConnectionInfo {
        ConnectionInfo {
            protocol: protocol.to_string(),
            local_addr: local_addr.to_string(),
            remote_addr: "*:*".to_string(),
            state: state.to_string(),
            pid,
            process_name: process_name.to_string(),
            is_proxy: false,
        }
    }

    #[test]
    fn lookup_matches_local_port_exactly_and_handles_ipv6() {
        let snapshot = ConnectionSnapshot {
            connections: vec![
                connection("TCP", "0.0.0.0:80", "LISTEN", 10, "web"),
                connection("TCP", "127.0.0.1:8080", "LISTEN", 11, "other"),
                connection("UDP", "[::]:80", "*", 10, "web"),
                connection("TCP", "[::1]:80", "LISTEN", 12, "api"),
            ],
            issue: None,
        };

        let result = lookup(80, snapshot);

        assert_eq!(result.status, "ok");
        assert_eq!(result.total, 3);
        assert_eq!(result.processes.len(), 2);
        assert_eq!(result.processes[0].pid, 10);
        assert_eq!(result.processes[0].endpoints.len(), 2);
        assert_eq!(result.processes[0].endpoints[1].state, None);
        assert_eq!(result.processes[1].pid, 12);
        assert_eq!(result.processes[1].process_name, "api");
        let json = serde_json::to_value(&result).expect("lookup output should serialize");
        assert!(json["processes"][0]["endpoints"][1]["state"].is_null());
    }

    #[test]
    fn lookup_does_not_match_remote_peer_port() {
        let mut connection = connection("TCP", "127.0.0.1:54000", "ESTAB", 10, "client");
        connection.remote_addr = "192.0.2.1:80".to_string();
        let result = lookup(
            80,
            ConnectionSnapshot {
                connections: vec![connection],
                issue: None,
            },
        );

        assert_eq!(result.status, "ok");
        assert_eq!(result.total, 0);
        assert!(result.processes.is_empty());
    }

    #[test]
    fn lookup_reports_a_complete_empty_result_for_an_unoccupied_port() {
        let result = lookup(
            1234,
            ConnectionSnapshot {
                connections: vec![],
                issue: None,
            },
        );

        assert_eq!(result.status, "ok");
        assert_eq!(result.total, 0);
        assert_eq!(result.error_code, None);
    }

    #[test]
    fn lookup_keeps_unknown_pid_endpoints_as_partial_results() {
        let snapshot = ConnectionSnapshot {
            connections: vec![connection("TCP", "[::1]:9000", "LISTEN", 0, "N/A")],
            issue: None,
        };

        let result = lookup(9000, snapshot);

        assert_eq!(result.status, "partial");
        assert_eq!(
            result.error_code.as_deref(),
            Some("process_metadata_incomplete")
        );
        assert_eq!(result.unidentified_endpoints.len(), 1);
        assert_eq!(result.total, 1);
    }

    #[test]
    fn lookup_preserves_collector_issue_even_when_no_endpoint_matches() {
        let snapshot = ConnectionSnapshot {
            connections: vec![],
            issue: Some(ConnectionReadIssue {
                code: "permission_denied",
            }),
        };

        let result = lookup(1234, snapshot);

        assert_eq!(result.status, "partial");
        assert_eq!(result.error_code.as_deref(), Some("permission_denied"));
        assert_eq!(result.total, 0);
    }

    #[test]
    fn unavailable_collector_is_not_reported_as_an_empty_port() {
        let result = unavailable(1234, "collector_unavailable");

        assert_eq!(result.status, "unavailable");
        assert_eq!(result.error_code.as_deref(), Some("collector_unavailable"));
    }

    #[test]
    fn local_port_parser_does_not_accept_malformed_addresses() {
        assert_eq!(local_port("[::1]:65535"), Some(65535));
        assert_eq!(local_port("127.0.0.1:80"), Some(80));
        assert_eq!(local_port("127.0.0.1:8080"), Some(8080));
        assert_eq!(local_port("127.0.0.1:not-a-port"), None);
        assert_eq!(local_port("127.0.0.1"), None);
    }

    #[test]
    fn zero_and_self_pid_are_rejected_before_process_access() {
        for (pid, expected) in [(0, "invalid_pid"), (100, "self_pid")] {
            let result = execute_kill(
                KillRequest {
                    pid,
                    yes: true,
                    mode: OutputMode::Table,
                    terminal: false,
                    self_pid: 100,
                },
                |_| panic!("must not inspect a rejected PID"),
                |_, _| panic!("must not prompt for a rejected PID"),
                |_| panic!("must not terminate a rejected PID"),
            );
            assert_eq!(result.status, "failed");
            assert_eq!(result.error_code.as_deref(), Some(expected));
        }
    }

    #[test]
    fn noninteractive_kill_requires_explicit_confirmation_without_side_effects() {
        let result = execute_kill(
            KillRequest {
                pid: 200,
                yes: false,
                mode: OutputMode::Json,
                terminal: true,
                self_pid: 100,
            },
            |_| panic!("confirmation must be checked before process lookup"),
            |_, _| panic!("JSON mode must not prompt"),
            |_| panic!("confirmation is required before termination"),
        );

        assert_eq!(result.status, "confirmation_required");
        assert_eq!(result.error_code.as_deref(), Some("confirmation_required"));
    }

    #[test]
    fn cancelling_interactive_confirmation_does_not_terminate() {
        let terminated = std::cell::Cell::new(false);
        let result = execute_kill(
            KillRequest {
                pid: 200,
                yes: false,
                mode: OutputMode::Table,
                terminal: true,
                self_pid: 100,
            },
            |_| Ok("worker".to_string()),
            |pid, name| {
                assert_eq!(pid, 200);
                assert_eq!(name, "worker");
                false
            },
            |_| {
                terminated.set(true);
                Ok(())
            },
        );

        assert_eq!(result.status, "cancelled");
        assert!(!terminated.get());
    }

    #[test]
    fn explicit_confirmation_runs_the_injected_terminator() {
        let terminated_pid = std::cell::Cell::new(None);
        let result = execute_kill(
            KillRequest {
                pid: 200,
                yes: true,
                mode: OutputMode::Json,
                terminal: false,
                self_pid: 100,
            },
            |_| Ok("worker".to_string()),
            |_, _| panic!("--yes must skip the prompt"),
            |pid| {
                terminated_pid.set(Some(pid));
                Ok(())
            },
        );

        assert_eq!(result.status, "terminated");
        assert!(result.success);
        assert_eq!(terminated_pid.get(), Some(200));
    }

    #[test]
    fn process_and_termination_errors_remain_distinct() {
        assert_eq!(
            classify_process_error(b"No such process", "termination_failed"),
            "process_not_found"
        );
        assert_eq!(
            classify_process_error(b"Operation not permitted", "termination_failed"),
            "permission_denied"
        );
        assert_eq!(
            classify_process_error(b"unexpected failure", "termination_failed"),
            "termination_failed"
        );
    }

    #[test]
    fn process_lookup_failures_do_not_reach_confirmation_or_termination() {
        for code in [
            "process_not_found",
            "permission_denied",
            "process_lookup_failed",
        ] {
            let result = execute_kill(
                KillRequest {
                    pid: 200,
                    yes: true,
                    mode: OutputMode::Json,
                    terminal: false,
                    self_pid: 100,
                },
                |_| Err(code),
                |_, _| panic!("explicit confirmation must not ask interactively"),
                |_| panic!("a process lookup failure must not terminate"),
            );

            assert_eq!(result.status, "failed");
            assert_eq!(result.error_code.as_deref(), Some(code));
        }
    }

    #[test]
    fn termination_tool_failures_are_reported() {
        for code in [
            "permission_denied",
            "terminator_unavailable",
            "termination_failed",
        ] {
            let result = execute_kill(
                KillRequest {
                    pid: 200,
                    yes: true,
                    mode: OutputMode::Json,
                    terminal: false,
                    self_pid: 100,
                },
                |_| Ok("worker".to_string()),
                |_, _| panic!("--yes must skip the prompt"),
                |_| Err(code),
            );

            assert_eq!(result.status, "failed");
            assert_eq!(result.error_code.as_deref(), Some(code));
        }
    }
}
