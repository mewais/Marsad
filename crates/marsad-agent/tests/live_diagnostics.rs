//! Live: the diagnostics of a real Claude Code session hold no content. The proof of
//! `diagnostics.rs`'s `no_content_in_any_log_or_export`, once more, with the real `claude` CLI on
//! PATH instead of the stand-in: two cheap model calls in a temporary project directory whose
//! name, session name, prompts, reply and a host secret carry unique markers. Afterwards no file
//! the diagnostics could reach holds a marker, and the journal, the export and the hub log hold
//! what they must.
//!
//! Opt-in: `MARSAD_LIVE=1 cargo test -p marsad-agent --test live_diagnostics -- --ignored
//! --nocapture`. Needs `claude` (authenticated) on PATH and network access; without
//! `MARSAD_LIVE=1`, or without `claude`, it fails.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use marsad_client::{ClientConfig, MarsadClient};
use marsad_proto::agentconfig::LaunchRequest;
use marsad_proto::export::ExportRequest;
use marsad_proto::hosts::{HostEnvSettings, HostVar};
use marsad_proto::runtime::{SessionEvent, SessionEventKind};
use marsad_proto::{methods, AgentEvent, Command, HostId};
use serde_json::Value;
use wasit_control::{Hello, TraceId};

/// A generous bound for each real model turn (the existing live tests bound whole workstreams
/// by 600 s).
const TURN_BOUND: Duration = Duration::from_secs(300);
/// The session's end after `stop`: the shim writes its last records then.
const END_BOUND: Duration = Duration::from_secs(120);

/// The first `claude` on PATH, as the daemon finds it.
fn claude_on_path() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join("claude"))
        .find(|p| p.is_file())
}

/// A hub daemon started by its client with the real drivers (no stand-in binaries).
fn live_client(data: &Path) -> MarsadClient {
    let mut cfg = ClientConfig::new(data, Hello::new("test", "gui-live", "gui"));
    cfg.agent_bin = Some(agent());
    cfg.serve_args = vec!["--host-id".into(), "hub".into()];
    MarsadClient::connect(cfg)
}

