//! Agents spawning workers on demand, end to end with real processes: the hub daemon (the spawn
//! authority), a second daemon behind a real sshd on localhost, the embedded Wasl nodes and the
//! fake agent CLI. Spawn requests are made the way the Wasl MCP server makes them (`wasl.spawn`
//! on the asking session's host); gates are answered through `session.command`.
//! The ssh-host scenario needs a local sshd (Linux CI); the standalone one runs everywhere.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use common::*;
use marsad_client::{MarsadClient, Subscription};
use marsad_proto::agentconfig::{LaunchRequest, LayerScope, OverrideLayer};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::hosts::HostDraft;
use marsad_proto::wasl::{EdgePolicy, HandoffDecision, Limits, TaskId, TaskState, TaskStates};
use marsad_proto::workstream::{
    NodeLaunch, NodeSession, SpawnPolicy, SpawnSettings, Workstream, WorkstreamDraft,
    WorkstreamNode, WorkstreamState, WorkstreamUpdate, WorkstreamView, WorkstreamsEvent,
};
use marsad_proto::{Command, GateDecision, GateRequest, HostId, SessionId, SessionSnapshot};
use serde_json::{json, Value};
use wasit_fake_cli::scenario::{Scenario, Turn};
use wasl_proto::{End, EndCode, SpawnDecision, SpawnReply};

fn hub_client(dir: &Path) -> MarsadClient {
    let mut cfg = client_config(dir, wasit_control::Hello::new("test", "gui", "gui"));
    cfg.serve_args.extend(serve_args());
    cfg.supervise.backoff.initial = Duration::from_millis(50);
    cfg.supervise.backoff.max = Duration::from_millis(500);
    MarsadClient::connect(cfg)
}

/// What every spawned worker plays (set on the host layers).
fn worker() -> Scenario {
    let mut b = Scenario::builder();
    for _ in 0..4 {
        b = b.turn(Turn::builder().say("The answer is 42."));
    }
    b.build()
}

fn orch_node(dir: &Path) -> WorkstreamNode {
    let mut l = NodeLaunch::new("claude");
    l.cwd = Some(dir.to_string_lossy().into_owned());
    l.overrides = OverrideLayer {
        env: scenario_env(dir, &Scenario::builder().build()),
        ..OverrideLayer::default()
    };
    WorkstreamNode::launched("orch", "orchestrator", l)
}

/// Follows the `workstreams` topic, keeping the latest view of each workstream.
struct Views {
    sub: Subscription<WorkstreamsEvent>,
    views: BTreeMap<String, WorkstreamView>,
}

impl Views {
    fn new(c: &MarsadClient) -> Views {
        Views {
            sub: c.workstreams_topic(None),
            views: BTreeMap::new(),
        }
    }

