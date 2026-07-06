//! cost_breakdown.rs — tests for the SLICE G additive `workstream_cost_breakdown`
//! extension (Usage tab's "Cost breakdown — By host / By node").
//!
//! These tests WILL fail to compile until the code-agent implements
//! `workstream_cost_breakdown`, `WorkstreamCostBreakdown`, and `CostRow` in
//! `marsad-viewmodel`. That is the expected tests-first signal.
//!
//! Design source: docs/design/DESIGN_HANDOFF.md §Level 3 Usage tab ("Cost
//! breakdown — By host and By node, each with a stacked bar and per-row
//! entries: driver pill, cost, % share, sorted by cost") and the graph
//! inspector's "Cost" box in `docs/design/Marsad Workstreams.dc.html`
//! (`gWs.hostCosts` / `gWs.nodeCosts`).
//!
//! Hard rule (U4 cost truthfulness, mirrored one level up from
//! `SessionView.cost_estimated`): a group is `estimated` only when it has NO
//! live agent-reported cost — i.e. every contributing session is itself
//! `cost_estimated`. Agent-reported cost is truth; the litellm/hand table is
//! backfill-only and must never clobber it.

use marsad_events::Phase;
use marsad_viewmodel::{workstream_cost_breakdown, SessionView};
use marsad_workstream::{Driver, Node, Workstream};

// ── helpers ──────────────────────────────────────────────────────────────────

fn ws(id: &str) -> Workstream {
    Workstream {
        id: id.to_string(),
        name: format!("ws-{id}"),
        goal: String::new(),
        profile: None,
    }
}

fn node(id: &str, ws: &str, driver: Driver, host: &str) -> Node {
    Node {
        id: id.to_string(),
        ws: ws.to_string(),
        name: format!("node-{id}"),
        driver,
        host: host.to_string(),
        state: Phase::Working,
        lead: false,
        branch: "main".to_string(),
        context_pct: 0,
        status: "ok".to_string(),
    }
}

/// A minimal `SessionView` literal with a caller-chosen id/cost/estimated
/// flag; every other field takes an inert default.
fn sv(id: &str, host_id: &str, cost_usd: f64, cost_estimated: bool) -> SessionView {
    SessionView {
        id: id.to_string(),
        name: format!("node-{id}"),
        host_id: host_id.to_string(),
        project: "/p".to_string(),
        branch: "main".to_string(),
        model: "m".to_string(),
        state: Phase::Working,
        changed_files: 0,
        subagents: 0,
        running_subagents: 0,
        cost_usd,
        tokens: 0,
        context_pct: 0.0,
        gate: None,
        transcript: vec![],
        changes: vec![],
        model_usage: vec![],
        cost_estimated,
        compactions: 0,
        last_compaction: None,
        rate_limit: None,
        tools: vec![],
        mcp_servers: vec![],
        slash_commands: vec![],
        posture: "ask".to_string(),
        subagent_list: vec![],
        shell_running: false,
        driver: "claude".to_string(),
    }
}

fn pct_sum(rows: &[marsad_viewmodel::CostRow]) -> f32 {
    rows.iter().map(|r| r.pct).sum()
}

// ── CB1: unknown ws_id → None ────────────────────────────────────────────────

#[test]
fn cb1_unknown_ws_id_returns_none() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![node("n1", "w1", Driver::Claude, "h1")];
    let sessions = vec![sv("n1", "h1", 1.0, false)];

    let result = workstream_cost_breakdown("does-not-exist", &workstreams, &nodes, &sessions);
    assert!(result.is_none());
}

// ── CB2: grouping by host sums multiple nodes on the same host ──────────────

#[test]
fn cb2_by_host_sums_nodes_on_the_same_host() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1"),
        node("n2", "w1", Driver::Claude, "h1"),
        node("n3", "w1", Driver::Claude, "h2"),
    ];
    let sessions = vec![
        sv("n1", "h1", 2.0, false),
        sv("n2", "h1", 3.0, false),
        sv("n3", "h2", 5.0, false),
    ];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();

    assert_eq!(bd.total_cost_usd, 10.0);
    assert_eq!(bd.by_host.len(), 2, "one row per distinct host");
    let h1 = bd.by_host.iter().find(|r| r.key == "h1").unwrap();
    let h2 = bd.by_host.iter().find(|r| r.key == "h2").unwrap();
    assert_eq!(h1.cost_usd, 5.0, "n1 + n2 summed on h1");
    assert_eq!(h2.cost_usd, 5.0);
}

