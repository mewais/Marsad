//! Live: a workstream of real CLIs — a claude orchestrator (cheapest model) delegating through
//! the automatically injected Wasl MCP server to an opencode and a kilo worker, with the
//! handoff gate answered through `session.command` like a client would.
//!
//! Opt-in: `MARSAD_LIVE=1 cargo test -p marsad-agent --test live_workstream -- --ignored
//! --nocapture`. Needs `claude` (authenticated), `opencode` and `kilo` on PATH and network
//! access; without `MARSAD_LIVE=1` it fails, saying how to run it.
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::*;
use marsad_proto::wasl::{EdgePolicy, TaskState};
use marsad_proto::workstream::{
    NodeLaunch, WorkstreamDraft, WorkstreamEdge, WorkstreamNode, WorkstreamView, WorkstreamsEvent,
};
use marsad_proto::{Command, GateChoiceKind, GateDecision, GateRequest};

fn enabled() -> bool {
    std::env::var("MARSAD_LIVE").is_ok_and(|v| v == "1")
}

fn node(id: &str, driver: &str, model: &str, cwd: &std::path::Path) -> WorkstreamNode {
    let mut l = NodeLaunch::new(driver);
    l.model = Some(model.into());
    l.cwd = Some(cwd.to_string_lossy().into_owned());
    WorkstreamNode {
        id: id.into(),
        name: id.into(),
        role: String::new(),
        description: format!("a {driver} agent"),
        session: None,
        launch: Some(l),
        spawned_by: None,
        spawn_request: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn live_claude_orchestrates_opencode_and_kilo() {
    if !enabled() {
        panic!("{}", live_hint("live_workstream"));
    }
    let d = TempDir::new();
    let mut cfg = client_config(
        d.path().join("l"),
        wasit_control::Hello::new("test", "gui", "gui"),
    );
    cfg.serve_args.extend(["--host-id".into(), "hub".into()]);
    let c = marsad_client::MarsadClient::connect(cfg);
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let mut orch = node("orchestrator", "claude", "haiku", &work);
    if let Some(l) = &mut orch.launch {
        l.first_prompt = Some(
            "You have two peers, \"opencode-worker\" and \"kilo-worker\". Use the wasl `delegate` \
             tool ONCE with both peers as targets and mode consult, instruction exactly: \
             \"What is 17 + 25? Reply with just the number.\" Then call `task_wait` on the task \
             ids, and finally reply with only the two numbers they answered, separated by a \
             space."
                .into(),
        );
    }
    let ws = c
        .create_workstream(WorkstreamDraft {
            name: "Live".into(),
            nodes: vec![
                orch,
                node(
                    "opencode-worker",
                    "opencode",
                    "opencode/deepseek-v4.1-flash-free",
                    &work,
                ),
                node("kilo-worker", "kilo", "kilo/kilo-auto/free", &work),
            ],
            edges: vec![
                WorkstreamEdge {
                    from: "orchestrator".into(),
                    to: "opencode-worker".into(),
                    policy: EdgePolicy::Ask,
                },
                WorkstreamEdge {
                    from: "orchestrator".into(),
                    to: "kilo-worker".into(),
                    policy: EdgePolicy::AlwaysAllow,
                },
            ],
            ..WorkstreamDraft::default()
        })
        .await
        .unwrap();
    let mut sub = c.workstreams_topic(None);
    let launched = c.launch_workstream(&ws.id).await.unwrap();
    for o in &launched.launched {
        assert!(o.error.is_none(), "{}: {:?}", o.node, o.error);
    }
    let mut answered = std::collections::HashSet::new();
    let mut handoff_gates = 0;
    let mut native_wasl_prompts = Vec::new();
    let mut views: BTreeMap<String, WorkstreamView> = BTreeMap::new();
    let res = tokio::time::timeout(Duration::from_secs(600), async {
        loop {
            let ev = match sub.next().await {
                Some(Ok((_, e))) => e,
                other => panic!("workstreams topic: {:?}", other.map(|r| r.map(|_| ()))),
            };
            match ev {
                WorkstreamsEvent::Reset { workstreams } => {
                    for v in workstreams {
                        views.insert(v.workstream.id.clone(), v);
                    }
                }
                WorkstreamsEvent::Upserted { workstream } => {
                    views.insert(workstream.workstream.id.clone(), *workstream);
                }
                WorkstreamsEvent::Removed { .. } => {}
            }
            let Some(v) = views.get(&ws.id) else { continue };
            // Answer every gate once: handoffs allow-once, native permissions approve.
            for n in &v.nodes {
                let (Some(s), Some(sess)) = (
                    &n.session,
                    v.workstream.node(&n.node).and_then(|x| x.session.clone()),
                ) else {
                    continue;
                };
                for g in &s.pending_gates {
                    let rid = g.gate.request_id().to_string();
                    if !answered.insert(rid.clone()) {
                        continue;
                    }
                    println!("gate on {}: {:?}", n.node, g.gate);
                    match &g.gate {
                        GateRequest::Handoff { .. } => handoff_gates += 1,
                        GateRequest::Permission { tool_name, .. } if tool_name.contains("wasl") => {
                            native_wasl_prompts.push(tool_name.clone())
                        }
                        _ => {}
                    }
                    c.runtime(Some(sess.host.clone()))
                        .command(
                            &sess.session,
                            Command::AnswerGate {
                                request_id: rid,
                                decision: GateDecision::by_kind(
                                    &g.gate,
                                    &[GateChoiceKind::AllowOnce],
                                )
                                .expect("an allow choice"),
                            },
                        )
                        .await
                        .unwrap();
                }
            }
            let orch = v.nodes.iter().find(|n| n.node == "orchestrator");
            let done = orch
                .and_then(|n| n.session.as_ref())
                .is_some_and(|s| s.turns_completed >= 1 && s.turn.is_none());
            if done && v.edges.iter().all(|e| e.in_flight.is_empty()) {
                return v.clone();
            }
        }
    })
    .await;
    let v = res.unwrap_or_else(|_| panic!("the workstream did not finish: {views:#?}"));
    let reply = v
        .nodes
        .iter()
        .find(|n| n.node == "orchestrator")
        .and_then(|n| n.session.as_ref())
        .and_then(|s| s.last_turn.as_ref())
        .and_then(|t| t.reply.clone())
        .unwrap_or_default();
    let tasks = c
        .edge_history(marsad_proto::workstream::EdgeHistoryRequest {
            workstream: ws.id.clone(),
            from: "orchestrator".into(),
            to: "opencode-worker".into(),
            before_ms: None,
            limit: None,
        })
        .await
        .unwrap();
    println!(
        "LIVE: reply={reply:?} handoff_gates={handoff_gates} native_wasl_prompts={native_wasl_prompts:?} edges={:?} cost={:?}",
        v.edges
            .iter()
            .map(|e| (&e.to, e.completed, e.unsuccessful))
            .collect::<Vec<_>>(),
        v.cost
    );
    for t in &tasks.tasks {
        if let TaskState::Completed { result } = &t.state {
            println!("opencode-worker answered {:?}", result.summary);
        }
    }
    assert!(handoff_gates >= 1, "the ask edge raised a handoff gate");
    assert!(
        v.edges.iter().all(|e| e.completed >= 1),
        "both workers answered: {:?}",
        v.edges
    );
    assert!(reply.contains("42"), "reply: {reply:?}");
    for n in &v.workstream.nodes {
        if let Some(s) = &n.session {
            let _ = c.runtime(Some(s.host.clone())).stop(&s.session).await; // best effort
        }
    }
    if let Ok(i) = c.daemon_info(None).await {
        let _ = std::process::Command::new("kill")
            .arg(i.pid.to_string())
            .status();
    }
}

/// Live: a claude session (cheapest model) in no workstream spawns an opencode worker (free
/// model) through `spawn_agent` and has it answer 6×7. Every spawn and handoff gate is allowed
/// once, as a client would; claude's own permission prompts are allowed too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn live_claude_spawns_an_opencode_worker() {
    if !enabled() {
        panic!("{}", live_hint("live_workstream"));
    }
    let d = TempDir::new();
    let mut cfg = client_config(
        d.path().join("l"),
        wasit_control::Hello::new("test", "gui", "gui"),
    );
    cfg.serve_args.extend(["--host-id".into(), "hub".into()]);
    let c = marsad_client::MarsadClient::connect(cfg);
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let mut req = marsad_proto::agentconfig::LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.model = Some("haiku".into());
    req.first_prompt = Some(
        "Use the wasl `spawn_agent` tool to spawn an opencode worker: name \"calc\", role \
         \"answers arithmetic\", driver \"opencode\", model \"opencode/deepseek-v4.1-flash-free\", \
         instruction \"What is 6*7? Reply with just the number.\". Then call `task_wait` on the \
         returned task id (timeout_secs 300) and reply with only the number the worker answered."
            .into(),
    );
    let solo = c.launch(req).await.unwrap();
    let rt = c.runtime(None);
    let mut sub = rt.subscribe_session(&solo.id, None);
    let mut answered = std::collections::HashSet::new();
    let (mut spawn_gates, mut handoffs) = (0, 0);
    let reply = tokio::time::timeout(Duration::from_secs(900), async {
        loop {
            // The worker's own permission prompts are allowed too.
            for w in rt
                .list()
                .await
                .unwrap()
                .into_iter()
                .filter(|s| s.id != solo.id)
            {
                for g in &w.pending_gates {
                    let rid = g.gate.request_id().to_string();
                    if answered.insert(rid.clone()) {
                        eprintln!("worker asks: {:?}", g.gate);
                        if let Some(dec) =
                            GateDecision::by_kind(&g.gate, &[GateChoiceKind::AllowOnce])
                        {
                            let _ = rt
                                .command(
                                    &w.id,
                                    Command::AnswerGate {
                                        request_id: rid,
                                        decision: dec,
                                    },
                                )
                                .await;
                        }
                    }
                }
            }
            let snap = rt.snapshot(&solo.id).await.unwrap();
            for g in &snap.pending_gates {
                let rid = g.gate.request_id().to_string();
                if !answered.insert(rid.clone()) {
                    continue;
                }
                let kind = match &g.gate {
                    GateRequest::Spawn { spawn: sg, .. } => {
                        spawn_gates += 1;
                        eprintln!("spawn gate: {} ({}, {:?})", sg.name, sg.driver, sg.model);
                        GateChoiceKind::AllowOnce
                    }
                    GateRequest::Handoff { .. } => {
                        handoffs += 1;
                        GateChoiceKind::AllowOnce
                    }
                    other => {
                        eprintln!("claude asks: {other:?}");
                        GateChoiceKind::AllowOnce
                    }
                };
                if let Some(dec) = GateDecision::by_kind(&g.gate, &[kind]) {
                    rt.command(
                        &solo.id,
                        Command::AnswerGate {
                            request_id: rid,
                            decision: dec,
                        },
                    )
                    .await
                    .unwrap();
                }
            }
            if snap.phase.name() == "idle" {
                if let Some(r) = snap.last_turn.as_ref().and_then(|t| t.reply.clone()) {
                    return r;
                }
            }
            if matches!(snap.phase.name(), "failed" | "exited") {
                panic!("orchestrator ended: {:?}", snap.phase);
            }
            // The orchestrator's events, or a worker's (bounded: workers are not subscribed).
            let _ = tokio::time::timeout(Duration::from_secs(2), sub.next()).await;
        }
    })
    .await
    .expect("the live spawn finished in time");
    eprintln!(
        "orchestrator replied: {reply:?} (spawn gates {spawn_gates}, handoff gates {handoffs})"
    );
    let all = c.workstreams().await.unwrap();
    eprintln!(
        "workstreams: {:?}",
        all.iter()
            .map(|w| (&w.name, w.nodes.len()))
            .collect::<Vec<_>>()
    );
    assert_eq!(spawn_gates, 1);
    assert!(reply.contains("42"), "{reply}");
    assert!(all
        .iter()
        .any(|w| w.nodes.iter().any(|n| n.spawned_by.is_some()
            && n.launch
                .as_ref()
                .is_some_and(|l| l.driver.as_str() == "opencode"))));
}
