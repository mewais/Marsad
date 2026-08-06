//! B14 `marsad-workstream` — error paths for `validate_connection` + `ConnError`.
//!
//! Covers the `validate_connection` precedence order (SelfLoop, then UnknownEndpoint,
//! CrossWorkstream, Duplicate, Cycle), direction-sensitivity of Duplicate, and the
//! `Display`/`Error` impls on `ConnError`.
//!
//! Spec: docs/specs/workstream.md — §3 test plan, cases E1-E7.

use std::error::Error as StdError;

use marsad_workstream::{
    validate_connection, ConnError, Connection, Driver, Node, Phase,
};

// ── helpers ──────────────────────────────────────────────────────────────────

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
        context_pct: 0,
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

// ── E1: self-loop wins even if the node is unknown ───────────────────────────

#[test]
fn self_loop_detected_even_when_node_unknown() {
    let nodes: Vec<Node> = vec![]; // "ghost" is not a known node at all
    let existing: Vec<Connection> = vec![];
    let proposed = conn("c1", "w1", "ghost", "ghost");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::SelfLoop)
    );
}

#[test]
fn self_loop_detected_for_known_node() {
    let nodes = vec![node("a", "w1", Driver::Claude, "h1", Phase::Working)];
    let existing: Vec<Connection> = vec![];
    let proposed = conn("c1", "w1", "a", "a");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::SelfLoop)
    );
}

// ── E2: unknown from/to → UnknownEndpoint ────────────────────────────────────

#[test]
fn unknown_from_endpoint() {
    let nodes = vec![node("b", "w1", Driver::Claude, "h1", Phase::Working)];
    let existing: Vec<Connection> = vec![];
    let proposed = conn("c1", "w1", "missing-a", "b");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::UnknownEndpoint)
    );
}

#[test]
fn unknown_to_endpoint() {
    let nodes = vec![node("a", "w1", Driver::Claude, "h1", Phase::Working)];
    let existing: Vec<Connection> = vec![];
    let proposed = conn("c1", "w1", "a", "missing-b");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::UnknownEndpoint)
    );
}

// ── E3: endpoint in another ws, or conn.ws disagrees → CrossWorkstream ───────

#[test]
fn endpoint_belongs_to_another_workstream() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w2", Driver::Claude, "h1", Phase::Working),
    ];
    let existing: Vec<Connection> = vec![];
    let proposed = conn("c1", "w1", "a", "b");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::CrossWorkstream)
    );
}

#[test]
fn conn_ws_disagrees_with_both_endpoints() {
    // Both endpoints exist and share a ws ("w1"), but conn.ws says "w9".
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    let existing: Vec<Connection> = vec![];
    let proposed = conn("c1", "w9", "a", "b");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::CrossWorkstream)
    );
}

// ── E4: missing-endpoint precedence over cross-workstream-ish situations ────

#[test]
fn missing_endpoint_takes_precedence_over_cross_workstream() {
    // `to` exists but in a different ws (w2); `from` doesn't exist at all.
    // Even though the situation "looks" cross-workstream-ish, the missing endpoint
    // must win and report UnknownEndpoint, not CrossWorkstream.
    let nodes = vec![node("b", "w2", Driver::Claude, "h1", Phase::Working)];
    let existing: Vec<Connection> = vec![];
    let proposed = conn("c1", "w1", "missing-a", "b");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::UnknownEndpoint)
    );
}

// ── E5: duplicate (from,to) in same ws; reversed (to,from) is not a dup ─────

#[test]
fn duplicate_from_to_in_same_ws_is_rejected() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    let existing = vec![conn("existing", "w1", "a", "b")];
    let proposed = conn("new", "w1", "a", "b");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::Duplicate)
    );
}

#[test]
fn reversed_direction_is_not_duplicate_but_closes_a_cycle() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    let existing = vec![conn("existing", "w1", "a", "b")];
    // (b, a) is the reverse of the existing (a, b): the direction-sensitive Duplicate
    // check does NOT fire (it's not the same (from,to) pair) — but the reverse edge
    // closes the 2-cycle a->b->a, so the Cycle check rejects it. Two-way links are
    // modeled as a single `bi` connection, never as two opposing edges. (spec §2.8/E5)
    let proposed = conn("new", "w1", "b", "a");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::Cycle)
    );
}

// ── E6: adding an edge that closes a directed cycle → Cycle ─────────────────

#[test]
fn edge_closing_a_directed_cycle_is_rejected() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
        node("c", "w1", Driver::Claude, "h1", Phase::Working),
    ];
    // a -> b -> c already present; proposing c -> a closes the cycle.
    let existing = vec![
        conn("c1", "w1", "a", "b"),
        conn("c2", "w1", "b", "c"),
    ];
    let proposed = conn("c3", "w1", "c", "a");
    assert_eq!(
        validate_connection(&proposed, &nodes, &existing),
        Err(ConnError::Cycle)
    );
}

// ── E7: ConnError Display is non-empty + distinct per variant; Error impl ───

#[test]
fn conn_error_display_is_nonempty_and_distinct_per_variant() {
    let variants = [
        ConnError::SelfLoop,
        ConnError::UnknownEndpoint,
        ConnError::CrossWorkstream,
        ConnError::Duplicate,
        ConnError::Cycle,
    ];

    let mut seen = Vec::new();
    for v in &variants {
        let s = v.to_string();
        assert!(
            !s.is_empty(),
            "Display for {v:?} must produce a non-empty string"
        );
        assert!(
            !seen.contains(&s),
            "Display for {v:?} produced a non-distinct string {s:?} (already seen: {seen:?})"
        );
        seen.push(s);
    }
    assert_eq!(seen.len(), variants.len());
}

#[test]
fn conn_error_implements_std_error() {
    // Exercise the std::error::Error trait object surface (source(), etc.) to prove
    // the impl is present and usable, not just declared.
    let err: Box<dyn StdError> = Box::new(ConnError::Cycle);
    assert!(err.source().is_none());
    assert_eq!(err.to_string(), ConnError::Cycle.to_string());
}
