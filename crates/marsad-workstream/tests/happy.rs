//! B14 `marsad-workstream` — happy paths: serde round-trips + derivation nominal cases.
//!
//! NOTE for the reviewer: this file uses `serde_json` (`to_string` / `from_str`) to
//! exercise the exact wire forms called out in the spec (§1). `serde_json` must be
//! added as a dev-dependency of `marsad-workstream` — see tests/DEV_DEPENDENCIES.txt.
//!
//! Spec: docs/specs/workstream.md — §3 test plan, cases T1-T13.

use marsad_workstream::{
    conns_of, dependency_depth, driver_dots, head_state, host_chips, is_single_node, nodes_of,
    state_counts, Connection, Driver, Handoff, HeadState, Node, Phase, Profile,
    StateCounts, Workstream,
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

// ── T1: Driver serde + as_str + color ───────────────────────────────────────

#[test]
fn driver_serde_round_trips_claude() {
    let json = serde_json::to_string(&Driver::Claude).unwrap();
    assert_eq!(json, "\"claude\"");
    let back: Driver = serde_json::from_str(&json).unwrap();
    assert_eq!(back, Driver::Claude);
}

#[test]
fn driver_serde_round_trips_codex() {
    let json = serde_json::to_string(&Driver::Codex).unwrap();
    assert_eq!(json, "\"codex\"");
    let back: Driver = serde_json::from_str(&json).unwrap();
    assert_eq!(back, Driver::Codex);
}

#[test]
fn driver_serde_round_trips_opencode() {
    let json = serde_json::to_string(&Driver::Opencode).unwrap();
    assert_eq!(json, "\"opencode\"");
    let back: Driver = serde_json::from_str(&json).unwrap();
    assert_eq!(back, Driver::Opencode);
}

#[test]
fn driver_as_str_all_variants() {
    assert_eq!(Driver::Claude.as_str(), "claude");
    assert_eq!(Driver::Codex.as_str(), "codex");
    assert_eq!(Driver::Opencode.as_str(), "opencode");
}

#[test]
fn driver_color_all_variants() {
    assert_eq!(Driver::Claude.color(), "#d97757");
    assert_eq!(Driver::Codex.color(), "#3fb6a8");
    assert_eq!(Driver::Opencode.color(), "#f0a050");
}

// ── T2: HeadState serde ─────────────────────────────────────────────────────
// (ConnKind removed — connections carry only `bi` direction, no data/command/both.)

#[test]
fn headstate_serde_round_trips_lowercase() {
    let cases = [
        (HeadState::Needs, "\"needs\""),
        (HeadState::Error, "\"error\""),
        (HeadState::Working, "\"working\""),
        (HeadState::Done, "\"done\""),
    ];
    for (val, expected_json) in cases {
        let json = serde_json::to_string(&val).unwrap();
        assert_eq!(json, expected_json, "serde form for {val:?}");
        let back: HeadState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, val);
    }
}

// ── T3: Workstream profile default/round-trip ───────────────────────────────

#[test]
fn workstream_profile_none_omits_or_defaults() {
    let ws = Workstream {
        id: "w1".to_string(),
        name: "Workstream One".to_string(),
        goal: "ship it".to_string(),
        profile: None,
    };
    let json = serde_json::to_string(&ws).unwrap();
    // Deserializing back from the produced JSON must recover profile: None.
    let back: Workstream = serde_json::from_str(&json).unwrap();
    assert_eq!(back.profile, None);
    assert_eq!(back, ws);

    // #[serde(default)] means a payload omitting `profile` entirely also deserializes fine.
    let no_field_json = r#"{"id":"w1","name":"Workstream One","goal":"ship it"}"#;
    let back2: Workstream = serde_json::from_str(no_field_json).unwrap();
    assert_eq!(back2.profile, None);
    assert_eq!(back2, ws);
}

#[test]
fn workstream_profile_some_round_trips() {
    let ws = Workstream {
        id: "w2".to_string(),
        name: "Workstream Two".to_string(),
        goal: "ship it too".to_string(),
        profile: Some("p1".to_string()),
    };
    let json = serde_json::to_string(&ws).unwrap();
    let back: Workstream = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ws);
    assert_eq!(back.profile, Some("p1".to_string()));
}

// ── T4: Node round-trip: lead default false, Phase value, context_pct bounds ─

