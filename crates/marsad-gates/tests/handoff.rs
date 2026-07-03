//! handoff.rs — the additive `Gate::Handoff` variant (X1/X2, `docs/specs/
//! wasl-connector.md` §5.1: "B03 marsad-gates — the Gate::Handoff variant").
//!
//! Authoritative spec quote: "`gate_canonical` gains: `Gate::Handoff { gate_id,
//! .. } => format!("handoff\x00{session_id}\x00{gate_id}")` — identity =
//! (scope, gate_id) only; `kind/from/summary/preview` are display payload,
//! exactly like `Gate::Question`'s `request_id`-only identity." and "The
//! `session_id` slot carries the Wasl scope string `wasl:<host_id>`" (a
//! marsad-core concern — this crate stays agnostic of the string's shape, it
//! is just the `session_id` parameter `approve`/`reject`/`decision_id`
//! already take for every other `Gate` variant).
//!
//! Black-box only: `gate_canonical` itself is private, so identity is
//! verified through the public `decision_id`/`approve`/`reject` surface
//! (equal inputs -> equal id; distinct (session_id, gate_id) -> distinct id;
//! payload-only differences -> SAME id) — mirrors the house convention
//! already used for `Gate::Question` in `tests/branches.rs`
//! (`decision_id_question_gate_is_stable`) and `tests/happy.rs`.
//!
//! Per §5.1: "approve/reject/decision_id/DecisionLedger work unchanged on the
//! new variant" — no new gates-crate API, only a new `Gate` variant. This
//! file WILL fail to compile until the code-agent lands `Gate::Handoff` (§5.1).
//! That is the expected tests-first compile-failure signal.