// ── CB3: grouping by node/session — one row per node, no summing ───────────

#[test]
fn cb3_by_node_one_row_per_session_no_summing() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1"),
        node("n2", "w1", Driver::Claude, "h1"),
    ];
    let sessions = vec![sv("n1", "h1", 2.0, false), sv("n2", "h1", 3.0, false)];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();

    assert_eq!(bd.by_node.len(), 2);
    let n1 = bd.by_node.iter().find(|r| r.key == "n1").unwrap();
    let n2 = bd.by_node.iter().find(|r| r.key == "n2").unwrap();
    assert_eq!(n1.cost_usd, 2.0);
    assert_eq!(n2.cost_usd, 3.0);
    assert_eq!(n1.label, "node-n1");
}

// ── CB4: % shares sum to 100 for both groupings ──────────────────────────────

#[test]
fn cb4_pct_shares_sum_to_100_by_host_and_by_node() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1"),
        node("n2", "w1", Driver::Claude, "h2"),
        node("n3", "w1", Driver::Claude, "h3"),
    ];
    let sessions = vec![
        sv("n1", "h1", 1.0, false),
        sv("n2", "h2", 2.0, false),
        sv("n3", "h3", 7.0, false),
    ];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();

    assert!(
        (pct_sum(&bd.by_host) - 100.0).abs() < 0.01,
        "by_host pct shares must sum to ~100, got {}",
        pct_sum(&bd.by_host)
    );
    assert!(
        (pct_sum(&bd.by_node) - 100.0).abs() < 0.01,
        "by_node pct shares must sum to ~100, got {}",
        pct_sum(&bd.by_node)
    );
}

#[test]
fn cb4_pct_is_zero_for_all_rows_when_total_cost_is_zero() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![node("n1", "w1", Driver::Claude, "h1")];
    let sessions = vec![sv("n1", "h1", 0.0, false)];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();

    assert_eq!(bd.total_cost_usd, 0.0);
    assert_eq!(bd.by_host[0].pct, 0.0, "no NaN/divide-by-zero");
    assert_eq!(bd.by_node[0].pct, 0.0);
}

// ── CB5: estimated-flag logic ────────────────────────────────────────────────

#[test]
fn cb5_by_node_estimated_true_when_session_is_estimated() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![node("n1", "w1", Driver::Claude, "h1")];
    let sessions = vec![sv("n1", "h1", 4.0, true)];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();
    assert!(bd.by_node[0].estimated);
    assert!(bd.total_estimated);
}

#[test]
fn cb5_by_host_estimated_true_only_when_all_contributing_sessions_are_estimated() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1"),
        node("n2", "w1", Driver::Claude, "h1"),
    ];
    // Both sessions on h1 are estimated-only.
    let sessions = vec![sv("n1", "h1", 1.0, true), sv("n2", "h1", 2.0, true)];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();
    let h1 = bd.by_host.iter().find(|r| r.key == "h1").unwrap();
    assert!(
        h1.estimated,
        "all contributing rows estimated -> group estimated"
    );
    assert!(bd.total_estimated);
}

// ── CB6: live cost is never clobbered — a mixed group is NOT estimated ──────

#[test]
fn cb6_by_host_not_estimated_when_any_contributing_session_is_live() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1"),
        node("n2", "w1", Driver::Claude, "h1"),
    ];
    // n1 is a LIVE agent-reported cost; n2 is estimated-only backfill.
    let sessions = vec![sv("n1", "h1", 5.0, false), sv("n2", "h1", 2.0, true)];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();
    let h1 = bd.by_host.iter().find(|r| r.key == "h1").unwrap();

    assert_eq!(
        h1.cost_usd, 7.0,
        "the live cost is summed in, never dropped or overridden"
    );
    assert!(
        !h1.estimated,
        "a group with ANY live contributing session must not be labelled estimated"
    );
    assert!(!bd.total_estimated);
}

#[test]
fn cb6_by_node_live_row_keeps_its_own_cost_and_estimated_false() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![node("n1", "w1", Driver::Claude, "h1")];
    let sessions = vec![sv("n1", "h1", 5.0, false)];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();
    assert_eq!(bd.by_node[0].cost_usd, 5.0);
    assert!(!bd.by_node[0].estimated);
}