#[test]
fn node_round_trip_lead_defaults_false() {
    let n = node("n1", "w1", Driver::Claude, "h1", Phase::Working);
    let json = serde_json::to_string(&n).unwrap();
    let back: Node = serde_json::from_str(&json).unwrap();
    assert_eq!(back, n);
    assert!(!back.lead);

    // #[serde(default)] means a payload omitting `lead` also deserializes with false.
    let no_lead_json = r#"{"id":"n1","ws":"w1","name":"node-n1","driver":"claude","host":"h1","state":"Working","branch":"main","context_pct":0,"status":"ok"}"#;
    let parsed: Result<Node, _> = serde_json::from_str(no_lead_json);
    assert!(
        parsed.is_ok(),
        "Node must deserialize with lead omitted: {parsed:?}"
    );
    assert!(!parsed.unwrap().lead);
}

#[test]
fn node_round_trip_context_pct_zero() {
    let mut n = node("n2", "w1", Driver::Codex, "h1", Phase::Done);
    n.context_pct = 0;
    let json = serde_json::to_string(&n).unwrap();
    let back: Node = serde_json::from_str(&json).unwrap();
    assert_eq!(back.context_pct, 0);
    assert_eq!(back, n);
}

#[test]
fn node_round_trip_context_pct_hundred() {
    let mut n = node("n3", "w1", Driver::Opencode, "h1", Phase::Needs);
    n.context_pct = 100;
    let json = serde_json::to_string(&n).unwrap();
    let back: Node = serde_json::from_str(&json).unwrap();
    assert_eq!(back.context_pct, 100);
    assert_eq!(back, n);
}

#[test]
fn node_round_trip_carries_lead_true_and_phase() {
    let mut n = node("n4", "w1", Driver::Claude, "h1", Phase::Error);
    n.lead = true;
    let json = serde_json::to_string(&n).unwrap();
    let back: Node = serde_json::from_str(&json).unwrap();
    assert_eq!(back, n);
    assert!(back.lead);
    assert_eq!(back.state, Phase::Error);
}

// ── T5: Connection round-trip: bi default false, gate None default, Some(Handoff) ─

#[test]
fn connection_round_trip_bi_and_gate_default() {
    let c = conn("c1", "w1", "a", "b");
    let json = serde_json::to_string(&c).unwrap();
    let back: Connection = serde_json::from_str(&json).unwrap();
    assert_eq!(back, c);
    assert!(!back.bi);
    assert_eq!(back.gate, None);

    // #[serde(default)] on both fields: a minimal payload omitting them still works.
    let minimal_json = r#"{"id":"c1","ws":"w1","from":"a","to":"b","kind":"data"}"#;
    let parsed: Result<Connection, _> = serde_json::from_str(minimal_json);
    assert!(
        parsed.is_ok(),
        "Connection must deserialize with bi/gate omitted: {parsed:?}"
    );
    let parsed = parsed.unwrap();
    assert!(!parsed.bi);
    assert_eq!(parsed.gate, None);
}

#[test]
fn connection_round_trip_with_gate_some() {
    let mut c = conn("c2", "w1", "a", "b");
    c.bi = true;
    c.gate = Some(Handoff {
        payload: "design doc".to_string(),
        risk: "medium".to_string(),
    });
    let json = serde_json::to_string(&c).unwrap();
    let back: Connection = serde_json::from_str(&json).unwrap();
    assert_eq!(back, c);
    assert!(back.bi);
    assert_eq!(
        back.gate,
        Some(Handoff {
            payload: "design doc".to_string(),
            risk: "medium".to_string(),
        })
    );
}

#[test]
fn profile_round_trips() {
    let p = Profile {
        id: "p1".to_string(),
        name: "Team Alpha".to_string(),
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: Profile = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
}

// ── T6: head_state mixed sets ────────────────────────────────────────────────

#[test]
fn head_state_needs_wins_over_everything() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Done),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
        node("c", "w1", Driver::Claude, "h1", Phase::Needs),
        node("d", "w1", Driver::Claude, "h1", Phase::Error),
    ];
    assert_eq!(head_state(&nodes), Some(HeadState::Needs));
}

#[test]
fn head_state_error_wins_over_working_and_done() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Error),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
        node("c", "w1", Driver::Claude, "h1", Phase::Done),
    ];
    assert_eq!(head_state(&nodes), Some(HeadState::Error));
}

#[test]
fn head_state_working_wins_over_done() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Done),
    ];
    assert_eq!(head_state(&nodes), Some(HeadState::Working));
}

