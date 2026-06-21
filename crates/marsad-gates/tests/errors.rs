//! errors.rs — every enumerated error/edge path.
//!
//! Covers:
//! - GateError::UnknownGate and GateError::ClosedGate reachable variants (constructed/matched)
//! - GateError::DuplicateDecision on second DecisionLedger::record call (idempotency)
//! - DecisionLedger::seen false before record, true after

use marsad_gates::{
    approve, decision_id, reject, DecisionLedger, DiffLine, Gate, GateError, PlanStep, Sig,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_permission_gate() -> Gate {
    Gate::Permission {
        command: "ls /".to_string(),
        cwd: "/".to_string(),
        risk: "low".to_string(),
        explanation: "List root directory".to_string(),
    }
}

fn make_plan_gate() -> Gate {
    Gate::Plan {
        summary: "Build plan".to_string(),
        steps: vec![PlanStep {
            title: "Step A".to_string(),
            detail: "Do A".to_string(),
        }],
    }
}

fn make_edit_gate() -> Gate {
    Gate::Edit {
        file: "lib.rs".to_string(),
        diff: vec![DiffLine {
            sig: Sig::Add,
            text: "pub fn foo() {}".to_string(),
            ln: "10".to_string(),
        }],
    }
}

// ---------------------------------------------------------------------------
// GateError variants are constructable and matchable
// ---------------------------------------------------------------------------

#[test]
fn gate_error_unknown_gate_variant_exists() {
    let e: GateError = GateError::UnknownGate;
    match e {
        GateError::UnknownGate => {}
        _ => panic!("expected UnknownGate"),
    }
}

#[test]
fn gate_error_closed_gate_variant_exists() {
    let e: GateError = GateError::ClosedGate;
    match e {
        GateError::ClosedGate => {}
        _ => panic!("expected ClosedGate"),
    }
}

#[test]
fn gate_error_duplicate_decision_carries_decision_id() {
    let gate = make_permission_gate();
    let id = decision_id("session-dup", &gate);
    let e: GateError = GateError::DuplicateDecision(id.clone());
    match e {
        GateError::DuplicateDecision(inner) => assert_eq!(inner, id),
        _ => panic!("expected DuplicateDecision"),
    }
}

// ---------------------------------------------------------------------------
// GateError implements Debug and Clone (spec: derive Debug, Clone, PartialEq)
// ---------------------------------------------------------------------------

#[test]
fn gate_error_implements_debug() {
    let e = GateError::UnknownGate;
    let _s = format!("{e:?}");
    let e2 = GateError::ClosedGate;
    let _s2 = format!("{e2:?}");
    let gate = make_plan_gate();
    let id = decision_id("s", &gate);
    let e3 = GateError::DuplicateDecision(id);
    let _s3 = format!("{e3:?}");
}

#[test]
fn gate_error_implements_partialeq() {
    assert_eq!(GateError::UnknownGate, GateError::UnknownGate);
    assert_eq!(GateError::ClosedGate, GateError::ClosedGate);
    assert_ne!(GateError::UnknownGate, GateError::ClosedGate);

    let gate = make_permission_gate();
    let id = decision_id("s", &gate);
    let e1 = GateError::DuplicateDecision(id.clone());
    let e2 = GateError::DuplicateDecision(id);
    assert_eq!(e1, e2);
}

// ---------------------------------------------------------------------------
// DecisionLedger: first record returns Ok
// ---------------------------------------------------------------------------

#[test]
fn ledger_record_first_time_returns_ok_permission() {
    let mut ledger = DecisionLedger::new();
    let gate = make_permission_gate();
    let d = approve("session-ledger-perm", &gate);
    let result = ledger.record(&d);
    assert!(
        result.is_ok(),
        "first record should return Ok, got: {result:?}"
    );
}

#[test]
fn ledger_record_first_time_returns_ok_plan() {
    let mut ledger = DecisionLedger::new();
    let gate = make_plan_gate();
    let d = approve("session-ledger-plan", &gate);
    let result = ledger.record(&d);
    assert!(
        result.is_ok(),
        "first record should return Ok, got: {result:?}"
    );
}

#[test]
fn ledger_record_first_time_returns_ok_edit() {
    let mut ledger = DecisionLedger::new();
    let gate = make_edit_gate();
    let d = approve("session-ledger-edit", &gate);
    let result = ledger.record(&d);
    assert!(
        result.is_ok(),
        "first record should return Ok, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// DecisionLedger: second record returns DuplicateDecision (idempotency)
// ---------------------------------------------------------------------------

#[test]
fn ledger_record_second_time_returns_duplicate_decision_permission() {
    let mut ledger = DecisionLedger::new();
    let gate = make_permission_gate();
    let d = approve("session-dup-perm", &gate);
    ledger.record(&d).expect("first record should succeed");
    let result = ledger.record(&d);
    match result {
        Err(GateError::DuplicateDecision(id)) => {
            assert_eq!(id, d.id, "DuplicateDecision should carry the original id");
        }
        other => panic!("expected DuplicateDecision, got: {other:?}"),
    }
}

#[test]
fn ledger_record_second_time_returns_duplicate_decision_plan() {
    let mut ledger = DecisionLedger::new();
    let gate = make_plan_gate();
    let d = reject("session-dup-plan", &gate, "bad plan");
    ledger.record(&d).expect("first record should succeed");
    let result = ledger.record(&d);
    match result {
        Err(GateError::DuplicateDecision(id)) => {
            assert_eq!(id, d.id);
        }
        other => panic!("expected DuplicateDecision, got: {other:?}"),
    }
}

#[test]
fn ledger_record_second_time_returns_duplicate_decision_edit() {
    let mut ledger = DecisionLedger::new();
    let gate = make_edit_gate();
    let d = approve("session-dup-edit", &gate);
    ledger.record(&d).expect("first record should succeed");
    let result = ledger.record(&d);
    match result {
        Err(GateError::DuplicateDecision(id)) => {
            assert_eq!(id, d.id);
        }
        other => panic!("expected DuplicateDecision, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// DecisionLedger: seen returns false before record, true after
// ---------------------------------------------------------------------------

#[test]
fn ledger_seen_false_before_record() {
    let ledger = DecisionLedger::new();
    let gate = make_permission_gate();
    let id = decision_id("session-seen-false", &gate);
    assert!(!ledger.seen(&id), "seen should be false before any record");
}

#[test]
fn ledger_seen_true_after_record() {
    let mut ledger = DecisionLedger::new();
    let gate = make_permission_gate();
    let d = approve("session-seen-true", &gate);
    ledger.record(&d).expect("first record should succeed");
    assert!(ledger.seen(&d.id), "seen should be true after record");
}

#[test]
fn ledger_seen_false_for_different_session() {
    let mut ledger = DecisionLedger::new();
    let gate = make_permission_gate();
    let d1 = approve("session-A", &gate);
    ledger.record(&d1).expect("first record should succeed");
    // Different session -> different id -> should not be seen
    let d2 = approve("session-B", &gate);
    assert!(
        !ledger.seen(&d2.id),
        "seen should be false for a different session's id"
    );
}

// ---------------------------------------------------------------------------
// Multiple different decisions in one ledger — all succeed on first record
// ---------------------------------------------------------------------------

#[test]
fn ledger_accepts_multiple_different_decisions() {
    let mut ledger = DecisionLedger::new();

    let perm_gate = make_permission_gate();
    let plan_gate = make_plan_gate();
    let edit_gate = make_edit_gate();

    let d1 = approve("session-multi", &perm_gate);
    let d2 = approve("session-multi", &plan_gate);
    let d3 = approve("session-multi", &edit_gate);

    ledger.record(&d1).expect("first permission record");
    ledger.record(&d2).expect("first plan record");
    ledger.record(&d3).expect("first edit record");

    assert!(ledger.seen(&d1.id));
    assert!(ledger.seen(&d2.id));
    assert!(ledger.seen(&d3.id));
}

// ---------------------------------------------------------------------------
// DecisionLedger::new() creates an empty ledger
// ---------------------------------------------------------------------------

#[test]
fn ledger_new_creates_empty_ledger() {
    let ledger = DecisionLedger::new();
    // No ids should be seen yet; verify using an arbitrary gate
    let gate = make_permission_gate();
    let id = decision_id("any-session", &gate);
    assert!(!ledger.seen(&id));
}
