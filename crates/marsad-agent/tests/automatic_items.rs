//! The Marsad MCP servers a new session gets (`marsad-agent mcp` for Wasl, `ask-mcp` for the
//! question tool) run from the helper of the daemon that launches it: the executable path that
//! daemon reports for itself (`daemon.info`), which is its build's own directory, never a fixed
//! or flat layout.

use marsad_agent::hub::{AskUserMcp, AutomaticItems, LaunchContext, WaslMcp};
use marsad_proto::agentconfig::ItemValue;
use marsad_proto::daemon::{DaemonInfo, VersionInfo};
use marsad_proto::{DriverId, HostId, SessionId};
use wasit_core::{DriverCapabilities, McpTransport};

/// The helper a WSL or SSH host's daemon runs from (its build's directory).
const EXE: &str = "/home/u/.marsad/bin/0.2.0+abc/Linux-x86_64/marsad-agent";

fn launch(questions: bool) -> LaunchContext {
    // The capabilities a CLI reports (the fields without a serde default are the ones given);
    // only the question capability decides the item.
    let capabilities: DriverCapabilities = serde_json::from_value(serde_json::json!({
        "interrupt": false,
        "set_model": false,
        "set_mode": false,
        "slash_commands": false,
        "permission_gates": false,
        "questions": questions,
        "plan_gate": false,
        "usage": false,
        "cost": false,
        "resume": false,
        "continue_latest": false,
        "streaming_output": false,
        "prompt_while_busy": "queued_by_cli",
    }))
    .expect("the CLI's capabilities");
    LaunchContext {
        host: HostId::new("ubuntu"),
        driver: DriverId::new("kilo"),
        project: None,
        session: SessionId::random(),
        workstream: None,
        daemon: DaemonInfo {
            version: VersionInfo {
                build: "0.2.0+abc".into(),
                target: "x86_64-unknown-linux-musl".into(),
            },
            host_id: "ubuntu".into(),
            hub: false,
            hub_id: None,
            data_dir: "/home/u/.marsad/machines/k".into(),
            pid: 1,
            exe: EXE.into(),
            socket: "/tmp/marsad/s.sock".into(),
        },
        capabilities,
    }
}

/// The program and arguments of the single MCP server item an automatic provider gives.
fn stdio_server(items: Vec<marsad_proto::agentconfig::AutomaticItem>) -> (String, Vec<String>) {
    assert_eq!(items.len(), 1, "one MCP server item");
    match items.into_iter().next().unwrap().value {
        ItemValue::McpServer {
            server: McpTransport::Stdio { command, args, .. },
        } => (command, args),
        other => panic!("not a stdio MCP server: {other:?}"),
    }
}

#[test]
fn a_new_session_runs_wasl_mcp_from_its_hosts_own_helper() {
    let (command, args) = stdio_server(WaslMcp.items(&launch(true)));
    assert_eq!(command, EXE);
    assert_eq!(args.first().map(String::as_str), Some("mcp"));
}

#[test]
fn a_new_session_runs_ask_mcp_from_its_hosts_own_helper() {
    let (command, args) = stdio_server(AskUserMcp.items(&launch(false)));
    assert_eq!(command, EXE);
    assert_eq!(args.first().map(String::as_str), Some("ask-mcp"));
}
