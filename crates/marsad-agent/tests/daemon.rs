//! The daemon through real processes: readiness and single instance, `--version`, the Wasl
//! `mcp` server, `stop`; and through `marsad-client`: launch → prompt → gate → answer, store access,
//! and a `kill -9` of the daemon mid-turn with the client restarting it on demand.
#![cfg(unix)]

mod common;

use std::process::Stdio;

use common::*;
use marsad_proto::agentconfig::{
    ItemOverride, ItemValue, LaunchRequest, LayerScope, OverrideAction, OverrideLayer,
};
use marsad_proto::runtime::{Phase, SessionEvent, SessionEventKind};
use marsad_proto::store::{NotificationPrefs, RecentProject, SeenMark};
use marsad_proto::{AgentEvent, Command, ConfigItemKind, GateDecision, HostId, McpTransport};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use wasit_fake_cli::scenario::{Scenario, Turn};

#[tokio::test]
async fn readiness_single_instance_version_mcp_and_stop() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let mut child = tokio::process::Command::new(agent())
        .arg("serve")
        .arg("--data")
        .arg(&data)
        .args(serve_args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        T,
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line),
    )
    .await
    .expect("readiness line")
    .unwrap();
    let ready: marsad_proto::daemon::Readiness = serde_json::from_str(line.trim()).unwrap();
    assert!(ready.ready);

    // A second daemon on the same endpoint refuses to start.
    let second = tokio::process::Command::new(agent())
        .arg("serve")
        .arg("--data")
        .arg(&data)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .unwrap();
    assert_eq!(second.code(), Some(3));

    let v = std::process::Command::new(agent())
        .arg("--version")
        .output()
        .unwrap();
    let info: marsad_proto::daemon::VersionInfo =
        serde_json::from_str(String::from_utf8_lossy(&v.stdout).trim()).unwrap();
    assert_eq!(info.build, ready.build);
    assert!(info.target.contains('-'));

    // `mcp` is the Wasl MCP server: it speaks MCP on stdio and asks this daemon's node.
    let bad = std::process::Command::new(agent())
        .arg("mcp")
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2), "missing --socket/--session");
    let mut mcp = tokio::process::Command::new(agent())
        .arg("mcp")
        .arg("--socket")
        .arg(data.join("marsad.sock"))
        .arg("--session")
        .arg("aaaaaaaaaaaa")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = mcp.stdin.take().unwrap();
    let mut out = BufReader::new(mcp.stdout.take().unwrap()).lines();
    use tokio::io::AsyncWriteExt;
    let rpc = |v: serde_json::Value| {
        let mut l = v.to_string();
        l.push('\n');
        l
    };
    stdin
        .write_all(
            rpc(
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"test","version":"0"}}}),
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let init = tokio::time::timeout(T, out.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(init.contains("\"wasl\""), "{init}");
    stdin
        .write_all(rpc(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).as_bytes())
        .await
        .unwrap();
    stdin
        .write_all(rpc(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).as_bytes())
        .await
        .unwrap();
    let tools = tokio::time::timeout(T, out.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    for t in ["list_peers", "delegate", "task_wait"] {
        assert!(tools.contains(t), "{t} missing: {tools}");
    }
    stdin
        .write_all(
            rpc(json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"list_peers","arguments":{}}}))
            .as_bytes(),
        )
        .await
        .unwrap();
    let call = tokio::time::timeout(T, out.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        call.contains("no session aaaaaaaaaaaa"),
        "the node answered for the unknown session: {call}"
    );
    drop(stdin);
    let _ = mcp.wait().await;

    // `stop` makes the daemon exit and frees its lock.
    let st = tokio::process::Command::new(agent())
        .arg("stop")
        .arg("--data")
        .arg(&data)
        .output()
        .await
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let code = tokio::time::timeout(T, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(code.success());
    let again = tokio::process::Command::new(agent())
        .arg("stop")
        .arg("--data")
        .arg(&data)
        .output()
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&again.stdout).contains("no daemon running"));
}