#[test]
fn head_state_only_done() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Done),
        node("b", "w1", Driver::Claude, "h1", Phase::Done),
    ];
    assert_eq!(head_state(&nodes), Some(HeadState::Done));
}

// ── T7: head_state Starting folds into Done (idle) ──────────────────────────

#[test]
fn head_state_starting_alone_is_done() {
    // Revamp: a launched-but-idle (Starting) session is idle → Done, not Working.
    let nodes = vec![node("a", "w1", Driver::Claude, "h1", Phase::Starting)];
    assert_eq!(head_state(&nodes), Some(HeadState::Done));
}

#[test]
fn head_state_starting_and_done_is_done() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Starting),
        node("b", "w1", Driver::Claude, "h1", Phase::Done),
    ];
    assert_eq!(head_state(&nodes), Some(HeadState::Done));
}

// ── T8: state_counts covers all buckets incl. Starting → done ───────────────

#[test]
fn state_counts_all_buckets_with_starting_folded_into_done() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Starting),
        node("c", "w1", Driver::Claude, "h1", Phase::Needs),
        node("d", "w1", Driver::Claude, "h1", Phase::Needs),
        node("e", "w1", Driver::Claude, "h1", Phase::Error),
        node("f", "w1", Driver::Claude, "h1", Phase::Done),
        node("g", "w1", Driver::Claude, "h1", Phase::Done),
        node("h", "w1", Driver::Claude, "h1", Phase::Done),
    ];
    let counts = state_counts(&nodes);
    // Starting (b) now folds into done: working=1, done=4.
    assert_eq!(
        counts,
        StateCounts {
            working: 1,
            needs: 2,
            error: 1,
            done: 4,
            paused: 0,
        }
    );
}

// ── T9: driver_dots dedup + first-appearance order ───────────────────────────

#[test]
fn driver_dots_dedup_first_appearance_order() {
    let nodes = vec![
        node("a", "w1", Driver::Opencode, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Working),
        node("c", "w1", Driver::Opencode, "h1", Phase::Working),
        node("d", "w1", Driver::Codex, "h1", Phase::Working),
    ];
    assert_eq!(
        driver_dots(&nodes),
        vec![Driver::Opencode, Driver::Claude, Driver::Codex]
    );
}

// ── T10: host_chips dedup + first-appearance order ───────────────────────────

#[test]
fn host_chips_dedup_first_appearance_order() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "host-b", Phase::Working),
        node("b", "w1", Driver::Claude, "host-a", Phase::Working),
        node("c", "w1", Driver::Claude, "host-b", Phase::Working),
        node("d", "w1", Driver::Claude, "host-c", Phase::Working),
    ];
    assert_eq!(
        host_chips(&nodes),
        vec![
            "host-b".to_string(),
            "host-a".to_string(),
            "host-c".to_string(),
        ]
    );
}

// ── T11: nodes_of / conns_of filter + order preserved, foreign ws excluded ──

#[test]
fn nodes_of_filters_and_preserves_order() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w2", Driver::Claude, "h1", Phase::Working),
        node("c", "w1", Driver::Claude, "h1", Phase::Done),
        node("d", "w3", Driver::Claude, "h1", Phase::Done),
    ];
    let filtered = nodes_of("w1", &nodes);
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].id, "a");
    assert_eq!(filtered[1].id, "c");
    assert!(filtered.iter().all(|n| n.ws == "w1"));
}

#[test]
fn conns_of_filters_and_preserves_order() {
    let conns = vec![
        conn("c1", "w1", "a", "b"),
        conn("c2", "w2", "x", "y"),
        conn("c3", "w1", "b", "c"),
    ];
    let filtered = conns_of("w1", &conns);
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].id, "c1");
    assert_eq!(filtered[1].id, "c3");
    assert!(filtered.iter().all(|c| c.ws == "w1"));
}

// ── T12: is_single_node ──────────────────────────────────────────────────────

#[test]
fn is_single_node_true_for_exactly_one() {
    let nodes = vec![node("a", "w1", Driver::Claude, "h1", Phase::Working)];
    assert!(is_single_node("w1", &nodes));
}

#[test]
fn is_single_node_false_for_two() {
    let nodes = vec![
        node("a", "w1", Driver::Claude, "h1", Phase::Working),
        node("b", "w1", Driver::Claude, "h1", Phase::Done),
    ];
    assert!(!is_single_node("w1", &nodes));
}