/// Follows `sub` until the turn ends. The prompts forbid tools, so a gate is a failure to report,
/// and so is a session that ends.
async fn turn_completes(sub: &mut wasit_runtime::TypedSubscription<SessionEvent>) {
    tokio::time::timeout(TURN_BOUND, async {
        loop {
            let e = match sub.next().await {
                Some(Ok((_, e))) => e,
                other => panic!(
                    "the session's events ended: {:?}",
                    other.map(|r| r.map(|_| ()))
                ),
            };
            match &e.kind {
                SessionEventKind::Agent {
                    event: AgentEvent::TurnCompleted { .. },
                } => return,
                SessionEventKind::Agent {
                    event: AgentEvent::GateRequested { gate },
                } => panic!("claude asked for a gate though the prompt forbids tools: {gate:?}"),
                SessionEventKind::Phase { phase, .. }
                    if matches!(phase.name(), "failed" | "exited") =>
                {
                    panic!("the session ended during a turn: {}", phase.name())
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the turn completed within its bound");
}

/// Follows `sub` until the session has exited.
async fn session_exits(sub: &mut wasit_runtime::TypedSubscription<SessionEvent>) {
    tokio::time::timeout(END_BOUND, async {
        loop {
            match sub.next().await {
                Some(Ok((_, e))) => {
                    if matches!(&e.kind, SessionEventKind::Phase { phase, .. } if phase.name() == "exited")
                    {
                        return;
                    }
                }
                other => panic!(
                    "the session's events ended before it exited: {:?}",
                    other.map(|r| r.map(|_| ()))
                ),
            }
        }
    })
    .await
    .expect("the session exited within its bound");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs a logged-in claude CLI and MARSAD_LIVE=1"]
async fn a_real_claude_session_leaves_no_content_in_any_diagnostic_file() {
    if std::env::var("MARSAD_LIVE").as_deref() != Ok("1") {
        panic!("{}", live_hint("live_diagnostics"));
    }
    let Some(claude) = claude_on_path() else {
        panic!(
            "the claude CLI is not on PATH: install Claude Code and log in with `claude`, then run \
             `MARSAD_LIVE=1 cargo test -p marsad-agent --test live_diagnostics -- --ignored --nocapture`"
        );
    };
    eprintln!("driver binary: {}", claude.display());

    // Unique markers: the project directory's name, the session's name, two prompts, the reply
    // the second prompt asks for, a host secret and the trace of the first launch.
    const PROJECT: &str = "LIVEPROJECTDIR-5c8e1d";
    const SESSION_NAME: &str = "LIVESESSIONNAME-a7d2e9";
    const PROMPT1: &str = "LIVEPROMPT-one-41b8f0";
    const PROMPT2: &str = "LIVEPROMPT-two-9e6c3a";
    const REPLY: &str = "LIVEREPLY-c0d7b5";
    const SECRET: &str = "LIVEHOSTSECRET-2f8a6e";
    const TRACE: &str = "u-livediag00000001";
    let markers = [PROJECT, SESSION_NAME, PROMPT1, PROMPT2, REPLY, SECRET];

    let d = TempDir::new();
    let data = d.path().join("d");
    let l = layout(&data);
    let work = d.path().join(PROJECT);
    std::fs::create_dir_all(&work).unwrap();

    let c = live_client(&data); // starts the daemon on demand, the way the GUI does

    // A host secret variable on the local host, through the host-environment method.
    let saved = c
        .set_host_env(HostEnvSettings {
            host: HostId::new("hub"),
            setup: String::new(),
            vars: vec![HostVar {
                name: "LIVE_DIAG_SECRET".into(),
                value: Some(SECRET.into()),
                secret: true,
            }],
            commands: Default::default(),
            revision: String::new(),
        })
        .await
        .unwrap();
    assert!(saved.not_applied.is_none(), "{saved:?}");

    // The session: haiku, the project directory, the session's name, a traced launch whose
    // first prompt names no tools.
    let mut req = LaunchRequest::new("claude");
    req.model = Some("haiku".into());
    req.name = Some(SESSION_NAME.into());
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.first_prompt = Some(format!(
        "Reply with exactly: OK. Do not use any tools. {PROMPT1}"
    ));
    let trace = TraceId::new(TRACE).unwrap();
    let launched: marsad_proto::agentconfig::LaunchReply = c
        .call_traced(None, methods::SESSIONS_LAUNCH, req, Some(trace))
        .await
        .unwrap();
    let id = launched.id.clone();
    let sid = id.to_string();
    let rt = c.runtime(None);
    let mut sub = rt.subscribe_session(&id, None);
    turn_completes(&mut sub).await;

    // The second turn: a prompt that asks for the reply marker.
    rt.command(
        &id,
        Command::prompt(format!(
            "Reply with exactly this word and nothing else: {REPLY}. Context marker: {PROMPT2}"
        )),
    )
    .await
    .unwrap();
    turn_completes(&mut sub).await;

    // End the session (the shim writes its last records), then export.
    rt.stop(&id).await.unwrap();
    session_exits(&mut sub).await;
    drop(sub);
    let snap = rt.snapshot(&id).await.unwrap();
    let cli_version = snap.cli_version.clone();
    let reply = snap
        .last_turn
        .as_ref()
        .and_then(|t| t.reply.clone())
        .unwrap_or_default();
    eprintln!("claude said (second turn): {reply:?}");
    let e = exporting::export(&c, &data, ExportRequest::default()).await;
    drop(rt);
    drop(c);
    stop(&data).await;

    // ── positive: the channels were driven, and the diagnostics hold what they must ──
    let session_dir = l.data.join("sessions").join(&sid);
    let journal = std::fs::read_to_string(session_dir.join("journal.ndjson")).unwrap();
    for m in [PROMPT1, PROMPT2, REPLY] {
        assert!(contains_any(&journal, m), "{m} is not in the journal");
    }
    let session_file = std::fs::read_to_string(session_dir.join("session.json")).unwrap();
    for m in [SESSION_NAME, PROJECT] {
        assert!(contains_any(&session_file, m), "{m} is not in session.json");
    }

    let m = e.manifest();
    assert_eq!(m["hub_host"], "hub", "{m}");
    assert!(
        e.section("host:hub", "daemon")
            .iter()
            .any(|r| r["ev"] == "daemon.start"),
        "the hub's daemon section has no daemon.start"
    );

    let trail = e.trail("host:hub", &sid);
    let of_kind =
        |ev: &str| -> Vec<&Value> { trail.iter().copied().filter(|t| t["ev"] == ev).collect() };
    let first = of_kind("input.applied")
        .into_iter()
        .find(|t| t["trace"] == TRACE && t["kind"] == "prompt")
        .unwrap_or_else(|| {
            panic!(
                "the first prompt was not applied under the launch's trace: {:#?}",
                of_kind("input.applied")
            )
        });
    let first_input = first["input"].clone();
    let for_first = |ev: &str| {
        trail
            .iter()
            .filter(|t| t["ev"] == ev && t["input"] == first_input)
            .count()
    };
    assert!(
        for_first("input.written") >= 1,
        "no input.written for the first prompt"
    );
    if for_first("input.acknowledged") == 0 {
        let for_input: Vec<&Value> = trail
            .iter()
            .copied()
            .filter(|t| t["input"] == first_input)
            .collect();
        eprintln!("the trail for the first prompt (input {first_input}): {for_input:#?}");
        panic!(
            "the installed claude CLI ({cli_version:?}) did not report input.acknowledged for the \
             first prompt; the trail for that input is printed above"
        );
    }
    assert!(
        of_kind("phase").iter().any(|t| t["cause"].is_object()),
        "no phase with a cause in the trail"
    );

    // The hub's log: the launch and the request it served, under the trace.
    let hub = records(&l.diag().join("daemon.ndjson"));
    assert!(
        of(&hub, "launch").iter().any(|r| r["trace"] == TRACE),
        "no launch record under the trace"
    );
    assert!(
        of(&hub, "rpc.traced").iter().any(|r| r["trace"] == TRACE),
        "no rpc.traced record under the trace"
    );

    // ── negative: no marker in any file, in any form ──
    // (`assert_no_markers` reads the export's gzip as the text it holds.)
    assert_no_markers(&l, &markers, 2);

    let export_size = std::fs::metadata(&e.path).unwrap().len();
    eprintln!(
        "LIVE SUMMARY: claude CLI {}; {} trail entries for the session; export {} bytes ({} lines)",
        cli_version.as_deref().unwrap_or("version not reported"),
        trail.len(),
        export_size,
        e.lines.len()
    );
}
