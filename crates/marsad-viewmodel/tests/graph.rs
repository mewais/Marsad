//! graph.rs — tests for the B11 additive `graph_view` extension.
//!
//! These tests WILL fail to compile until the code-agent implements `graph_view`,
//! `GraphView`, `GraphNode`, `GraphEdge`, `EdgeStatus`, `Inspector`, and
//! `HandoffSummary` in `marsad-viewmodel`. That is the expected tests-first signal.
//!
//! Spec: docs/specs/viewmodel-ext.md — §Graph, test plan cases GV1-GV7.

use marsad_viewmodel::{graph_view, EdgeStatus};
use marsad_workstream::{Connection, Driver, Handoff, Node, Phase, Workstream};

// ── helpers ──────────────────────────────────────────────────────────────────

fn ws(id: &str, name: &str, goal: &str) -> Workstream {
    Workstream {
        id: id.to_string(),
        name: name.to_string(),
        goal: goal.to_string(),
        profile: None,
    }
}

fn node(id: &str, ws: &str, driver: Driver, host: &str, state: Phase) -> Node {
    Node {
        id: id.to_string(),
        ws: ws.to_string(),
        name: format!("node-{id}"),
        driver,
        live: true,
        end_reason: None,
        host: host.to_string(),
        state,
        lead: false,
        branch: "main".to_string(),
        context_pct: 42,
        status: "ok".to_string(),
    }
}

fn conn(id: &str, ws: &str, from: &str, to: &str) -> Connection {
    Connection {
        id: id.to_string(),
        ws: ws.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        bi: false,
        gate: None,
    }
}

// ── GV1: unknown ws_id → None ────────────────────────────────────────────────

#[test]
fn gv1_unknown_ws_id_returns_none() {
    let workstreams = vec![ws("w1", "One", "goal")];
    let nodes = vec![node("n1", "w1", Driver::Claude, "h1", Phase::Working)];
    let connections: Vec<Connection> = vec![];

    let result = graph_view("does-not-exist", &workstreams, &nodes, &connections);
    assert!(result.is_none());
}

// ── GV2: nodes mapped 1:1, depth on chain + diamond ─────────────────────────

#[test]
fn gv2_nodes_mapped_1_to_1_linear_chain_depths() {
    let workstreams = vec![ws("w1", "Chain", "goal")];
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Codex, "h2", Phase::Done),
        node("c", "w1", Driver::Gemini, "h3", Phase::Needs),
    ];
    let connections = vec![
        conn("c1", "w1", "a", "b"),
        conn("c2", "w1", "b", "c"),
    ];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();

    assert_eq!(graph.ws_id, "w1");
    assert_eq!(graph.name, "Chain");
    assert_eq!(graph.goal, "goal");
    assert_eq!(graph.nodes.len(), 3);

    let a = &graph.nodes[0];
    assert_eq!(a.id, "a");
    assert_eq!(a.name, "node-a");
    assert_eq!(a.driver, Driver::Claude);
    assert_eq!(a.host, "h1");
    assert_eq!(a.state, Phase::Working);
    assert_eq!(a.status, "ok");
    assert_eq!(a.context_pct, 42);
    assert!(!a.is_lead);
    assert_eq!(a.depth, 0);

    assert_eq!(graph.nodes[1].id, "b");
    assert_eq!(graph.nodes[1].depth, 1);

    assert_eq!(graph.nodes[2].id, "c");
    assert_eq!(graph.nodes[2].depth, 2);
}