#[test]
fn is_single_node_false_for_zero() {
    let nodes = vec![node("a", "w2", Driver::Claude, "h1", Phase::Working)];
    assert!(!is_single_node("w1", &nodes));
}

// ── T13: dependency_depth linear chain + diamond ─────────────────────────────

#[test]
fn dependency_depth_linear_chain() {
    let conns = vec![
        conn("c1", "w1", "a", "b"),
        conn("c2", "w1", "b", "c"),
    ];
    assert_eq!(dependency_depth("a", "w1", &conns), 0);
    assert_eq!(dependency_depth("b", "w1", &conns), 1);
    assert_eq!(dependency_depth("c", "w1", &conns), 2);
}

#[test]
fn dependency_depth_diamond() {
    // a -> b, a -> c, b -> d, c -> d
    let conns = vec![
        conn("c1", "w1", "a", "b"),
        conn("c2", "w1", "a", "c"),
        conn("c3", "w1", "b", "d"),
        conn("c4", "w1", "c", "d"),
    ];
    assert_eq!(dependency_depth("a", "w1", &conns), 0);
    assert_eq!(dependency_depth("b", "w1", &conns), 1);
    assert_eq!(dependency_depth("c", "w1", &conns), 1);
    assert_eq!(dependency_depth("d", "w1", &conns), 2);
}

/// Two-axis severity is LIVENESS-FIRST: a not-live (Paused) node is `Paused`
/// regardless of its activity phase — even Done. Pausing kills the process, so a
/// finished-but-paused session shows PAUSED (grouped separately), not Done. A
/// live node always outranks a paused one in an aggregate card.
#[test]
fn head_state_not_live_nodes_are_paused_regardless_of_phase() {
    // Live Working → Working.
    let live_working = node("a", "ws", Driver::Claude, "h", Phase::Working);
    assert_eq!(head_state(&[live_working]), Some(HeadState::Working));

    // Not-live Working (host-killed mid-work) → Paused.
    let mut dead_working = node("b", "ws", Driver::Claude, "h", Phase::Working);
    dead_working.live = false;
    assert_eq!(
        head_state(std::slice::from_ref(&dead_working)),
        Some(HeadState::Paused)
    );

    // Not-live Done → Paused (finished, but not running → grouped as Paused).
    let mut dead_done = node("c", "ws", Driver::Claude, "h", Phase::Done);
    dead_done.live = false;
    assert_eq!(head_state(&[dead_done]), Some(HeadState::Paused));

    // Mixed ws: a paused node + a live Needs → the live Needs wins.
    let live_needs = node("d", "ws", Driver::Claude, "h", Phase::Needs);
    assert_eq!(head_state(&[dead_working, live_needs]), Some(HeadState::Needs));

    // state_counts: the not-live node counts as `paused`, not its phase.
    let live_needs2 = node("e", "ws", Driver::Claude, "h", Phase::Needs);
    let mut dead_working2 = node("f", "ws", Driver::Claude, "h", Phase::Working);
    dead_working2.live = false;
    let counts = state_counts(&[live_needs2, dead_working2]);
    assert_eq!(counts.needs, 1);
    assert_eq!(counts.working, 0, "not-live working node must not count as working");
    assert_eq!(counts.paused, 1, "not-live node counts as paused");
}

/// LIVENESS-FIRST: pausing kills the process, so a paused session's pending gate
/// (Needs) or error is WASTED — unanswerable until a cold resume. It must NOT
/// surface as Needs/Error; it shows Paused and groups separately (core also
/// clears the gate on pause). A LIVE Needs/Error still surfaces as attention.
#[test]
fn head_state_paused_overrides_needs_and_error() {
    let mut dead_needs = node("a", "ws", Driver::Claude, "h", Phase::Needs);
    dead_needs.live = false;
    assert_eq!(head_state(&[dead_needs.clone()]), Some(HeadState::Paused));

    let mut dead_error = node("b", "ws", Driver::Claude, "h", Phase::Error);
    dead_error.live = false;
    assert_eq!(head_state(&[dead_error]), Some(HeadState::Paused));

    // They count as paused, not needs/error.
    let counts = state_counts(&[dead_needs]);
    assert_eq!(counts.paused, 1);
    assert_eq!(counts.needs, 0, "a paused gate is not a live 'needs'");

    // But a LIVE Needs still surfaces as attention (unchanged).
    let live_needs = node("z", "ws", Driver::Claude, "h", Phase::Needs);
    assert_eq!(head_state(&[live_needs]), Some(HeadState::Needs));
}
