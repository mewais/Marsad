//! B14 `marsad-workstream` — branch/edge coverage.
//!
//! Covers: empty-input branches, dormant-state (Stale/Detached) exclusion,
//! cycle detection (empty/DAG/cycle), dependency_depth edge cases (no inbound,
//! absent node, node in a cycle), a valid new connection, and the `bi`-edge
//! directed-cycle-detection precise behavior.
//!
//! Spec: docs/specs/workstream.md — §3 test plan, cases B1-B8.

use marsad_workstream::{
    dependency_depth, driver_dots, head_state, host_chips, is_acyclic, state_counts,
    validate_connection, ConnKind, Connection, Driver, HeadState, Node, Phase, StateCounts,
};

// ── helpers ──────────────────────────────────────────────────────────────────

fn node(id: &str, ws: &str, driver: Driver, host: &str, state: Phase) -> Node {
    Node {
        id: id.to_string(),
        ws: ws.to_string(),
        name: format!("node-{id}"),
        driver,
        live: true,
        host: host.to_string(),
        state,
        lead: false,
        branch: "main".to_string(),
        context_pct: 0,
        status: "ok".to_string(),
    }
}

fn conn(id: &str, ws: &str, from: &str, to: &str, kind: ConnKind) -> Connection {
    Connection {
        id: id.to_string(),
        ws: ws.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        kind,
        bi: false,
        gate: None,
    }
}

// ── B1: head_state empty slice → None ────────────────────────────────────────

#[test]
fn head_state_empty_slice_is_none() {
    let nodes: Vec<Node> = vec![];
    assert_eq!(head_state(&nodes), None);
}

// ── B2: head_state all Stale/Detached → None; Needs among Stale → Some(Needs) ─

#[test]
fn head_state_all_dormant_is_none() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Stale),
        node("b", "w1", Driver::Claude, "h1", Phase::Detached),
        node("c", "w1", Driver::Claude, "h1", Phase::Stale),
    ];
    assert_eq!(head_state(&nodes), None);
}

#[test]
fn head_state_needs_among_dormant_is_some_needs() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Stale),
        node("b", "w1", Driver::Claude, "h1", Phase::Detached),
        node("c", "w1", Driver::Claude, "h1", Phase::Needs),
    ];
    assert_eq!(head_state(&nodes), Some(HeadState::Needs));
}

// ── B3: state_counts — Stale/Detached counted in no bucket ───────────────────

#[test]
fn state_counts_dormant_nodes_all_zero() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Stale),
        node("b", "w1", Driver::Claude, "h1", Phase::Detached),
        node("c", "w1", Driver::Claude, "h1", Phase::Stale),
    ];
    let counts = state_counts(&nodes);
    assert_eq!(counts, StateCounts::default());
    assert_eq!(
        counts,
        StateCounts {
            working: 0,
            needs: 0,
            error: 0,
            done: 0,
        }
    );
}

// ── B4: driver_dots / host_chips empty slice → empty vec ────────────────────

#[test]
fn driver_dots_empty_slice_is_empty_vec() {
    let nodes: Vec<Node> = vec![];
    assert!(driver_dots(&nodes).is_empty());
}

#[test]
fn host_chips_empty_slice_is_empty_vec() {
    let nodes: Vec<Node> = vec![];
    assert!(host_chips(&nodes).is_empty());
}

// ── B5: is_acyclic — empty true, DAG true, 3-cycle false ────────────────────

#[test]
fn is_acyclic_empty_edges_is_true() {
    let conns: Vec<Connection> = vec![];
    assert!(is_acyclic("w1", &conns));
}

#[test]
fn is_acyclic_dag_is_true() {
    let conns = vec![
        conn("c1", "w1", "a", "b", ConnKind::Data),
        conn("c2", "w1", "b", "c", ConnKind::Data),
        conn("c3", "w1", "a", "c", ConnKind::Data),
    ];
    assert!(is_acyclic("w1", &conns));
}

#[test]
fn is_acyclic_three_cycle_is_false() {
    // a -> b -> c -> a
    let conns = vec![
        conn("c1", "w1", "a", "b", ConnKind::Data),
        conn("c2", "w1", "b", "c", ConnKind::Data),
        conn("c3", "w1", "c", "a", ConnKind::Data),
    ];
    assert!(!is_acyclic("w1", &conns));
}

// ── B6: dependency_depth — no inbound, absent node, node in a cycle ─────────

#[test]
fn dependency_depth_no_inbound_is_zero() {
    let conns = vec![conn("c1", "w1", "a", "b", ConnKind::Data)];
    assert_eq!(dependency_depth("a", "w1", &conns), 0);
}

#[test]
fn dependency_depth_absent_node_is_zero() {
    let conns = vec![conn("c1", "w1", "a", "b", ConnKind::Data)];
    assert_eq!(dependency_depth("does-not-exist", "w1", &conns), 0);
}

#[test]
fn dependency_depth_node_in_cycle_is_zero() {
    // a -> b -> c -> a; querying any of them should be 0 per spec (caller validates acyclicity).
    let conns = vec![
        conn("c1", "w1", "a", "b", ConnKind::Data),
        conn("c2", "w1", "b", "c", ConnKind::Data),
        conn("c3", "w1", "c", "a", ConnKind::Data),
    ];
    assert_eq!(dependency_depth("a", "w1", &conns), 0);
    assert_eq!(dependency_depth("b", "w1", &conns), 0);
    assert_eq!(dependency_depth("c", "w1", &conns), 0);
}

// ── B7: validate_connection — valid new edge in a 2-node ws → Ok(()) ────────

#[test]
fn validate_connection_valid_new_edge_is_ok() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    let existing: Vec<Connection> = vec![];
    let proposed = conn("new", "w1", "a", "b", ConnKind::Data);
    assert_eq!(validate_connection(&proposed, &nodes, &existing), Ok(()));
}

// ── B8: bi edge treated as single directed edge for cycle detection ─────────

#[test]
fn bi_edge_is_directed_only_for_cycle_detection() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    // Existing: a -> b marked `bi` (still just one directed edge a->b for cycle purposes).
    let mut existing_edge = conn("c1", "w1", "a", "b", ConnKind::Data);
    existing_edge.bi = true;
    let existing = vec![existing_edge];

    // Proposing b -> a closes the cycle a->b->a, since the bi flag does NOT add a
    // reverse directed edge on its own — the *proposed* b->a is what completes it.
    let proposed = conn("c2", "w1", "b", "a", ConnKind::Data);
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(marsad_workstream::ConnError::Cycle)
    );
}