// ── CB7: sorted by cost (descending) ─────────────────────────────────────────

#[test]
fn cb7_by_host_and_by_node_sorted_by_cost_descending() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h-small"),
        node("n2", "w1", Driver::Claude, "h-big"),
        node("n3", "w1", Driver::Claude, "h-mid"),
    ];
    let sessions = vec![
        sv("n1", "h-small", 1.0, false),
        sv("n2", "h-big", 9.0, false),
        sv("n3", "h-mid", 4.0, false),
    ];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();

    let host_costs: Vec<f64> = bd.by_host.iter().map(|r| r.cost_usd).collect();
    assert_eq!(
        host_costs,
        vec![9.0, 4.0, 1.0],
        "by_host sorted highest cost first"
    );

    let node_costs: Vec<f64> = bd.by_node.iter().map(|r| r.cost_usd).collect();
    assert_eq!(
        node_costs,
        vec![9.0, 4.0, 1.0],
        "by_node sorted highest cost first"
    );
}

// ── CB8: driver_dots per row (dedup, first-appearance, like driver_dots()) ──

#[test]
fn cb8_by_host_driver_dots_dedup_first_appearance() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![
        node("n1", "w1", Driver::Gemini, "h1"),
        node("n2", "w1", Driver::Claude, "h1"),
        node("n3", "w1", Driver::Gemini, "h1"),
    ];
    let sessions = vec![
        sv("n1", "h1", 1.0, false),
        sv("n2", "h1", 1.0, false),
        sv("n3", "h1", 1.0, false),
    ];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();
    let h1 = bd.by_host.iter().find(|r| r.key == "h1").unwrap();
    assert_eq!(h1.driver_dots, vec![Driver::Gemini, Driver::Claude]);
}

#[test]
fn cb8_by_node_driver_dots_is_the_single_node_driver() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![node("n1", "w1", Driver::Codex, "h1")];
    let sessions = vec![sv("n1", "h1", 1.0, false)];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();
    assert_eq!(bd.by_node[0].driver_dots, vec![Driver::Codex]);
}

// ── CB9: a node with no matching SessionView contributes zero, not-estimated ─

#[test]
fn cb9_node_with_no_matching_session_view_contributes_zero_not_estimated() {
    let workstreams = vec![ws("w1")];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1"),
        node("n2", "w1", Driver::Claude, "h1"), // no matching SessionView below
    ];
    let sessions = vec![sv("n1", "h1", 3.0, true)];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();

    let n2 = bd.by_node.iter().find(|r| r.key == "n2").unwrap();
    assert_eq!(n2.cost_usd, 0.0);
    assert!(
        !n2.estimated,
        "no cost data at all is not 'estimated', it's just empty (mirrors make_session_view)"
    );

    let h1 = bd.by_host.iter().find(|r| r.key == "h1").unwrap();
    assert_eq!(h1.cost_usd, 3.0);
    assert!(
        !h1.estimated,
        "the group has a non-estimated (empty) contributor, so it is not all-estimated"
    );
}

// ── CB10: only the requested workstream's nodes participate ────────────────

#[test]
fn cb10_only_the_requested_workstreams_nodes_participate() {
    let workstreams = vec![ws("w1"), ws("w2")];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1"),
        node("n2", "w2", Driver::Claude, "h1"),
    ];
    let sessions = vec![sv("n1", "h1", 1.0, false), sv("n2", "h1", 99.0, false)];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();

    assert_eq!(bd.total_cost_usd, 1.0, "w2's node/session must be excluded");
    assert_eq!(bd.by_node.len(), 1);
    assert_eq!(bd.by_node[0].key, "n1");
}

// ── CB11: empty ws (no nodes) → empty breakdown, no panic ───────────────────

#[test]
fn cb11_workstream_with_no_nodes_produces_empty_breakdown_no_panic() {
    let workstreams = vec![ws("w1")];
    let nodes: Vec<Node> = vec![];
    let sessions: Vec<SessionView> = vec![];

    let bd = workstream_cost_breakdown("w1", &workstreams, &nodes, &sessions).unwrap();

    assert_eq!(bd.total_cost_usd, 0.0);
    assert!(
        !bd.total_estimated,
        "no contributing rows -> not estimated (vacuous truth avoided)"
    );
    assert!(bd.by_host.is_empty());
    assert!(bd.by_node.is_empty());
}