fn gate_scenario() -> Scenario {
    Scenario::builder()
        .turn(
            Turn::builder()
                .tool("Bash", json!({"command": "ls"}))
                .needs_permission()
                .result_if_allowed("listed")
                .done()
                .say("finished listing"),
        )
        .build()
}

async fn next_event(sub: &mut wasit_runtime::TypedSubscription<SessionEvent>) -> SessionEvent {
    match tokio::time::timeout(T, sub.next()).await {
        Ok(Some(Ok((_, e)))) => e,
        other => panic!(
            "no session event: {:?}",
            other.map(|o| o.map(|r| r.map(|(c, _)| c)))
        ),
    }
}

fn is_phase(e: &SessionEvent, name: &str) -> bool {
    matches!(&e.kind, SessionEventKind::Phase { phase } if phase.name() == name)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_prompt_gate_answer_and_kill9_restart() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let c = client(&data); // starts the daemon on demand

    // Session layer with an added MCP server and the scenario environment.
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        items: vec![ItemOverride {
            kind: ConfigItemKind::McpServer,
            name: "extra".into(),
            driver: None,
            action: OverrideAction::Set {
                value: ItemValue::McpServer {
                    server: McpTransport::Stdio {
                        command: "cat".into(),
                        args: vec![],
                        env: Default::default(),
                    },
                },
            },
        }],
        env: scenario_env(d.path(), &gate_scenario()),
        ..OverrideLayer::default()
    };
    let launched = c.launch(req).await.unwrap();
    assert_eq!(launched.host, HostId::new("hub"));
    let extra = launched
        .effective
        .item(ConfigItemKind::McpServer, "extra")
        .expect("the added server is in the effective config");
    assert_eq!(extra.state, marsad_proto::agentconfig::ItemState::Added);
    let id = launched.id.clone();
    // The session layer was stored under the new session.
    let stored = c
        .layer(LayerScope::Session {
            host: launched.host.clone(),
            session: id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(stored.items.len(), 1);
    assert_eq!(
        c.recent_projects().await.unwrap()[0].path,
        work.to_string_lossy()
    );

    let rt = c.runtime(None);
    let mut sub = rt.subscribe_session(&id, None);
    let mut seen: Vec<SessionEvent> = Vec::new();
    loop {
        let e = next_event(&mut sub).await;
        let idle = is_phase(&e, "idle");
        seen.push(e);
        if idle {
            break;
        }
    }
    rt.command(
        &id,
        Command::Prompt {
            text: "list".into(),
        },
    )
    .await
    .unwrap();
    let request_id = loop {
        let e = next_event(&mut sub).await;
        let rid = match &e.kind {
            SessionEventKind::Agent {
                event: AgentEvent::GateRequested { gate },
            } => Some(gate.request_id().to_string()),
            _ => None,
        };
        seen.push(e);
        if let Some(r) = rid {
            break r;
        }
    };
    assert_eq!(rt.snapshot(&id).await.unwrap().phase, Phase::NeedsYou);

    // kill -9 the daemon mid-turn; the session (held by its shim) keeps waiting on the gate,
    // and the client starts a new daemon on demand.
    let pid = c.daemon_info(None).await.unwrap().pid;
    let mut state = c.link().state();
    sh(d.path(), &["kill", "-9", &pid.to_string()]);
    // The link notices (signalled), then reconnects to a daemon the client started.
    tokio::time::timeout(T, state.wait_for(|s| !s.is_connected()))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(T, state.wait_for(|s| s.is_connected()))
        .await
        .unwrap()
        .unwrap();
    rt.command(
        &id,
        Command::AnswerGate {
            request_id,
            decision: GateDecision::choose("allow"),
        },
    )
    .await
    .unwrap();
    assert_ne!(
        c.daemon_info(None).await.unwrap().pid,
        pid,
        "a new daemon serves"
    );
    loop {
        let e = next_event(&mut sub).await;
        let done = matches!(
            &e.kind,
            SessionEventKind::Agent {
                event: AgentEvent::TurnCompleted { .. }
            }
        );
        seen.push(e);
        if done {
            break;
        }
    }
    assert!(
        sub.subscribe_count() >= 2,
        "the durable subscription resubscribed"
    );
    let page = rt.history(&id, None, 2000).await.unwrap();
    assert_eq!(
        &page.events[..seen.len()],
        &seen[..],
        "no gap, no duplicate across the restart"
    );
    assert_eq!(rt.snapshot(&id).await.unwrap().turns_completed, 1);

    // Store access through the restarted daemon (state persisted).
    assert_eq!(c.recent_projects().await.unwrap().len(), 1);
    let prefs = NotificationPrefs {
        finished: false,
        ..NotificationPrefs::default()
    };
    assert_eq!(c.set_notification_prefs(prefs).await.unwrap(), prefs);
    assert_eq!(c.notification_prefs().await.unwrap(), prefs);
    c.set_setting("theme", json!("dark")).await.unwrap();
    assert_eq!(c.settings().await.unwrap().values["theme"], json!("dark"));
    c.mark_seen(SeenMark {
        host: HostId::new("hub"),
        session: id.clone(),
        cursor: 1,
        at_ms: 0,
    })
    .await
    .unwrap();
    let all = c.mark_all_seen().await.unwrap();
    let m = all.iter().find(|m| m.session == id).unwrap();
    assert_eq!(m.cursor, rt.snapshot(&id).await.unwrap().cursor);
    c.add_recent_project(RecentProject {
        host: HostId::new("hub"),
        path: "/x".into(),
        last_used_ms: 0,
    })
    .await
    .unwrap();
    assert_eq!(c.recent_projects().await.unwrap()[0].path, "/x");

    // Typed errors survive the wire.
    let err = c.launch(LaunchRequest::new("nope")).await.unwrap_err();
    assert!(
        matches!(
            err,
            marsad_client::ClientError::Marsad(marsad_proto::MarsadError::InvalidRequest { .. })
        ),
        "{err:?}"
    );
    rt.delete(&id).await.unwrap();
    let pid = c.daemon_info(None).await.unwrap().pid;
    sh(d.path(), &["kill", &pid.to_string()]);
}

#[tokio::test]
async fn fs_and_base_config() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let c = client(&data);
    std::fs::create_dir_all(d.path().join("proj/.git")).unwrap();
    std::fs::write(d.path().join("file.txt"), "x").unwrap();
    let l = c
        .fs_list(
            None,
            marsad_proto::fs::FsListRequest {
                path: d.path().to_string_lossy().into_owned(),
                show_hidden: false,
                dirs_only: true,
            },
        )
        .await
        .unwrap();
    let proj = l.entries.iter().find(|e| e.name == "proj").unwrap();
    assert!(proj.is_dir && proj.is_git_repo);
    assert!(l.entries.iter().all(|e| e.is_dir));
    assert!(
        c.fs_exists(None, &d.path().join("file.txt").to_string_lossy())
            .await
            .unwrap()
            .exists
    );
    assert!(!c.fs_home(None).await.unwrap().is_empty());
    let err = c
        .fs_list(
            None,
            marsad_proto::fs::FsListRequest {
                path: "/definitely/not/here".into(),
                show_hidden: false,
                dirs_only: false,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        marsad_client::ClientError::Marsad(marsad_proto::MarsadError::NotFound { .. })
    ));
    // The fake CLI has no config commands: the base layer comes back with the probe errors,
    // never silently empty.
    let base = c
        .agentconfig_base(None, "claude", None, false)
        .await
        .unwrap();
    assert!(!base.refreshing);
    assert!(!base.errors.is_empty(), "{base:?}");
    let pid = c.daemon_info(None).await.unwrap().pid;
    sh(d.path(), &["kill", &pid.to_string()]);
}
