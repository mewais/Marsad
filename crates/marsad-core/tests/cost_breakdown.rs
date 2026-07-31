//! cost_breakdown.rs — test suite for SLICE G's `Core::cost_breakdown` (the
//! Usage tab's "Cost breakdown — By host / By node" for a workstream's
//! sessions).
//!
//! Mirrors the harness in `tests/workstreams.rs` (`Core::with_deps` +
//! `LoopbackFake` + `MemoryBackend`) and the cost-line helpers in
//! `tests/u4_cost_truthfulness.rs` (`live_result_line` for a live,
//! agent-reported cost; `backfill_line` for a litellm/hand-table ESTIMATED
//! backfill cost).
//!
//! These tests WILL fail to compile until the code-agent implements
//! `Core::cost_breakdown` (delegating to
//! `marsad_viewmodel::workstream_cost_breakdown`). That is the expected
//! tests-first signal.

use marsad_core::{Core, CoreConfig, LaunchMode, LaunchSpec, };
use marsad_store::MemoryBackend;
use marsad_transport::LoopbackFake;

// ---------------------------------------------------------------------------
// Harness (mirrors tests/workstreams.rs / tests/u4_cost_truthfulness.rs)
// ---------------------------------------------------------------------------

fn core_with_backend(backend: MemoryBackend) -> Core {
    let (a, _b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-cost-breakdown-test".to_string(),
    };
    Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(marsad_core::RealTranscript),
        Box::new(backend),
    )
    .expect("with_deps")
}

fn launch(core: &mut Core, project: &str, host_id: &str) -> String {
    core.launch_session(LaunchSpec {
        project: project.to_string(),
        host_id: host_id.to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-sonnet-4-6".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: LaunchMode::Fresh,
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    })
    .expect("launch_session")
}

fn init_line(session_id: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-sonnet-4-6","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}}}}"#
    )
}

/// A LIVE, agent-reported `result` event — the only path that ever sets
/// `SessionView.cost_estimated = false` with a real cost.
fn live_result_line(session_id: &str, model: &str, input: u64, output: u64, cost: f64) -> String {
    serde_json::json!({
        "type": "AgentEvent",
        "session_id": session_id,
        "event": {
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "total_cost_usd": cost,
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            "num_turns": 1,
            "permission_denials": [],
            "modelUsage": {
                (model): {
                    "inputTokens": input,
                    "outputTokens": output,
                    "costUSD": cost,
                }
            }
        }
    })
    .to_string()
}