    async fn until(
        &mut self,
        id: &str,
        what: &str,
        pred: impl Fn(&WorkstreamView) -> bool,
    ) -> WorkstreamView {
        eprintln!("waiting for {what}");
        let fut = async {
            loop {
                if let Some(v) = self.views.get(id) {
                    if pred(v) {
                        return v.clone();
                    }
                }
                let ev = match self.sub.next().await {
                    Some(Ok((_, e))) => e,
                    // The hub restarted: the subscription resumes.
                    Some(Err(_)) => continue,
                    None => panic!("workstreams topic ended"),
                };
                match ev {
                    WorkstreamsEvent::Reset { workstreams } => {
                        self.views = workstreams
                            .into_iter()
                            .map(|v| (v.workstream.id.clone(), v))
                            .collect();
                    }
                    WorkstreamsEvent::Upserted { workstream } => {
                        self.views
                            .insert(workstream.workstream.id.clone(), *workstream);
                    }
                    WorkstreamsEvent::Removed { id } => {
                        self.views.remove(&id);
                    }
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(120), fut)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}: {:#?}", self.views.get(id)))
    }
}

fn session_of(w: &Workstream, node: &str) -> NodeSession {
    w.node(node)
        .and_then(|n| n.session.clone())
        .unwrap_or_else(|| panic!("node {node} has no session"))
}

fn spawn_params(from: &NodeSession, rid: &str, spec: Value) -> Value {
    json!({"from": from.session, "request_id": rid, "spec": spec})
}

/// `wasl.spawn` on the asking session's host (as the MCP server does), in the background.
fn spawn_bg(
    c: &MarsadClient,
    from: &NodeSession,
    rid: &str,
    spec: Value,
) -> tokio::task::JoinHandle<Result<SpawnReply, String>> {
    let c = c.clone();
    let from = from.clone();
    let p = spawn_params(&from, rid, spec);
    tokio::spawn(async move {
        c.call_raw(Some(&from.host), "wasl.spawn", p)
            .await
            .map(|v| serde_json::from_value(v).unwrap())
            .map_err(|e| e.to_string())
    })
}

async fn spawn_now(
    c: &MarsadClient,
    from: &NodeSession,
    rid: &str,
    spec: Value,
) -> Result<SpawnReply, String> {
    tokio::time::timeout(Duration::from_secs(120), spawn_bg(c, from, rid, spec))
        .await
        .expect("spawn answered")
        .unwrap()
}

async fn answer(c: &MarsadClient, s: &NodeSession, request_id: &str, decision: GateDecision) {
    c.runtime(Some(s.host.clone()))
        .command(
            &s.session,
            Command::AnswerGate {
                request_id: request_id.into(),
                decision,
            },
        )
        .await
        .unwrap();
}

/// Wait for a snapshot of `s` matching `pred` (signalled by the session's events).
async fn wait_snap(
    c: &MarsadClient,
    s: &NodeSession,
    what: &str,
    pred: impl Fn(&SessionSnapshot) -> bool,
) -> SessionSnapshot {
    eprintln!("waiting for {what}");
    let rt = c.runtime(Some(s.host.clone()));
    let fut = async {
        let mut sub = rt.subscribe_session(&s.session, None);
        loop {
            if let Ok(snap) = rt.snapshot(&s.session).await {
                if pred(&snap) {
                    return snap;
                }
            }
            // Next event (or a resubscription after a restart), then look again.
            let _ = sub.next().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// The spawn gate on `s`; fails at once if the spawn call ends first.
async fn gate_on(
    c: &MarsadClient,
    s: &NodeSession,
    pending: &mut tokio::task::JoinHandle<Result<SpawnReply, String>>,
) -> GateRequest {
    tokio::select! {
        r = pending => panic!("the spawn call ended before a gate: {r:?}"),
        snap = wait_snap(c, s, "spawn gate", |x| spawn_gate_of(x).is_some()) => spawn_gate_of(&snap).unwrap(),
    }
}

fn spawn_gate_of(snap: &SessionSnapshot) -> Option<GateRequest> {
    snap.pending_gates
        .iter()
        .find(|g| matches!(g.gate, GateRequest::Spawn { .. }))
        .map(|g| g.gate.clone())
}

async fn wait_done(c: &MarsadClient, host: &HostId, id: &TaskId) -> TaskState {
    let fut = async {
        loop {
            let v = c
                .call_raw(
                    Some(host),
                    "wasl.task.wait",
                    json!({"ids": [id], "timeout_ms": 30000}),
                )
                .await;
            let Ok(v) = v else { continue };
            let s: TaskStates = serde_json::from_value(v).unwrap();
            if let Some(t) = s.tasks.first().filter(|t| t.state.is_terminal()) {
                return t.state.clone();
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .expect("task finished")
}

async fn set_host_layers(c: &MarsadClient, dir: &Path, hosts: &[&HostId]) {
    for h in hosts {
        c.set_layer(
            LayerScope::Host { host: (*h).clone() },
            OverrideLayer {
                env: scenario_env(dir, &worker()),
                ..OverrideLayer::default()
            },
        )
        .await
        .unwrap();
    }
}

async fn update_spawn(c: &MarsadClient, id: &str, f: impl FnOnce(&mut SpawnSettings)) {
    let mut w = c.workstream(id).await.unwrap();
    f(&mut w.spawn);
    c.update_workstream(WorkstreamUpdate {
        workstream: w,
        expected_revision: None,
    })
    .await
    .unwrap();
}

/// Sessions on `host` labelled as node `node`.
async fn sessions_of_node(c: &MarsadClient, host: &HostId, node: &str) -> usize {
    c.runtime(Some(host.clone()))
        .list()
        .await
        .unwrap()
        .iter()
        .filter(|s| s.labels.get("marsad.node").map(String::as_str) == Some(node))
        .count()
}

/// Wait until the hub reports `host`'s link connected.
async fn connected(c: &MarsadClient, host: &HostId) {
    let mut fleet = c.fleet(None);
    let fut = async {
        loop {
            match next(&mut fleet).await.1 {
                FleetEvent::Link { host: h, state } if h == *host && state.is_connected() => return,
                FleetEvent::Reset { hosts } => {
                    let up = hosts.iter().any(|f| {
                        f.host.id == *host
                            && matches!(&f.link, marsad_proto::fleet::FleetLink::Remote { state } if state.is_connected())
                    });
                    if up {
                        return;
                    }
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .expect("host connected");
}

/// orchestrator (hub host) spawns `calc` on the ssh host with a first task (allow once; the
/// task's handoff still asks); `calc` (a remote node) spawns `helper` and the human extends
/// "always allow" to handoffs; depth and count limits; a spawn whose gate waits through a hub
/// restart launches exactly one agent and a repeated launch returns it; dismissal only by the
/// spawner; archiving stops the spawned agents.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_across_local_and_ssh_host() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let c = hub_client(&d.path().join("l"));
    let hub = HostId::new("hub");
    let bx = HostId::new("box");
    c.add_host(HostDraft {
        name: "Box".into(),
        kind: sshd.kind(&sshd.key),
        id: Some(bx.clone()),
    })
    .await
    .unwrap();
    connected(&c, &bx).await;
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    set_host_layers(&c, d.path(), &[&hub, &bx]).await;

    let ws = c
        .create_workstream(WorkstreamDraft {
            name: "Spawning".into(),
            nodes: vec![orch_node(&work)],
            limits: Limits::default(),
            spawn: SpawnSettings {
                allowed_hosts: vec![bx.clone()],
                max_agents: 3,
                ..SpawnSettings::default()
            },
            ..WorkstreamDraft::default()
        })
        .await
        .unwrap();
    let mut views = Views::new(&c);
    let launched = c.launch_workstream(&ws.id).await.unwrap();
    let orch = session_of(&launched.workstream, "orch");
    let cwd = work.to_string_lossy().into_owned();

    // 1. orchestrator → calc on the ssh host, with a first task; allow once.
    let pending = spawn_bg(
        &c,
        &orch,
        "r1",
        json!({"name": "calc", "role": "does arithmetic", "host": "Box", "cwd": cwd, "instruction": "what is 6*7?"}),
    );
    let v = views
        .until(&ws.id, "spawn gate in the workstream view", |v| {
            !v.pending_spawns.is_empty()
        })
        .await;
    let g = &v.pending_spawns[0];
    assert_eq!(g.node, "orch");
    match &g.gate {
        GateRequest::Spawn { spawn: g, .. } => {
            assert_eq!(g.from, "orchestrator");
            assert_eq!(g.name, "calc");
            assert_eq!(g.role, "does arithmetic");
            assert_eq!(g.host, "Box");
            assert!(!g.driver.is_empty());
            assert_eq!(g.workstream, "Spawning");
            assert!(!g.new_workstream);
            assert_eq!(g.instruction.as_deref(), Some("what is 6*7?"));
        }
        other => panic!("not a spawn gate: {other:?}"),
    }
    answer(
        &c,
        &orch,
        g.gate.request_id(),
        SpawnDecision::AllowOnce.to_gate(),
    )
    .await;
    let r1 = tokio::time::timeout(Duration::from_secs(120), pending)
        .await
        .unwrap()
        .unwrap()
        .expect("spawned");
    assert_eq!(r1.name, "calc");
    assert_eq!(r1.agent.host, "box");
    assert_eq!(r1.workstream, ws.id);
    // The connection asks: allow once does not extend to handoffs.
    let task = r1.task.expect("first task");
    assert!(
        matches!(task.state, TaskState::PendingApproval { .. }),
        "{task:?}"
    );
    let v = views
        .until(&ws.id, "calc in the graph with its handoff gate", |v| {
            v.workstream
                .node("calc")
                .is_some_and(|n| n.session.is_some())
                && v.edges
                    .iter()
                    .any(|e| e.to == "calc" && !e.pending_gates.is_empty())
        })
        .await;
    let calc_node = v.workstream.node("calc").unwrap();
    assert_eq!(calc_node.spawned_by.as_deref(), Some("orch"));
    assert_eq!(calc_node.role, "does arithmetic");
    assert_eq!(
        v.workstream.edge("orch", "calc").unwrap().policy,
        EdgePolicy::Ask
    );
    let calc = session_of(&v.workstream, "calc");
    assert_eq!(calc.host, bx);
    let hg = v
        .edges
        .iter()
        .find(|e| e.to == "calc")
        .unwrap()
        .pending_gates[0]
        .gate
        .clone();
    answer(
        &c,
        &orch,
        hg.request_id(),
        HandoffDecision::AllowOnce.to_gate(),
    )
    .await;
    let done = wait_done(&c, &hub, &task.id).await;
    match done {
        TaskState::Completed { result } => assert_eq!(result.summary, "The answer is 42."),
        other => panic!("expected completed: {other:?}"),
    }
    // The spawned session got the Wasl item like any other (it could delegate) and its name.
    let snap = wait_snap(&c, &calc, "calc named", |s| {
        s.display_name.as_deref() == Some("calc")
    })
    .await;
    assert_eq!(snap.labels.get("marsad.workstream"), Some(&ws.id));

    // 2. calc (a node on the ssh host: its node forwards to the hub) spawns helper on its own
    //    host and directory; "always allow spawning and handoffs".
    let mut pending = spawn_bg(
        &c,
        &calc,
        "r2",
        json!({"name": "helper", "role": "checks the arithmetic"}),
    );
    let gate = gate_on(&c, &calc, &mut pending).await;
    match &gate {
        GateRequest::Spawn { spawn: g, .. } => {
            let gcwd = &g.cwd;
            assert_eq!(g.from, "calc");
            assert_eq!(g.host, "Box");
            assert_eq!(gcwd, &cwd);
        }
        _ => unreachable!(),
    }
    answer(
        &c,
        &calc,
        gate.request_id(),
        SpawnDecision::AlwaysAllowWithHandoffs.to_gate(),
    )
    .await;
    let r2 = tokio::time::timeout(Duration::from_secs(120), pending)
        .await
        .unwrap()
        .unwrap()
        .expect("helper spawned");
    assert_eq!(r2.agent.host, "box");
    let w = c.workstream(&ws.id).await.unwrap();
    assert_eq!(w.spawn.policy, SpawnPolicy::AlwaysAllow);
    assert!(w.spawn.handoffs_always_allow);
    assert_eq!(
        w.node("helper").unwrap().spawned_by.as_deref(),
        Some("calc")
    );
    assert_eq!(
        w.edge("calc", "helper").unwrap().policy,
        EdgePolicy::AlwaysAllow
    );
    let helper = session_of(&w, "helper");

    // 3. Depth: helper is generation 2, the most the workstream allows.
    let e = spawn_now(&c, &helper, "r3", json!({"name": "deep", "role": "x"}))
        .await
        .expect_err("too deep");
    assert!(e.contains("generation"), "{e}");

    // 4. Always allowed now: no gate. Then the count limit (3).
    let r4 = spawn_now(
        &c,
        &orch,
        "r4",
        json!({"name": "calc3", "role": "more arithmetic"}),
    )
    .await
    .expect("no gate");
    assert_eq!(r4.agent.host, "hub");
    let e = spawn_now(&c, &orch, "r5", json!({"name": "calc4", "role": "x"}))
        .await
        .expect_err("limit");
    assert!(e.contains("3 spawned agents"), "{e}");

    // 5. A spawn whose gate waits on calc (ssh host) while the hub restarts.
    update_spawn(&c, &ws.id, |s| {
        s.policy = SpawnPolicy::Ask;
        s.max_agents = 6;
    })
    .await;
    let mut first = spawn_bg(
        &c,
        &calc,
        "late-1",
        json!({"name": "late", "role": "late work"}),
    );
    let gate = gate_on(&c, &calc, &mut first).await;
    stop_hub(&c).await;
    // The call in flight lost the hub.
    assert!(first.await.unwrap().is_err());
    // Answered after the restart (the client restarts the hub on demand).
    connected(&c, &bx).await;
    answer(
        &c,
        &calc,
        gate.request_id(),
        SpawnDecision::AllowOnce.to_gate(),
    )
    .await;
    // A retry with the same request id gets the same agent.
    let r6 = spawn_now(
        &c,
        &calc,
        "late-1",
        json!({"name": "late", "role": "late work"}),
    )
    .await
    .expect("late spawned");
    let r6b = spawn_now(
        &c,
        &calc,
        "late-1",
        json!({"name": "late", "role": "late work"}),
    )
    .await
    .unwrap();
    assert_eq!(r6.agent, r6b.agent);
    // A repeated launch (as after a hub restart mid-launch) returns it too.
    let again: wasl_proto::SpawnLaunched = serde_json::from_value(
        c.call_raw(
            None,
            "workstreams.spawn.launch",
            json!({"request_id": "late-1", "parent": format!("box/{}", calc.session),
                   "spec": {"name": "late", "role": "late work"}, "decision": {"decision": "allow_once"}}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(again.agent, r6.agent);
    assert_eq!(sessions_of_node(&c, &bx, "late").await, 1);
    let w = c.workstream(&ws.id).await.unwrap();
    assert_eq!(w.nodes.iter().filter(|n| n.name == "late").count(), 1);

    // 6. Dismiss: only the spawner may.
    let bad = c
        .call_raw(
            Some(&hub),
            "wasl.dismiss",
            json!({"from": orch.session, "peer": "helper"}),
        )
        .await
        .expect_err("not orch's worker");
    assert!(bad.to_string().contains("not spawned by you"), "{bad}");
    c.call_raw(
        Some(&bx),
        "wasl.dismiss",
        json!({"from": calc.session, "peer": "helper"}),
    )
    .await
    .unwrap();
    wait_snap(&c, &helper, "helper stopped", |s| {
        s.phase.name() == "exited"
    })
    .await;

    // 7. Archive: the spawned agents are stopped, not deleted.
    c.call_raw(None, "workstreams.archive", json!({"id": ws.id}))
        .await
        .unwrap();
    for n in ["calc", "calc3", "late"] {
        let s = session_of(&w, n);
        wait_snap(&c, &s, &format!("{n} stopped"), |s| {
            s.phase.name() == "exited"
        })
        .await;
    }
    let orch_snap = c.runtime(None).snapshot(&orch.session).await.unwrap();
    assert_ne!(
        orch_snap.phase.name(),
        "exited",
        "the orchestrator was not spawned"
    );
}

/// A session in no workstream: the gate says approving creates "<name> + workers"; deny (with a
/// reason) creates nothing; allow once creates the workstream with both agents; policy
/// "never", an unknown host and a missing directory on another host are refused with reasons.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_from_a_standalone_session() {
    let d = TempDir::new();
    let c = hub_client(&d.path().join("l"));
    let hub = HostId::new("hub");
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    set_host_layers(&c, d.path(), &[&hub]).await;
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &Scenario::builder().build()),
        ..OverrideLayer::default()
    };
    let solo = c.launch(req).await.unwrap();
    c.runtime(None)
        .rename(&solo.id, Some("Planner".into()))
        .await
        .unwrap();
    let me = NodeSession {
        host: hub.clone(),
        session: solo.id.clone(),
    };

    let mut pending = spawn_bg(
        &c,
        &me,
        "s1",
        json!({"name": "scout", "role": "looks around"}),
    );
    let gate = gate_on(&c, &me, &mut pending).await;
    match &gate {
        GateRequest::Spawn { spawn: g, .. } => {
            assert_eq!(g.workstream, "Planner + workers");
            assert!(g.new_workstream);
            assert_eq!(g.from, "Planner");
        }
        _ => unreachable!(),
    }
    answer(
        &c,
        &me,
        gate.request_id(),
        SpawnDecision::Deny {
            reason: Some("do it yourself".into()),
        }
        .to_gate(),
    )
    .await;
    let e = pending.await.unwrap().expect_err("denied");
    assert!(e.contains("do it yourself"), "{e}");
    assert!(c.workstreams().await.unwrap().is_empty());

    let mut pending = spawn_bg(
        &c,
        &me,
        "s2",
        json!({"name": "scout", "role": "looks around"}),
    );
    let gate = gate_on(&c, &me, &mut pending).await;
    answer(
        &c,
        &me,
        gate.request_id(),
        SpawnDecision::AllowOnce.to_gate(),
    )
    .await;
    let r = pending.await.unwrap().expect("spawned");
    let all = c.workstreams().await.unwrap();
    assert_eq!(all.len(), 1);
    let w = &all[0];
    assert_eq!(w.name, "Planner + workers");
    assert_eq!(w.state, WorkstreamState::Running);
    assert_eq!(w.id, r.workstream);
    let planner = w.nodes.iter().find(|n| n.spawned_by.is_none()).unwrap();
    assert_eq!(planner.session.as_ref().unwrap().session, solo.id);
    let scout = w.node("scout").unwrap();
    assert_eq!(scout.spawned_by.as_deref(), Some(planner.id.as_str()));
    assert_eq!(
        w.edge(&planner.id, "scout").unwrap().policy,
        EdgePolicy::Ask
    );
    // The new agent is reachable over the connection.
    let peers: wasl_proto::PeerList = serde_json::from_value(
        c.call_raw(Some(&hub), "wasl.peers.list", json!({"from": solo.id}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(peers.peers.iter().any(|p| p.card.name == "scout"));
    let sp = peers.spawnable.expect("spawnable");
    assert!(sp.available, "{sp:?}");
    assert_eq!(sp.remaining, Some(7));
    assert_eq!(sp.hosts[0].id, "hub");
    assert!(sp.hosts[0].drivers.iter().any(|d| d.id == "claude"));

    // Refusals.
    let e = spawn_now(
        &c,
        &me,
        "s3",
        json!({"name": "x", "role": "x", "host": "nowhere"}),
    )
    .await
    .expect_err("unknown host");
    assert!(e.contains("no host"), "{e}");
    let e = spawn_now(&c, &me, "s4", json!({"name": "scout", "role": "x"}))
        .await
        .expect_err("name taken");
    assert!(e.contains("already exists"), "{e}");
    let e = spawn_now(
        &c,
        &me,
        "s5",
        json!({"name": "y", "role": "x", "driver": "nope"}),
    )
    .await
    .expect_err("driver");
    assert!(e.contains("not installed"), "{e}");
    update_spawn(&c, &w.id, |s| s.policy = SpawnPolicy::Never).await;
    let e = spawn_now(&c, &me, "s6", json!({"name": "z", "role": "x"}))
        .await
        .expect_err("never");
    assert!(e.contains("turned off"), "{e}");
    let _ = SessionId::random();
}

/// The fake claude agent spawns a worker through its injected Wasl MCP tool (`spawn_agent` with a
/// first task), waits for the task (`task_wait`) and reports the worker's answer; the human
/// answers the spawn gate and the first handoff on the agent's session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fake_agent_spawns_through_its_mcp_tools() {
    let d = TempDir::new();
    let c = hub_client(&d.path().join("l"));
    let hub = HostId::new("hub");
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    set_host_layers(&c, d.path(), &[&hub]).await;
    let orchestrator = Scenario::builder()
        .turn(
            Turn::builder()
                .say("I'll get a worker for that.")
                .mcp_call(
                    "wasl",
                    "spawn_agent",
                    json!({"name": "calc", "role": "does arithmetic", "instruction": "what is 6*7?"}),
                    Some("s"),
                )
                .mcp_call(
                    "wasl",
                    "task_wait",
                    json!({"ids": ["{{s.task.id}}"], "timeout_secs": 60}),
                    Some("w"),
                )
                .say("calc said: {{w.tasks.0.result}}"),
        )
        .build();
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &orchestrator),
        ..OverrideLayer::default()
    };
    let solo = c.launch(req).await.unwrap();
    let me = NodeSession {
        host: hub.clone(),
        session: solo.id.clone(),
    };
    wait_snap(&c, &me, "idle", |s| s.phase.name() == "idle").await;
    c.runtime(None)
        .command(&solo.id, Command::prompt("work out 6*7 with a helper"))
        .await
        .unwrap();
    let snap = wait_snap(&c, &me, "spawn gate from the agent's tool", |s| {
        spawn_gate_of(s).is_some()
    })
    .await;
    let gate = spawn_gate_of(&snap).unwrap();
    answer(
        &c,
        &me,
        gate.request_id(),
        SpawnDecision::AllowOnce.to_gate(),
    )
    .await;
    let snap = wait_snap(&c, &me, "the first task's handoff gate", |s| {
        s.pending_gates
            .iter()
            .any(|g| matches!(g.gate, GateRequest::Handoff { .. }))
    })
    .await;
    let hg = snap
        .pending_gates
        .iter()
        .find(|g| matches!(g.gate, GateRequest::Handoff { .. }))
        .unwrap()
        .gate
        .clone();
    answer(
        &c,
        &me,
        hg.request_id(),
        HandoffDecision::AllowOnce.to_gate(),
    )
    .await;
    let snap = wait_snap(&c, &me, "the agent reports the worker's answer", |s| {
        s.last_turn
            .as_ref()
            .and_then(|t| t.reply.as_deref())
            .is_some_and(|r| r.starts_with("calc said"))
    })
    .await;
    assert_eq!(
        snap.last_turn.unwrap().reply.as_deref(),
        Some("calc said: The answer is 42.")
    );
    let all = c.workstreams().await.unwrap();
    assert_eq!(all.len(), 1);
    assert!(all[0].node("calc").unwrap().spawned_by.is_some());
}

/// A spawn gate raised from a turn (the agent's `spawn_agent` call blocks it) is withdrawn when
/// the user interrupts that turn: the call fails "withdrawn", the gate leaves the session with
/// `gate_resolved` reason `turn_interrupted`, a late "allow" is refused with the typed
/// `gate_withdrawn` error, and nothing is launched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_gate_is_withdrawn_when_the_asking_turn_is_interrupted() {
    let d = TempDir::new();
    let c = hub_client(&d.path().join("l"));
    let hub = HostId::new("hub");
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    set_host_layers(&c, d.path(), &[&hub]).await;
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(
            d.path(),
            &Scenario::builder()
                .turn(Turn::builder().say("thinking").wait_for_interrupt())
                .build(),
        ),
        ..OverrideLayer::default()
    };
    let solo = c.launch(req).await.unwrap();
    let me = NodeSession {
        host: hub.clone(),
        session: solo.id.clone(),
    };
    let rt = c.runtime(None);
    wait_snap(&c, &me, "idle", |s| s.phase.name() == "idle").await;
    rt.command(&solo.id, Command::prompt("go")).await.unwrap();
    wait_snap(&c, &me, "turn running", |s| s.turn.is_some()).await;

    let mut pending = spawn_bg(
        &c,
        &me,
        "w1",
        json!({"name": "scout", "role": "looks around"}),
    );
    let gate = gate_on(&c, &me, &mut pending).await;
    rt.command(&solo.id, Command::Interrupt).await.unwrap();
    let e = tokio::time::timeout(Duration::from_secs(120), pending)
        .await
        .expect("spawn answered")
        .unwrap()
        .expect_err("withdrawn");
    assert!(e.contains("withdrawn"), "{e}");
    wait_snap(&c, &me, "gate gone", |s| spawn_gate_of(s).is_none()).await;
    let history = rt.history(&solo.id, None, 1000).await.unwrap();
    assert!(history.events.iter().any(|ev| matches!(
        &ev.kind,
        marsad_proto::runtime::SessionEventKind::Agent {
            event: marsad_proto::AgentEvent::GateResolved { request_id, reason }
        } if request_id == gate.request_id()
            && reason.as_ref().is_some_and(|r| r.code == "turn_interrupted")
    )));
    let late = rt
        .command(
            &solo.id,
            Command::AnswerGate {
                request_id: gate.request_id().into(),
                decision: SpawnDecision::AllowOnce.to_gate(),
            },
        )
        .await;
    assert!(
        matches!(
            &late,
            Err(wasit_runtime::ClientError::Command(
                wasit_runtime::wire::CommandError::GateWithdrawn { .. }
            ))
        ),
        "{late:?}"
    );
    assert!(c.workstreams().await.unwrap().is_empty());
    assert_eq!(rt.list().await.unwrap().len(), 1);
}

/// How a workstream's spawn requests ended is in its view, whoever asked and however it ended:
/// denied by the human, the asking turn interrupted, refused by the supervisor. Wasl's node
/// publishes each ending (`wasl.spawns`); the view lists the latest, newest first, and a hub
/// restart gets them back from the node's journal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_workstream_view_lists_how_spawn_requests_ended() {
    let d = TempDir::new();
    let c = hub_client(&d.path().join("l"));
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    set_host_layers(&c, d.path(), &[&HostId::new("hub")]).await;
    // An orchestrator whose turns run until interrupted.
    let mut l = NodeLaunch::new("claude");
    l.cwd = Some(work.to_string_lossy().into_owned());
    l.overrides = OverrideLayer {
        env: scenario_env(
            d.path(),
            &Scenario::builder()
                .turn(Turn::builder().say("thinking").wait_for_interrupt())
                .build(),
        ),
        ..OverrideLayer::default()
    };
    let ws = c
        .create_workstream(WorkstreamDraft {
            name: "Outcomes".into(),
            nodes: vec![WorkstreamNode::launched("orch", "orchestrator", l)],
            limits: Limits::default(),
            ..WorkstreamDraft::default()
        })
        .await
        .unwrap();
    let mut views = Views::new(&c);
    let launched = c.launch_workstream(&ws.id).await.unwrap();
    let orch = session_of(&launched.workstream, "orch");
    wait_snap(&c, &orch, "idle", |s| s.phase.name() == "idle").await;
    let rt = c.runtime(None);

    // 1. Denied by the human, with a reason.
    let mut pending = spawn_bg(
        &c,
        &orch,
        "d1",
        json!({"name": "scout", "role": "looks around"}),
    );
    let gate = gate_on(&c, &orch, &mut pending).await;
    answer(
        &c,
        &orch,
        gate.request_id(),
        SpawnDecision::Deny {
            reason: Some("do it yourself".into()),
        }
        .to_gate(),
    )
    .await;
    let e = pending.await.unwrap().expect_err("denied");
    assert!(e.contains("do it yourself"), "{e}");
    let v = views
        .until(&ws.id, "the denied request in the view", |v| {
            v.recent_spawns.iter().any(|o| o.request_id == "d1")
        })
        .await;
    assert!(v.pending_spawns.is_empty());
    let denied = &v.recent_spawns[0];
    assert_eq!(denied.end, End::new(EndCode::Denied));
    assert_eq!(
        (denied.node.as_str(), denied.name.as_str()),
        ("orch", "scout")
    );
    assert_eq!(denied.session, orch.session);
    assert_eq!(denied.host, orch.host);
    assert!(denied
        .detail
        .as_deref()
        .is_some_and(|t| t.contains("do it yourself")));

    // 2. The asking turn is interrupted while the gate waits.
    rt.command(&orch.session, Command::prompt("go"))
        .await
        .unwrap();
    wait_snap(&c, &orch, "turn running", |s| s.turn.is_some()).await;
    let mut pending = spawn_bg(&c, &orch, "i1", json!({"name": "helper", "role": "helps"}));
    gate_on(&c, &orch, &mut pending).await;
    rt.command(&orch.session, Command::Interrupt).await.unwrap();
    let e = pending.await.unwrap().expect_err("withdrawn");
    assert!(e.contains("interrupted"), "{e}");
    let v = views
        .until(&ws.id, "the interrupted request in the view", |v| {
            v.recent_spawns.iter().any(|o| o.request_id == "i1")
        })
        .await;
    assert!(v.pending_spawns.is_empty());
    let interrupted = &v.recent_spawns[0];
    assert_eq!(interrupted.end, End::new(EndCode::TurnInterrupted));
    assert_eq!(interrupted.name, "helper");
    assert!(interrupted
        .detail
        .as_deref()
        .is_some_and(|t| t.contains("interrupted")));

    // 3. The supervisor (the hub) refuses the plan: no gate was ever shown.
    let e = spawn_now(
        &c,
        &orch,
        "p1",
        json!({"name": "x", "role": "x", "host": "nowhere"}),
    )
    .await
    .expect_err("unknown host");
    assert!(e.contains("no host"), "{e}");
    let v = views
        .until(&ws.id, "the refused request in the view", |v| {
            v.recent_spawns.iter().any(|o| o.request_id == "p1")
        })
        .await;
    let refused = &v.recent_spawns[0];
    assert_eq!(
        refused.end,
        End::caused_by(EndCode::Refused, wasl_proto::codes::SPAWN_REFUSED)
    );
    assert!(refused
        .detail
        .as_deref()
        .is_some_and(|t| t.contains("no host")));
    // Newest first.
    let ids: Vec<&str> = v
        .recent_spawns
        .iter()
        .map(|o| o.request_id.as_str())
        .collect();
    assert_eq!(ids, ["p1", "i1", "d1"]);
    let before = v.recent_spawns.clone();

    // 4. A hub restart: the view lists the same endings again, rebuilt from the node's journal.
    stop_hub(&c).await;
    let mut views = Views::new(&c);
    let v = views
        .until(&ws.id, "the endings after the hub restarted", |v| {
            v.recent_spawns.len() == 3
        })
        .await;
    assert_eq!(v.recent_spawns, before);
}