#[test]
fn gv2_nodes_depth_on_diamond() {
    // a -> b, a -> c, b -> d, c -> d
    let workstreams = vec![ws("w1", "Diamond", "goal")];
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
        node("c", "w1", Driver::Claude, "h1", Phase::Working),
        node("d", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    let connections = vec![
        conn("c1", "w1", "a", "b"),
        conn("c2", "w1", "a", "c"),
        conn("c3", "w1", "b", "d"),
        conn("c4", "w1", "c", "d"),
    ];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();

    let depth_of = |id: &str| graph.nodes.iter().find(|n| n.id == id).unwrap().depth;
    assert_eq!(depth_of("a"), 0);
    assert_eq!(depth_of("b"), 1);
    assert_eq!(depth_of("c"), 1);
    assert_eq!(depth_of("d"), 2);
}

// ── GV3: edge status precedence ──────────────────────────────────────────────

#[test]
fn gv3_edge_status_plain_is_normal() {
    let workstreams = vec![ws("w1", "One", "goal")];
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    let connections = vec![conn("c1", "w1", "a", "b")];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();
    let edge = &graph.edges[0];
    assert_eq!(edge.status, EdgeStatus::Normal);
    assert!(!edge.has_gate);
}

#[test]
fn gv3_edge_status_bi_without_gate_is_consulting() {
    let workstreams = vec![ws("w1", "One", "goal")];
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    let mut c = conn("c1", "w1", "a", "b");
    c.bi = true;
    let connections = vec![c];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();
    let edge = &graph.edges[0];
    assert_eq!(edge.status, EdgeStatus::Consulting);
    assert!(!edge.has_gate);
}

#[test]
fn gv3_edge_status_gated_is_needs_gate_wins_over_bi() {
    let workstreams = vec![ws("w1", "One", "goal")];
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    let mut c = conn("c1", "w1", "a", "b");
    c.bi = true;
    c.gate = Some(Handoff {
        payload: "handoff payload".to_string(),
        risk: "high".to_string(),
    });
    let connections = vec![c];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();
    let edge = &graph.edges[0];
    assert_eq!(
        edge.status,
        EdgeStatus::Needs,
        "gate must win over bi for status precedence"
    );
    assert!(edge.has_gate);
}

#[test]
fn gv3_edge_has_gate_matches_gate_is_some_for_ungated() {
    let workstreams = vec![ws("w1", "One", "goal")];
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    let connections = vec![conn("c1", "w1", "a", "b")];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();
    assert!(!graph.edges[0].has_gate);
}

// (GV4 "kind passthrough" removed — connection kind no longer exists; edge
//  status precedence for directed/consult/gated is covered by GV3 above.)

// ── GV5: inspector counts + pending_handoff first gated ──────────────────────

#[test]
fn gv5_inspector_working_needs_counts_and_pending_handoff_first_gated() {
    let workstreams = vec![ws("w1", "One", "goal")];
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Starting),
        node("c", "w1", Driver::Claude, "h1", Phase::Needs),
        node("d", "w1", Driver::Claude, "h1", Phase::Done),
    ];
    let mut c1 = conn("c1", "w1", "a", "b");
    // not gated
    let mut c2 = conn("c2", "w1", "b", "c");
    c2.gate = Some(Handoff {
        payload: "first gated payload".to_string(),
        risk: "medium".to_string(),
    });
    let mut c3 = conn("c3", "w1", "c", "d");
    c3.gate = Some(Handoff {
        payload: "second gated payload".to_string(),
        risk: "high".to_string(),
    });
    c1.bi = false;
    let connections = vec![c1, c2, c3];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();

    // Starting is folded into Done (idle), so only `a` counts as working; `b`
    // (Starting) and `d` (Done) are both idle. needs = 1 (c).
    assert_eq!(graph.inspector.working, 1);
    assert_eq!(graph.inspector.needs, 1);

    let handoff = graph
        .inspector
        .pending_handoff
        .expect("expected a pending handoff");
    assert_eq!(
        handoff.conn_id, "c2",
        "first gated connection in conns_of order"
    );
    assert_eq!(handoff.from, "b");
    assert_eq!(handoff.to, "c");
    assert_eq!(handoff.payload, "first gated payload");
    assert_eq!(handoff.risk, "medium");
}

// ── GV6: pending_handoff None when no connection gated ──────────────────────

#[test]
fn gv6_pending_handoff_none_when_no_gate() {
    let workstreams = vec![ws("w1", "One", "goal")];
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Done),
    ];
    let connections = vec![conn("c1", "w1", "a", "b")];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();
    assert!(graph.inspector.pending_handoff.is_none());
}

#[test]
fn gv6_pending_handoff_none_when_no_connections_at_all() {
    let workstreams = vec![ws("w1", "One", "goal")];
    let nodes = vec![node("a", "w1", Driver::Claude, "h1", Phase::Working)];
    let connections: Vec<Connection> = vec![];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();
    assert!(graph.inspector.pending_handoff.is_none());
}

// ── GV7: single-node ws ───────────────────────────────────────────────────────

#[test]
fn gv7_single_node_ws_has_one_node_no_edges_correct_inspector() {
    let workstreams = vec![ws("w1", "Solo", "goal")];
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("z", "w2", Driver::Claude, "h1", Phase::Needs), // foreign ws, must be excluded
    ];
    let connections: Vec<Connection> = vec![];

    let graph = graph_view("w1", &workstreams, &nodes, &connections).unwrap();

    assert_eq!(graph.nodes.len(), 1);
    assert_eq!(graph.nodes[0].id, "a");
    assert_eq!(graph.nodes[0].depth, 0);
    assert!(graph.edges.is_empty());
    assert_eq!(graph.inspector.working, 1);
    assert_eq!(graph.inspector.needs, 0);
    assert!(graph.inspector.pending_handoff.is_none());
}