/// A `TranscriptBackfill` event — recomputes cost via the litellm/hand
/// pricing table (ESTIMATE-only; `cost_estimated = true` when no live result
/// has landed for this session).
fn backfill_line(session_id: &str, model: &str, input: u64, output: u64) -> String {
    let jsonl = serde_json::json!({
        "type": "assistant",
        "message": {
            "model": model,
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            }
        }
    })
    .to_string();
    serde_json::json!({
        "type": "TranscriptBackfill",
        "session_id": session_id,
        "jsonl": jsonl,
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// CBW1 — unknown ws_id -> None
// ---------------------------------------------------------------------------

#[test]
fn cbw1_unknown_ws_id_returns_none() {
    let core = core_with_backend(MemoryBackend::new());
    assert!(core.cost_breakdown("does-not-exist").is_none());
}

// ---------------------------------------------------------------------------
// CBW2 — groups by host/node, sums, sorts, and never lets an estimate clobber
// a live cost (and never mislabels a mixed group as "estimated").
// ---------------------------------------------------------------------------

#[test]
fn cbw2_groups_by_host_and_node_with_live_and_estimated_sessions() {
    let mut core = core_with_backend(MemoryBackend::new());

    let s_live = launch(&mut core, "proj-live", "host-a");
    let s_est = launch(&mut core, "proj-est", "host-a");
    let s_other_host = launch(&mut core, "proj-other", "host-b");

    core.create_workstream(
        "ws-cost".to_string(),
        "Cost WS".to_string(),
        String::new(),
        None,
    )
    .expect("create_workstream");
    core.assign_session(&s_live, "ws-cost", true)
        .expect("assign s_live");
    core.assign_session(&s_est, "ws-cost", false)
        .expect("assign s_est");
    core.assign_session(&s_other_host, "ws-cost", false)
        .expect("assign s_other_host");

    core.on_agent_line(&init_line(&s_live)).expect("init live");
    core.on_agent_line(&init_line(&s_est)).expect("init est");
    core.on_agent_line(&init_line(&s_other_host))
        .expect("init other host");

    // s_live: a LIVE, agent-reported $6.00 cost.
    core.on_agent_line(&live_result_line(
        &s_live,
        "claude-sonnet-4.6[1m]",
        10,
        20,
        6.0,
    ))
    .expect("live result");

    // s_est: ESTIMATE-only (backfill, no live result ever landed) — whatever
    // dollar amount the litellm/hand table computes; we only assert it's
    // nonzero and flagged estimated below, not the exact number (that table
    // is out of scope for this slice).
    core.on_agent_line(&backfill_line(
        &s_est,
        "claude-sonnet-4-6",
        900_000,
        900_000,
    ))
    .expect("backfill");

    // s_other_host: another LIVE cost, on a different host, $1.00.
    core.on_agent_line(&live_result_line(
        &s_other_host,
        "claude-sonnet-4.6[1m]",
        1,
        1,
        1.0,
    ))
    .expect("live result other host");

    let bd = core
        .cost_breakdown("ws-cost")
        .expect("ws-cost is a known workstream");

    // by_node: one row per session.
    assert_eq!(bd.by_node.len(), 3);
    let node_live = bd.by_node.iter().find(|r| r.key == s_live).unwrap();
    let node_est = bd.by_node.iter().find(|r| r.key == s_est).unwrap();
    let node_other = bd.by_node.iter().find(|r| r.key == s_other_host).unwrap();
    assert_eq!(node_live.cost_usd, 6.0);
    assert!(
        !node_live.estimated,
        "live cost must not be flagged estimated"
    );
    assert!(
        node_est.cost_usd > 0.0,
        "estimated backfill produced a nonzero cost"
    );
    assert!(node_est.estimated, "backfill-only session is estimated");
    assert_eq!(node_other.cost_usd, 1.0);
    assert!(!node_other.estimated);

    // by_host: host-a sums s_live + s_est (mixed -> NOT estimated, live cost
    // is never clobbered or dropped); host-b is just s_other_host.
    assert_eq!(bd.by_host.len(), 2);
    let host_a = bd.by_host.iter().find(|r| r.key == "host-a").unwrap();
    let host_b = bd.by_host.iter().find(|r| r.key == "host-b").unwrap();
    assert_eq!(
        host_a.cost_usd,
        6.0 + node_est.cost_usd,
        "host-a sums both its sessions' costs"
    );
    assert!(
        !host_a.estimated,
        "host-a has a live contributing session, so it must not be labelled estimated"
    );
    assert_eq!(host_b.cost_usd, 1.0);
    assert!(!host_b.estimated);

    // total_cost_usd is the sum of everything; total_estimated is false
    // because at least one session is live.
    assert_eq!(bd.total_cost_usd, 6.0 + node_est.cost_usd + 1.0);
    assert!(!bd.total_estimated);

    // Sorted by cost descending in both groupings.
    let node_costs: Vec<f64> = bd.by_node.iter().map(|r| r.cost_usd).collect();
    let mut sorted = node_costs.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());
    assert_eq!(
        node_costs, sorted,
        "by_node must be sorted by cost descending"
    );

    let host_costs: Vec<f64> = bd.by_host.iter().map(|r| r.cost_usd).collect();
    let mut sorted_hosts = host_costs.clone();
    sorted_hosts.sort_by(|a, b| b.partial_cmp(a).unwrap());
    assert_eq!(
        host_costs, sorted_hosts,
        "by_host must be sorted by cost descending"
    );

    // % shares sum to ~100 for both groupings.
    let node_pct_sum: f32 = bd.by_node.iter().map(|r| r.pct).sum();
    let host_pct_sum: f32 = bd.by_host.iter().map(|r| r.pct).sum();
    assert!((node_pct_sum - 100.0).abs() < 0.01);
    assert!((host_pct_sum - 100.0).abs() < 0.01);
}

// ---------------------------------------------------------------------------
// CBW3 — a session assigned to a DIFFERENT workstream is excluded entirely.
// ---------------------------------------------------------------------------

#[test]
fn cbw3_only_the_requested_workstreams_sessions_participate() {
    let mut core = core_with_backend(MemoryBackend::new());

    let s_in = launch(&mut core, "proj-in", "host-a");
    let s_out = launch(&mut core, "proj-out", "host-a");

    core.create_workstream("ws-a".to_string(), "A".to_string(), String::new(), None)
        .expect("create ws-a");
    core.create_workstream("ws-b".to_string(), "B".to_string(), String::new(), None)
        .expect("create ws-b");
    core.assign_session(&s_in, "ws-a", true)
        .expect("assign s_in");
    core.assign_session(&s_out, "ws-b", true)
        .expect("assign s_out");

    core.on_agent_line(&init_line(&s_in)).expect("init s_in");
    core.on_agent_line(&init_line(&s_out)).expect("init s_out");
    core.on_agent_line(&live_result_line(&s_in, "claude-sonnet-4.6[1m]", 1, 1, 2.0))
        .expect("live result s_in");
    core.on_agent_line(&live_result_line(
        &s_out,
        "claude-sonnet-4.6[1m]",
        1,
        1,
        99.0,
    ))
    .expect("live result s_out");

    let bd = core.cost_breakdown("ws-a").expect("ws-a exists");
    assert_eq!(bd.total_cost_usd, 2.0, "ws-b's session must be excluded");
    assert_eq!(bd.by_node.len(), 1);
    assert_eq!(bd.by_node[0].key, s_in);
}
