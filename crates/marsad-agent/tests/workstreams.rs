//! Workstreams end to end with real processes: the hub daemon, a second daemon behind a real
//! sshd on localhost, the embedded Wasl nodes and the fake agent CLI. Delegations are made the
//! way the Wasl MCP server makes them (`wasl.delegate` on the sending session's host); every
//! gate is answered through `session.command`, as a client would.
//! The ssh-host scenario needs a local sshd (Linux CI); the rest runs on every platform.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use common::*;
use marsad_client::{MarsadClient, Subscription};
use marsad_proto::agentconfig::{ItemState, LaunchRequest, OverrideLayer};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::hosts::HostDraft;
use marsad_proto::wasl::{
    DelegateReply, EdgePolicy, HandoffDecision, Limits, TaskBrief, TaskId, TaskState, TaskStates,
};
use marsad_proto::workstream::{
    AddNode, Connect, EdgeHistoryRequest, FromTemplate, NodeLaunch, NodeSession, SaveTemplate,
    WorkstreamDraft, WorkstreamEdge, WorkstreamNode, WorkstreamState, WorkstreamView,
    WorkstreamsEvent,
};
use marsad_proto::{
    Command, ConfigItemKind, GateChoiceKind, GateDecision, GateRequest, HostId, McpTransport,
};
use serde_json::json;
use wasit_fake_cli::scenario::{Scenario, Turn};

fn hub_client(dir: &Path) -> MarsadClient {
    let mut cfg =
        marsad_client::ClientConfig::new(dir, wasit_control::Hello::new("test", "gui", "gui"));
    cfg.agent_bin = Some(agent());
    cfg.serve_args = serve_args();
    cfg.supervise.backoff.initial = Duration::from_millis(50);
    cfg.supervise.backoff.max = Duration::from_millis(500);
    MarsadClient::connect(cfg)
}

/// A worker that answers `answer` after `ms` of work, for several turns.
fn slow_worker(answer: &str, ms: u64) -> Scenario {
    let mut b = Scenario::builder();
    for _ in 0..4 {
        b = b.turn(
            Turn::builder()
                .say("working on it")
                .sleep_ms(ms)
                .say(answer),
        );
    }
    b.build()
}

/// A worker whose turn needs a native permission first.
fn gated_worker(answer: &str) -> Scenario {
    Scenario::builder()
        .turn(
            Turn::builder()
                .say("let me check")
                .tool("Bash", json!({"command": "expr 2 + 2"}))
                .needs_permission()
                .result_if_allowed("4")
                .done()
                .say(answer),
        )
        .build()
}

fn node(dir: &Path, id: &str, host: Option<&HostId>, sc: &Scenario) -> WorkstreamNode {
    let mut l = NodeLaunch::new("claude");
    l.host = host.cloned();
    l.cwd = Some(dir.to_string_lossy().into_owned());
    l.overrides = OverrideLayer {
        env: scenario_env(dir, sc),
        ..OverrideLayer::default()
    };
    WorkstreamNode {
        id: id.into(),
        name: id.into(),
        role: String::new(),
        description: format!("the {id}"),
        session: None,
        launch: Some(l),
        spawned_by: None,
        spawn_request: None,
    }
}