use marsad_gates::{
    approve, decision_id, reject, route, to_control_response, Decision, DecisionLedger, Gate,
    GateError,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn handoff_gate(gate_id: &str) -> Gate {
    Gate::Handoff {
        gate_id: gate_id.to_string(),
        kind: "message".to_string(),
        from: "wasl-a".to_string(),
        summary: "message to 1 peer(s)".to_string(),
        preview: "hi bob".to_string(),
    }
}

fn handoff_gate_with_payload(
    gate_id: &str,
    kind: &str,
    from: &str,
    summary: &str,
    preview: &str,
) -> Gate {
    Gate::Handoff {
        gate_id: gate_id.to_string(),
        kind: kind.to_string(),
        from: from.to_string(),
        summary: summary.to_string(),
        preview: preview.to_string(),
    }
}

/// The Wasl scope string a real core builds: `wasl:<host_id>` (§5.1). This
/// crate is agnostic of the shape — any `&str` works as `session_id` here —
/// but using the real shape keeps the fixture honest.
fn wasl_scope(host_id: &str) -> String {
    format!("wasl:{host_id}")
}

// ---------------------------------------------------------------------------
// decision_id stability: same (scope, gate_id) -> equal id
// ---------------------------------------------------------------------------

#[test]
fn decision_id_stable_same_scope_same_gate_id() {
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("local");
    let id1 = decision_id(&scope, &gate);
    let id2 = decision_id(&scope, &gate);
    assert_eq!(
        id1, id2,
        "decision_id must be deterministic for the same (scope, gate_id)"
    );
}

// ---------------------------------------------------------------------------
// distinct gate_id -> distinct id (same scope)
// ---------------------------------------------------------------------------

#[test]
fn decision_id_distinct_different_gate_id_same_scope() {
    let scope = wasl_scope("local");
    let gate1 = handoff_gate("gate-1");
    let gate2 = handoff_gate("gate-2");
    let id1 = decision_id(&scope, &gate1);
    let id2 = decision_id(&scope, &gate2);
    assert_ne!(
        id1, id2,
        "different gate_id must produce distinct decision_id"
    );
}

// ---------------------------------------------------------------------------
// distinct scope -> distinct id (same gate_id)
// ---------------------------------------------------------------------------

#[test]
fn decision_id_distinct_different_scope_same_gate_id() {
    let gate = handoff_gate("gate-1");
    let id1 = decision_id(&wasl_scope("local"), &gate);
    let id2 = decision_id(&wasl_scope("hpc"), &gate);
    assert_ne!(
        id1, id2,
        "different Wasl scope (host_id) must produce distinct decision_id"
    );
}

// ---------------------------------------------------------------------------
// payload fields (kind/from/summary/preview) do NOT affect identity
// ---------------------------------------------------------------------------

#[test]
fn decision_id_unaffected_by_payload_fields() {
    let scope = wasl_scope("local");
    let gate_a = handoff_gate_with_payload("gate-1", "message", "wasl-a", "summary A", "preview A");
    let gate_b = handoff_gate_with_payload(
        "gate-1",
        "command",
        "wasl-b",
        "totally different summary",
        "totally different preview",
    );
    let id_a = decision_id(&scope, &gate_a);
    let id_b = decision_id(&scope, &gate_b);
    assert_eq!(
        id_a, id_b,
        "kind/from/summary/preview must NOT affect decision_id identity — \
         only (scope, gate_id) does, exactly like Gate::Question's request_id-only identity"
    );
}

/// Cross-check against a different `Gate` variant: distinct variants must
/// still produce distinct ids even when `gate_id`-shaped identity strings
/// coincide with another variant's identity field (variant-tag separation,
/// mirrors `decision_id_distinct_different_gate_variants` in
/// `marsad-gates/tests/branches.rs`).
#[test]
fn decision_id_distinct_from_question_gate_with_matching_identity_string() {
    let scope = "session-x".to_string();
    let handoff = handoff_gate("shared-id");
    let question = Gate::Question {
        request_id: "shared-id".to_string(),
        questions: vec![],
    };
    let id_handoff = decision_id(&scope, &handoff);
    let id_question = decision_id(&scope, &question);
    assert_ne!(
        id_handoff, id_question,
        "Gate::Handoff and Gate::Question must not collide even with the same identity string"
    );
}

// ---------------------------------------------------------------------------
// approve / reject / route on Handoff
// ---------------------------------------------------------------------------

#[test]
fn approve_handoff_gate_returns_approve_decision() {
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("local");
    let d = approve(&scope, &gate);
    assert_eq!(d.session_id, scope);
    assert_eq!(d.decision, Decision::Approve);
    assert!(!d.id.0.is_empty());
}

#[test]
fn reject_handoff_gate_returns_reject_decision_with_reason() {
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("local");
    let d = reject(&scope, &gate, "not during business hours");
    assert_eq!(d.session_id, scope);
    match &d.decision {
        Decision::Reject { reason } => assert_eq!(reason, "not during business hours"),
        Decision::Approve => panic!("expected Reject, got Approve"),
    }
}

#[test]
fn route_handoff_approve_returns_exact_scope() {
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("remote-host");
    let d = approve(&scope, &gate);
    assert_eq!(
        route(&d),
        "wasl:remote-host",
        "route() must return exactly the Wasl scope string (session_id slot), \
         preserving the invariant \"a decision routes to exactly one owner\""
    );
}

#[test]
fn route_handoff_reject_returns_exact_scope() {
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("local");
    let d = reject(&scope, &gate, "denied");
    assert_eq!(route(&d), "wasl:local");
}

// ---------------------------------------------------------------------------
// Handoff implements Clone/Debug/PartialEq (derived traits, matches every
// other Gate variant)
// ---------------------------------------------------------------------------

#[test]
fn gate_handoff_implements_clone_debug_partialeq() {
    let g = handoff_gate("gate-1");
    let g2 = g.clone();
    assert_eq!(g, g2);
    let _dbg = format!("{g:?}");
}

#[test]
fn gate_handoff_partialeq_distinguishes_payload_differences() {
    let g1 = handoff_gate_with_payload("gate-1", "message", "wasl-a", "s1", "p1");
    let g2 = handoff_gate_with_payload("gate-1", "message", "wasl-a", "s2", "p2");
    assert_ne!(
        g1, g2,
        "Gate equality (unlike decision_id) DOES see the full payload"
    );
}

// ---------------------------------------------------------------------------
// DecisionLedger: record -> seen -> DuplicateDecision on re-record
// ---------------------------------------------------------------------------

#[test]
fn ledger_record_first_time_ok_then_seen_true() {
    let mut ledger = DecisionLedger::new();
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("local");
    let d = approve(&scope, &gate);
    assert!(!ledger.seen(&d.id), "must not be seen before record");
    ledger.record(&d).expect("first record must succeed");
    assert!(ledger.seen(&d.id), "must be seen after record");
}

#[test]
fn ledger_record_second_time_returns_duplicate_decision() {
    let mut ledger = DecisionLedger::new();
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("local");
    let d = approve(&scope, &gate);
    ledger.record(&d).expect("first record must succeed");
    let result = ledger.record(&d);
    match result {
        Err(GateError::DuplicateDecision(id)) => assert_eq!(id, d.id),
        other => panic!("expected DuplicateDecision, got: {other:?}"),
    }
}

/// §5.1's own note (mirrored in wasl-connector.md §5.2): approve-then-deny of
/// the SAME gate is also deduped, since `decision_id` ignores the decision
/// value entirely — only (scope, gate_id) feeds identity.
#[test]
fn ledger_approve_then_reject_same_gate_shares_one_id_and_dedupes() {
    let mut ledger = DecisionLedger::new();
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("local");
    let d_approve = approve(&scope, &gate);
    let d_reject = reject(&scope, &gate, "changed my mind");
    assert_eq!(
        d_approve.id, d_reject.id,
        "approve and reject of the same (scope, gate_id) must share one decision id"
    );
    ledger
        .record(&d_approve)
        .expect("first record must succeed");
    let result = ledger.record(&d_reject);
    assert!(
        matches!(result, Err(GateError::DuplicateDecision(_))),
        "second decision on the same gate must be a duplicate, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// to_control_response still functions on a Handoff-derived decision
// ---------------------------------------------------------------------------

#[test]
fn to_control_response_handoff_approve_maps_to_allow_with_id_passthrough() {
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("local");
    let d = approve(&scope, &gate);
    let resp = to_control_response(&d);
    assert!(resp.allow);
    assert_eq!(resp.deny_reason, None);
    assert_eq!(
        resp.request_id, d.id.0,
        "to_control_response must pass the decision id through as request_id, \
         same as every other Gate variant"
    );
}

#[test]
fn to_control_response_handoff_reject_maps_to_deny_with_reason_and_id_passthrough() {
    let gate = handoff_gate("gate-1");
    let scope = wasl_scope("local");
    let d = reject(&scope, &gate, "not during business hours");
    let resp = to_control_response(&d);
    assert!(!resp.allow);
    assert_eq!(
        resp.deny_reason.as_deref(),
        Some("not during business hours")
    );
    assert_eq!(resp.request_id, d.id.0);
}

// ---------------------------------------------------------------------------
// Empty-payload edge cases (kind/from/summary/preview all empty strings)
// ---------------------------------------------------------------------------

#[test]
fn handoff_gate_with_all_empty_payload_fields_still_approves() {
    let gate = handoff_gate_with_payload("gate-empty", "", "", "", "");
    let scope = wasl_scope("local");
    let d = approve(&scope, &gate);
    assert_eq!(d.decision, Decision::Approve);
    assert!(!d.id.0.is_empty());
}
