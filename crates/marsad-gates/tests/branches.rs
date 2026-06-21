//! branches.rs — table-driven cases closing remaining regions to 100%.
//!
//! Covers:
//! - decision_id STABILITY: same (session_id, gate) -> equal id; different session -> distinct id;
//!   different gate content -> distinct id  (all asserted explicitly)
//! - Each Gate variant exercised in isolation
//! - Empty reject reason
//! - DecisionLedger::seen true/false
//! - Approve and Reject for all three gate types in table-driven form

use marsad_gates::{
    approve, decision_id, reject, Decision, DecisionLedger, DiffLine, Gate, GateError, PlanStep,
    Sig,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn permission_gate(command: &str) -> Gate {
    Gate::Permission {
        command: command.to_string(),
        cwd: "/tmp".to_string(),
        risk: "low".to_string(),
        explanation: "test".to_string(),
    }
}

fn plan_gate(summary: &str) -> Gate {
    Gate::Plan {
        summary: summary.to_string(),
        steps: vec![PlanStep {
            title: "Step".to_string(),
            detail: "detail".to_string(),
        }],
    }
}

fn edit_gate(file: &str) -> Gate {
    Gate::Edit {
        file: file.to_string(),
        diff: vec![DiffLine {
            sig: Sig::Add,
            text: "new line".to_string(),
            ln: "1".to_string(),
        }],
    }
}

// ---------------------------------------------------------------------------
// decision_id STABILITY: same inputs -> equal id
// ---------------------------------------------------------------------------

#[test]
fn decision_id_stable_same_session_same_permission_gate() {
    let gate = permission_gate("cat /etc/passwd");
    let id1 = decision_id("session-stable", &gate);
    let id2 = decision_id("session-stable", &gate);
    assert_eq!(
        id1, id2,
        "decision_id must be deterministic for the same (session_id, gate)"
    );
}

#[test]
fn decision_id_stable_same_session_same_plan_gate() {
    let gate = plan_gate("Refactor auth");
    let id1 = decision_id("session-stable", &gate);
    let id2 = decision_id("session-stable", &gate);
    assert_eq!(
        id1, id2,
        "decision_id must be deterministic for the same (session_id, gate)"
    );
}

#[test]
fn decision_id_stable_same_session_same_edit_gate() {
    let gate = edit_gate("src/lib.rs");
    let id1 = decision_id("session-stable", &gate);
    let id2 = decision_id("session-stable", &gate);
    assert_eq!(
        id1, id2,
        "decision_id must be deterministic for the same (session_id, gate)"
    );
}

// ---------------------------------------------------------------------------
// decision_id STABILITY: different session -> distinct id (same gate)
// ---------------------------------------------------------------------------

#[test]
fn decision_id_distinct_different_session_permission() {
    let gate = permission_gate("ls");
    let id1 = decision_id("session-X", &gate);
    let id2 = decision_id("session-Y", &gate);
    assert_ne!(
        id1, id2,
        "different session_id must produce distinct decision_id"
    );
}

#[test]
fn decision_id_distinct_different_session_plan() {
    let gate = plan_gate("Deploy plan");
    let id1 = decision_id("session-1", &gate);
    let id2 = decision_id("session-2", &gate);
    assert_ne!(
        id1, id2,
        "different session_id must produce distinct decision_id"
    );
}

#[test]
fn decision_id_distinct_different_session_edit() {
    let gate = edit_gate("main.rs");
    let id1 = decision_id("session-alpha", &gate);
    let id2 = decision_id("session-beta", &gate);
    assert_ne!(
        id1, id2,
        "different session_id must produce distinct decision_id"
    );
}

// ---------------------------------------------------------------------------
// decision_id STABILITY: different gate content -> distinct id (same session)
// ---------------------------------------------------------------------------

#[test]
fn decision_id_distinct_different_permission_command() {
    let gate1 = permission_gate("ls /");
    let gate2 = permission_gate("rm /tmp/x");
    let id1 = decision_id("session-same", &gate1);
    let id2 = decision_id("session-same", &gate2);
    assert_ne!(
        id1, id2,
        "different gate content must produce distinct decision_id"
    );
}

#[test]
fn decision_id_distinct_different_plan_summary() {
    let gate1 = plan_gate("Plan A");
    let gate2 = plan_gate("Plan B");
    let id1 = decision_id("session-same", &gate1);
    let id2 = decision_id("session-same", &gate2);
    assert_ne!(
        id1, id2,
        "different plan summary must produce distinct decision_id"
    );
}

#[test]
fn decision_id_distinct_different_edit_file() {
    let gate1 = edit_gate("src/main.rs");
    let gate2 = edit_gate("src/lib.rs");
    let id1 = decision_id("session-same", &gate1);
    let id2 = decision_id("session-same", &gate2);
    assert_ne!(
        id1, id2,
        "different file path must produce distinct decision_id"
    );
}

#[test]
fn decision_id_distinct_different_gate_variants() {
    let perm = permission_gate("ls");
    let plan = plan_gate("ls"); // same "content" text but different variant
    let id1 = decision_id("session-same", &perm);
    let id2 = decision_id("session-same", &plan);
    assert_ne!(
        id1, id2,
        "different Gate variants must produce distinct decision_id"
    );
}

// ---------------------------------------------------------------------------
// Each Gate variant — approve/reject in isolation (table-driven style)
// ---------------------------------------------------------------------------

#[test]
fn each_gate_variant_approve_produces_correct_session_id() {
    let cases: Vec<(&str, Gate)> = vec![
        ("session-perm", permission_gate("pwd")),
        ("session-plan", plan_gate("Test plan")),
        ("session-edit", edit_gate("test.rs")),
    ];

    for (session, gate) in &cases {
        let d = approve(session, gate);
        assert_eq!(
            &d.session_id, session,
            "session_id mismatch for gate: {gate:?}"
        );
        assert_eq!(d.decision, Decision::Approve);
    }
}

#[test]
fn each_gate_variant_reject_produces_correct_decision() {
    let cases: Vec<(&str, Gate, &str)> = vec![
        ("session-perm", permission_gate("pwd"), "denied by policy"),
        ("session-plan", plan_gate("Test plan"), "plan rejected"),
        ("session-edit", edit_gate("test.rs"), "edit blocked"),
    ];

    for (session, gate, reason) in &cases {
        let d = reject(session, gate, reason);
        assert_eq!(&d.session_id, session);
        match &d.decision {
            Decision::Reject { reason: r } => assert_eq!(r, reason),
            Decision::Approve => panic!("expected Reject for gate: {gate:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Empty reject reason
// ---------------------------------------------------------------------------

#[test]
fn reject_with_empty_reason_permission() {
    let gate = permission_gate("echo hi");
    let d = reject("session-empty-reason", &gate, "");
    match &d.decision {
        Decision::Reject { reason } => assert_eq!(reason, "", "empty reason must be preserved"),
        Decision::Approve => panic!("expected Reject"),
    }
}

#[test]
fn reject_with_empty_reason_plan() {
    let gate = plan_gate("Deploy");
    let d = reject("session-empty-reason-plan", &gate, "");
    match &d.decision {
        Decision::Reject { reason } => assert_eq!(reason, ""),
        Decision::Approve => panic!("expected Reject"),
    }
}

#[test]
fn reject_with_empty_reason_edit() {
    let gate = edit_gate("config.rs");
    let d = reject("session-empty-reason-edit", &gate, "");
    match &d.decision {
        Decision::Reject { reason } => assert_eq!(reason, ""),
        Decision::Approve => panic!("expected Reject"),
    }
}

// ---------------------------------------------------------------------------
// DecisionLedger::seen — true/false across multiple sessions and gates
// ---------------------------------------------------------------------------

#[test]
fn ledger_seen_false_for_unrecorded_id() {
    let ledger = DecisionLedger::new();
    let gate = permission_gate("unrecorded");
    let id = decision_id("session-unrecorded", &gate);
    assert!(!ledger.seen(&id));
}

#[test]
fn ledger_seen_true_only_after_record() {
    let mut ledger = DecisionLedger::new();
    let gate = plan_gate("Seen plan");
    let d = approve("session-seen-check", &gate);
    assert!(!ledger.seen(&d.id), "should not be seen before record");
    ledger.record(&d).expect("record should succeed");
    assert!(ledger.seen(&d.id), "should be seen after record");
}

#[test]
fn ledger_seen_does_not_bleed_across_gate_types() {
    let mut ledger = DecisionLedger::new();
    let perm_gate = permission_gate("command");
    let plan_gate_inst = plan_gate("command"); // same text, different variant

    let d_perm = approve("session-bleed", &perm_gate);
    ledger.record(&d_perm).expect("record permission");

    let id_plan = decision_id("session-bleed", &plan_gate_inst);
    assert!(
        !ledger.seen(&id_plan),
        "recording a permission gate must not mark the plan gate id as seen"
    );
}

#[test]
fn ledger_seen_does_not_bleed_across_sessions() {
    let mut ledger = DecisionLedger::new();
    let gate = edit_gate("shared.rs");

    let d_a = approve("session-A-bleed", &gate);
    ledger.record(&d_a).expect("record for session A");

    let id_b = decision_id("session-B-bleed", &gate);
    assert!(
        !ledger.seen(&id_b),
        "recording for session A must not mark session B's id as seen"
    );
}

// ---------------------------------------------------------------------------
// decision_id equality matches GateDecision.id stability (cross-check)
// ---------------------------------------------------------------------------

#[test]
fn decision_id_matches_gate_decision_id_field() {
    let gate = permission_gate("grep foo bar");
    let session = "session-crosscheck";
    let standalone_id = decision_id(session, &gate);
    let d_approve = approve(session, &gate);
    let d_reject = reject(session, &gate, "reason");

    assert_eq!(
        standalone_id, d_approve.id,
        "decision_id() must match approve().id"
    );
    assert_eq!(
        standalone_id, d_reject.id,
        "decision_id() must match reject().id"
    );
    assert_eq!(
        d_approve.id, d_reject.id,
        "approve and reject for same inputs share the same id"
    );
}

// ---------------------------------------------------------------------------
// Plan gate with empty steps list
// ---------------------------------------------------------------------------

#[test]
fn plan_gate_with_no_steps_approve() {
    let gate = Gate::Plan {
        summary: "Empty steps plan".to_string(),
        steps: vec![],
    };
    let d = approve("session-empty-steps", &gate);
    assert_eq!(d.decision, Decision::Approve);
}

#[test]
fn plan_gate_with_no_steps_decision_id_stable() {
    let gate = Gate::Plan {
        summary: "Empty steps plan".to_string(),
        steps: vec![],
    };
    let id1 = decision_id("session-empty-steps", &gate);
    let id2 = decision_id("session-empty-steps", &gate);
    assert_eq!(id1, id2);
}

// ---------------------------------------------------------------------------
// Edit gate with empty diff list
// ---------------------------------------------------------------------------

#[test]
fn edit_gate_with_empty_diff_approve() {
    let gate = Gate::Edit {
        file: "empty.rs".to_string(),
        diff: vec![],
    };
    let d = approve("session-empty-diff", &gate);
    assert_eq!(d.decision, Decision::Approve);
}

// ---------------------------------------------------------------------------
// Ledger idempotency: second record for approve and reject both return Duplicate
// ---------------------------------------------------------------------------

#[test]
fn ledger_idempotency_approve_then_approve_again() {
    let mut ledger = DecisionLedger::new();
    let gate = permission_gate("idempotent-cmd");
    let d = approve("session-idem", &gate);
    ledger.record(&d).expect("first record ok");
    let result = ledger.record(&d);
    match result {
        Err(GateError::DuplicateDecision(_)) => {}
        other => panic!("expected DuplicateDecision, got: {other:?}"),
    }
}

#[test]
fn ledger_idempotency_reject_then_reject_again() {
    let mut ledger = DecisionLedger::new();
    let gate = plan_gate("idem-plan");
    let d = reject("session-idem-plan", &gate, "rejected");
    ledger.record(&d).expect("first record ok");
    let result = ledger.record(&d);
    match result {
        Err(GateError::DuplicateDecision(_)) => {}
        other => panic!("expected DuplicateDecision, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// DecisionId derives Debug, Clone, PartialEq
// ---------------------------------------------------------------------------

#[test]
fn decision_id_implements_debug_clone_partialeq() {
    let gate = permission_gate("test-cmd");
    let id = decision_id("session-id-traits", &gate);
    let id2 = id.clone();
    assert_eq!(id, id2);
    let _dbg = format!("{id:?}");
}