fn edge(from: &str, to: &str, policy: EdgePolicy) -> WorkstreamEdge {
    WorkstreamEdge {
        from: from.into(),
        to: to.into(),
        policy,
    }
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
                    other => panic!("workstreams topic: {:?}", other.map(|r| r.map(|_| ()))),
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

fn session_of(v: &WorkstreamView, node: &str) -> NodeSession {
    v.workstream
        .node(node)
        .and_then(|n| n.session.clone())
        .unwrap_or_else(|| panic!("node {node} has no session"))
}

async fn delegate(
    c: &MarsadClient,
    from: &NodeSession,
    to: &[&str],
    instruction: &str,
) -> DelegateReply {
    let v = c
        .call_raw(
            Some(&from.host),
            "wasl.delegate",
            json!({"from": from.session, "to": to, "mode": "command", "instruction": instruction}),
        )
        .await
        .unwrap();
    serde_json::from_value(v).unwrap()
}

fn task(r: &DelegateReply, i: usize) -> TaskBrief {
    match (&r.results[i].task, &r.results[i].error) {
        (Some(t), _) => t.clone(),
        (None, e) => panic!("delegation rejected: {e:?}"),
    }
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
            // The hub may be restarting: the client reconnects and the call is retried.
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

fn summary(s: &TaskState) -> String {
    match s {
        TaskState::Completed { result } => result.summary.clone(),
        other => panic!("expected completed, got {other:?}"),
    }
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

fn kill_ssh(port: u16) {
    pkill(&format!("^ssh -T .*-p {port} "));
}

async fn stop_hub(c: &MarsadClient) {
    let pid = c.daemon_info(None).await.unwrap().pid;
    stop_pid(pid);
}

/// orchestrator (hub) → w1 (hub) and w2 (ssh host): launch, parallel delegation, handoff
/// gates in the fleet and workstreams topics answered via `session.command` (allow once,
/// always allow — persisted into the document and surviving a hub restart), the worker's
/// native gate, a link drop mid-task, and a hub restart mid-task with the topology re-pushed.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_nodes_across_local_and_ssh_host() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let c = hub_client(&d.path().join("l"));
    let hub = HostId::new("hub");
    let bx = HostId::new("box");
    let mut fleet = c.fleet(None);
    c.add_host(HostDraft {
        name: "Box".into(),
        kind: sshd.kind(&sshd.key),
        id: Some(bx.clone()),
    })
    .await
    .unwrap();
    let fut = async {
        loop {
            if let FleetEvent::Link { host, state } = next(&mut fleet).await.1 {
                if host == bx && state.is_connected() {
                    return;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .expect("box connected");

    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    let ws = c
        .create_workstream(WorkstreamDraft {
            name: "Cross host".into(),
            nodes: vec![
                node(&work, "orch", None, &Scenario::builder().build()),
                node(&work, "w1", None, &gated_worker("W1 done")),
                node(&work, "w2", Some(&bx), &slow_worker("W2 done", 2500)),
            ],
            edges: vec![
                edge("orch", "w1", EdgePolicy::Ask),
                edge("orch", "w2", EdgePolicy::Ask),
            ],
            limits: Limits::default(),
            ..WorkstreamDraft::default()
        })
        .await
        .unwrap();
    assert_eq!(ws.state, WorkstreamState::Draft);
    let mut views = Views::new(&c);
    let launched = c.launch_workstream(&ws.id).await.unwrap();
    assert_eq!(launched.launched.len(), 3);
    assert!(
        launched.launched.iter().all(|o| o.session.is_some()),
        "{launched:?}"
    );
    assert_eq!(launched.workstream.state, WorkstreamState::Running);
    let version = launched.workstream.topology_version;
    let v = views
        .until(
            &ws.id,
            "topology applied on both hosts, sessions live",
            |v| {
                v.sync.len() == 2
                    && v.sync.iter().all(|s| s.applied == version)
                    && v.nodes.iter().all(|n| n.session.is_some())
            },
        )
        .await;
    let (orch, w1, w2) = (
        session_of(&v, "orch"),
        session_of(&v, "w1"),
        session_of(&v, "w2"),
    );
    assert_eq!(w2.host, bx);
    let w1_snap = v.nodes.iter().find(|n| n.node == "w1").unwrap();
    assert_eq!(
        w1_snap
            .session
            .as_ref()
            .unwrap()
            .labels
            .get("marsad.node")
            .map(String::as_str),
        Some("w1")
    );

    // Parallel delegation: both handoffs wait for the human.
    let r = delegate(&c, &orch, &["w1", "w2"], "do your part").await;
    let (t1, t2) = (task(&r, 0), task(&r, 1));
    assert!(matches!(t1.state, TaskState::PendingApproval { .. }));
    assert!(matches!(t2.state, TaskState::PendingApproval { .. }));
    let v = views
        // The gates come with the orchestrator's session snapshot, the tasks with the task
        // feed: two feeds, so the view can show one before the other.
        .until(&ws.id, "two handoff gates, their tasks in flight", |v| {
            v.edges
                .iter()
                .all(|e| e.pending_gates.len() == 1 && e.in_flight.len() == 1)
        })
        .await;
    let gate_of = |to: &str| {
        let e = v.edges.iter().find(|e| e.to == to).unwrap();
        assert_eq!(e.in_flight.len(), 1);
        e.pending_gates[0].gate.request_id().to_string()
    };
    let (g1, g2) = (gate_of("w1"), gate_of("w2"));
    // The same gates are on the orchestrator's snapshot in the fleet.
    let fut = async {
        loop {
            if let FleetEvent::SessionUpserted { host, session } = next(&mut fleet).await.1 {
                let handoffs = session
                    .pending_gates
                    .iter()
                    .filter(|g| matches!(g.gate, GateRequest::Handoff { .. }))
                    .count();
                if host == hub && session.id == orch.session && handoffs == 2 {
                    assert_eq!(session.phase.name(), "needs_you");
                    return;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .expect("handoff gates in the fleet");
    answer(&c, &orch, &g1, HandoffDecision::AllowOnce.to_gate()).await;
    answer(&c, &orch, &g2, HandoffDecision::AlwaysAllow.to_gate()).await;

    // w1's own permission gate, answered like any gate.
    let v = views
        .until(&ws.id, "w1's native gate", |v| {
            v.nodes.iter().any(|n| {
                n.node == "w1"
                    && n.session.as_ref().is_some_and(|s| {
                        s.pending_gates
                            .iter()
                            .any(|g| matches!(g.gate, GateRequest::Permission { .. }))
                    })
            })
        })
        .await;
    let native = v.nodes.iter().find(|n| n.node == "w1").unwrap();
    let gate = native.session.as_ref().unwrap().pending_gates[0]
        .gate
        .clone();
    let decision = GateDecision::by_kind(&gate, &[GateChoiceKind::AllowOnce]).unwrap();
    answer(&c, &w1, gate.request_id(), decision).await;

    // w2 is working when its link drops; the result arrives after the reconnect.
    views
        .until(&ws.id, "w2 running", |v| {
            v.edges.iter().any(|e| {
                e.to == "w2"
                    && e.in_flight
                        .iter()
                        .any(|t| matches!(t.state, TaskState::Running { .. }))
            })
        })
        .await;
    kill_ssh(sshd.port);
    assert_eq!(summary(&wait_done(&c, &hub, &t1.id).await), "W1 done");
    assert_eq!(summary(&wait_done(&c, &hub, &t2.id).await), "W2 done");

    // "Always allow" went into the document (and a new topology version).
    let v = views
        .until(&ws.id, "always allow persisted, both results in", |v| {
            v.workstream.edge("orch", "w2").map(|e| e.policy) == Some(EdgePolicy::AlwaysAllow)
                && v.edges.iter().all(|e| e.completed == 1)
        })
        .await;
    assert_eq!(
        v.workstream.edge("orch", "w1").map(|e| e.policy),
        Some(EdgePolicy::Ask)
    );
    assert!(v.workstream.topology_version > version);

    // Hub restart mid-task: the task (no gate now) completes, the topology is re-pushed.
    let r = delegate(&c, &orch, &["w2"], "again").await;
    let t3 = task(&r, 0);
    assert!(!matches!(t3.state, TaskState::PendingApproval { .. }));
    views
        .until(&ws.id, "w2 running again", |v| {
            v.edges.iter().any(|e| {
                e.in_flight
                    .iter()
                    .any(|t| t.task.id == t3.id && matches!(t.state, TaskState::Running { .. }))
            })
        })
        .await;
    let old_pid = c.daemon_info(None).await.unwrap().pid;
    stop_hub(&c).await;
    assert_eq!(summary(&wait_done(&c, &hub, &t3.id).await), "W2 done");
    assert_ne!(c.daemon_info(None).await.unwrap().pid, old_pid, "a new hub");
    let doc = c.workstream(&ws.id).await.unwrap();
    assert_eq!(
        doc.edge("orch", "w2").map(|e| e.policy),
        Some(EdgePolicy::AlwaysAllow),
        "the decision survives the hub restart"
    );
    let mut views = Views::new(&c);
    views
        .until(&ws.id, "topology re-pushed after the restart", |v| {
            v.sync
                .iter()
                .all(|s| s.applied == doc.topology_version && s.error.is_none())
        })
        .await;
    // Still no gate on that connection.
    let r = delegate(&c, &orch, &["w2"], "third").await;
    assert!(!matches!(
        task(&r, 0).state,
        TaskState::PendingApproval { .. }
    ));
    assert_eq!(
        summary(&wait_done(&c, &hub, &task(&r, 0).id).await),
        "W2 done"
    );
    stop_hub(&c).await;
}

/// A running standalone session joins a workstream live and takes a delegation; blocked
/// connections and a paused workstream reject delegations; history pages; templates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_join_blocked_pause_history_and_templates() {
    let d = TempDir::new();
    let c = hub_client(&d.path().join("l"));
    let hub = HostId::new("hub");
    let work = d.path().join("w");
    std::fs::create_dir_all(&work).unwrap();

    // A plain session: the Wasl MCP server is injected automatically, locked and trusted.
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &slow_worker("standalone ok", 200)),
        ..OverrideLayer::default()
    };
    let solo = c.launch(req).await.unwrap();
    let item = solo
        .effective
        .item(ConfigItemKind::McpServer, "wasl")
        .expect("wasl item");
    assert_eq!(item.state, ItemState::Automatic);
    assert!(item.locked);
    let cfg = solo.effective.agent_config();
    let wasl = cfg.mcp_servers.iter().find(|m| m.name == "wasl").unwrap();
    assert!(wasl.trusted);
    match &wasl.transport {
        McpTransport::Stdio { command, args, .. } => {
            assert_eq!(Path::new(command), agent());
            assert_eq!(args[0], "mcp");
            assert!(args.contains(&solo.id.to_string()));
        }
        other => panic!("unexpected transport {other:?}"),
    }

    let ws = c
        .create_workstream(WorkstreamDraft {
            name: "Live".into(),
            nodes: vec![node(&work, "orch", None, &Scenario::builder().build())],
            ..WorkstreamDraft::default()
        })
        .await
        .unwrap();
    assert_eq!(ws.id, "live");
    let launched = c.launch_workstream(&ws.id).await.unwrap();
    let orch = launched.launched[0].session.clone().unwrap();

    // Join live, connect, delegate.
    let solo_ref = NodeSession {
        host: hub.clone(),
        session: solo.id.clone(),
    };
    c.add_node(AddNode {
        workstream: ws.id.clone(),
        node: WorkstreamNode {
            id: String::new(),
            name: "Helper".into(),
            role: "helper".into(),
            description: String::new(),
            session: Some(solo_ref.clone()),
            launch: None,
            spawned_by: None,
            spawn_request: None,
        },
        position: None,
    })
    .await
    .unwrap();
    let conn = |policy| Connect {
        workstream: ws.id.clone(),
        from: "orch".into(),
        to: "helper".into(),
        policy,
    };
    let mut views = Views::new(&c);
    let connected = c
        .connect_nodes(conn(EdgePolicy::AlwaysAllow))
        .await
        .unwrap();
    // The topology reaches the node asynchronously; delegate once it is in force.
    views
        .until(&ws.id, "connection in force", |v| {
            v.sync
                .iter()
                .any(|s| s.host == hub && s.applied >= connected.topology_version)
        })
        .await;
    let r = delegate(&c, &orch, &["Helper"], "help me").await;
    assert_eq!(
        summary(&wait_done(&c, &hub, &task(&r, 0).id).await),
        "standalone ok"
    );

    // Each change reaches the node asynchronously: act once it is in force.
    let in_force = |version: u64| {
        move |v: &WorkstreamView| {
            v.sync
                .iter()
                .any(|s| s.host == HostId::new("hub") && s.applied >= version)
        }
    };
    // Blocked connection.
    let w = c.set_edge_policy(conn(EdgePolicy::Deny)).await.unwrap();
    views
        .until(&ws.id, "deny in force", in_force(w.topology_version))
        .await;
    let r = delegate(&c, &orch, &["helper"], "again").await;
    assert_eq!(r.results[0].error.as_ref().unwrap().code, "denied");

    // Paused workstream.
    c.set_edge_policy(conn(EdgePolicy::AlwaysAllow))
        .await
        .unwrap();
    let p = c.pause_workstream(&ws.id).await.unwrap();
    assert_eq!(p.state, WorkstreamState::Paused);
    views
        .until(&ws.id, "pause in force", in_force(p.topology_version))
        .await;
    let r = delegate(&c, &orch, &["helper"], "while paused").await;
    assert_eq!(r.results[0].error.as_ref().unwrap().code, "paused");
    let resumed = c.resume_workstream(&ws.id).await.unwrap();
    views
        .until(
            &ws.id,
            "resume in force",
            in_force(resumed.topology_version),
        )
        .await;
    let r = delegate(&c, &orch, &["helper"], "resumed").await;
    let second = task(&r, 0);
    assert_eq!(
        summary(&wait_done(&c, &hub, &second.id).await),
        "standalone ok"
    );

    // History of the connection, newest first, paged.
    let h = |before_ms, limit| EdgeHistoryRequest {
        workstream: ws.id.clone(),
        from: "orch".into(),
        to: "helper".into(),
        before_ms,
        limit,
    };
    let all = c.edge_history(h(None, None)).await.unwrap();
    assert_eq!(all.tasks.len(), 2);
    assert_eq!(all.tasks[0].task.id, second.id);
    let p1 = c.edge_history(h(None, Some(1))).await.unwrap();
    assert_eq!(p1.tasks.len(), 1);
    assert!(p1.next_before.is_some());
    let mut views = Views::new(&c);
    let v = views
        .until(&ws.id, "two completed", |v| {
            v.edges.first().is_some_and(|e| e.completed == 2)
        })
        .await;
    assert!(v.cost.by_node.contains_key("helper") || v.nodes.len() == 2);

    // Templates: launch specs only (the joined session's from its snapshot), no sessions.
    let t = c
        .save_template(SaveTemplate {
            workstream: ws.id.clone(),
            name: Some("Pair".into()),
            description: None,
        })
        .await
        .unwrap();
    assert_eq!(t.nodes.len(), 2);
    assert!(t
        .nodes
        .iter()
        .all(|n| n.session.is_none() && n.launch.is_some()));
    assert_eq!(
        t.edges[0].policy,
        EdgePolicy::Ask,
        "decisions are not templated"
    );
    let copy = c
        .from_template(FromTemplate {
            template: t.id.clone(),
            name: None,
            hosts: BTreeMap::new(),
        })
        .await
        .unwrap();
    assert_eq!(copy.state, WorkstreamState::Draft);
    assert_eq!(copy.template.as_deref(), Some(t.id.as_str()));
    assert_eq!(copy.nodes.len(), 2);
    assert_eq!(c.templates().await.unwrap().len(), 1);
    c.delete_template(&t.id).await.unwrap();

    // Delete: the topology goes away on the node (the session keeps running).
    c.delete_workstream(&ws.id).await.unwrap();
    views
        .until(&copy.id, "the copy is still listed", |_| true)
        .await;
    assert!(c.workstream(&ws.id).await.is_err());
    let fut = async {
        let mut delay = Duration::from_millis(5);
        loop {
            let v = c
                .call_raw(None, "wasl.topology.get", json!(null))
                .await
                .unwrap();
            let list: marsad_proto::wasl::TopologyList = serde_json::from_value(v).unwrap();
            if list.workstreams.iter().all(|t| t.workstream != "live") {
                return;
            }
            // Backoff schedule: the removal is pushed asynchronously and emits no event.
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_millis(500));
        }
    };
    tokio::time::timeout(Duration::from_secs(30), fut)
        .await
        .expect("topology removed");
    stop_hub(&c).await;
}
