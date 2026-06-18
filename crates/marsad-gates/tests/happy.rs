//! happy.rs — nominal paths for every public fn/variant.
//!
//! Covers:
//! - approve + reject for each Gate variant (Permission, Plan, Edit)
//! - to_control_response mapping: Approve -> allow behavior, Reject -> deny with reason as message
//! - route yields exactly the decision's session_id

use marsad_diff::{DiffLine, Sig};
use marsad_gates::{approve, reject, route, to_control_response, Decision, Gate, GateDecision, PlanStep};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_permission_gate() -> Gate {
    Gate::Permission {
        command: "rm -rf /tmp/test".to_string(),
        cwd: "/home/user/project".to_string(),
        risk: "high".to_string(),
        explanation: "Removes temporary files".to_string(),
    }
}

fn make_plan_gate() -> Gate {
    Gate::Plan {
        summary: "Refactor the auth module".to_string(),
        steps: vec![
            PlanStep {
                title: "Step 1".to_string(),
                detail: "Read existing code".to_string(),
            },
            PlanStep {
                title: "Step 2".to_string(),
                detail: "Write new code".to_string(),
            },
        ],
    }
}

fn make_edit_gate() -> Gate {
    Gate::Edit {
        file: "src/main.rs".to_string(),
        diff: vec![
            DiffLine { sig: Sig::Context, text: "fn main() {".to_string(), ln: "1".to_string() },
            DiffLine { sig: Sig::Del, text: "    println!(\"hello\");".to_string(), ln: "2".to_string() },
            DiffLine { sig: Sig::Add, text: "    println!(\"world\");".to_string(), ln: "2".to_string() },
            DiffLine { sig: Sig::Context, text: "}".to_string(), ln: "3".to_string() },
        ],
    }
}

// ---------------------------------------------------------------------------
// approve tests — Permission gate
// ---------------------------------------------------------------------------

#[test]
fn approve_permission_gate_returns_approve_decision() {
    let gate = make_permission_gate();
    let d = approve("session-abc", &gate);
    assert_eq!(d.session_id, "session-abc");
    assert_eq!(d.decision, Decision::Approve);
}

#[test]
fn approve_permission_gate_id_is_populated() {
    let gate = make_permission_gate();
    let d = approve("session-abc", &gate);
    assert!(!d.id.0.is_empty(), "DecisionId must not be empty");
}

// ---------------------------------------------------------------------------
// reject tests — Permission gate
// ---------------------------------------------------------------------------

#[test]
fn reject_permission_gate_returns_reject_decision() {
    let gate = make_permission_gate();
    let d = reject("session-abc", &gate, "too risky");
    assert_eq!(d.session_id, "session-abc");
    match &d.decision {
        Decision::Reject { reason } => assert_eq!(reason, "too risky"),
        Decision::Approve => panic!("expected Reject, got Approve"),
    }
}

// ---------------------------------------------------------------------------
// approve tests — Plan gate
// ---------------------------------------------------------------------------

#[test]
fn approve_plan_gate_returns_approve_decision() {
    let gate = make_plan_gate();
    let d = approve("session-plan", &gate);
    assert_eq!(d.session_id, "session-plan");
    assert_eq!(d.decision, Decision::Approve);
}

#[test]
fn approve_plan_gate_id_is_populated() {
    let gate = make_plan_gate();
    let d = approve("session-plan", &gate);
    assert!(!d.id.0.is_empty());
}

// ---------------------------------------------------------------------------
// reject tests — Plan gate
// ---------------------------------------------------------------------------

#[test]
fn reject_plan_gate_returns_reject_decision() {
    let gate = make_plan_gate();
    let d = reject("session-plan", &gate, "plan is incomplete");
    match &d.decision {
        Decision::Reject { reason } => assert_eq!(reason, "plan is incomplete"),
        Decision::Approve => panic!("expected Reject, got Approve"),
    }
}

// ---------------------------------------------------------------------------
// approve tests — Edit gate
// ---------------------------------------------------------------------------

#[test]
fn approve_edit_gate_returns_approve_decision() {
    let gate = make_edit_gate();
    let d = approve("session-edit", &gate);
    assert_eq!(d.session_id, "session-edit");
    assert_eq!(d.decision, Decision::Approve);
}

#[test]
fn approve_edit_gate_id_is_populated() {
    let gate = make_edit_gate();
    let d = approve("session-edit", &gate);
    assert!(!d.id.0.is_empty());
}

// ---------------------------------------------------------------------------
// reject tests — Edit gate
// ---------------------------------------------------------------------------

#[test]
fn reject_edit_gate_returns_reject_decision() {
    let gate = make_edit_gate();
    let d = reject("session-edit", &gate, "diff looks wrong");
    match &d.decision {
        Decision::Reject { reason } => assert_eq!(reason, "diff looks wrong"),
        Decision::Approve => panic!("expected Reject, got Approve"),
    }
}

// ---------------------------------------------------------------------------
// to_control_response — Approve -> allow behavior
// ---------------------------------------------------------------------------

#[test]
fn to_control_response_approve_permission_maps_to_allow() {
    let gate = make_permission_gate();
    let d = approve("session-abc", &gate);
    let resp = to_control_response(&d);
    // NOTE: marsad_proto::ControlResponse is the target type.
    // The spec says Approve maps to behavior:"allow".
    // We assert the returned value has allow behavior via Debug or the public field/method.
    // Adjust field access to match the actual marsad_proto::ControlResponse definition once
    // implemented. Using Debug here as a forward-compat assertion.
    let debug = format!("{:?}", resp);
    assert!(
        debug.to_lowercase().contains("allow"),
        "expected allow behavior in ControlResponse, got: {debug}"
    );
}

#[test]
fn to_control_response_approve_plan_maps_to_allow() {
    let gate = make_plan_gate();
    let d = approve("session-plan", &gate);
    let resp = to_control_response(&d);
    let debug = format!("{:?}", resp);
    assert!(
        debug.to_lowercase().contains("allow"),
        "expected allow behavior in ControlResponse, got: {debug}"
    );
}

#[test]
fn to_control_response_approve_edit_maps_to_allow() {
    let gate = make_edit_gate();
    let d = approve("session-edit", &gate);
    let resp = to_control_response(&d);
    let debug = format!("{:?}", resp);
    assert!(
        debug.to_lowercase().contains("allow"),
        "expected allow behavior in ControlResponse, got: {debug}"
    );
}

// ---------------------------------------------------------------------------
// to_control_response — Reject -> deny with reason as message
// ---------------------------------------------------------------------------

#[test]
fn to_control_response_reject_permission_maps_to_deny_with_reason() {
    let gate = make_permission_gate();
    let d = reject("session-abc", &gate, "blocked by policy");
    let resp = to_control_response(&d);
    let debug = format!("{:?}", resp);
    assert!(
        debug.to_lowercase().contains("deny"),
        "expected deny behavior in ControlResponse, got: {debug}"
    );
    assert!(
        debug.contains("blocked by policy"),
        "expected reject reason as message in ControlResponse, got: {debug}"
    );
}

#[test]
fn to_control_response_reject_plan_maps_to_deny_with_reason() {
    let gate = make_plan_gate();
    let d = reject("session-plan", &gate, "steps unclear");
    let resp = to_control_response(&d);
    let debug = format!("{:?}", resp);
    assert!(
        debug.to_lowercase().contains("deny"),
        "expected deny behavior, got: {debug}"
    );
    assert!(
        debug.contains("steps unclear"),
        "expected reason as message, got: {debug}"
    );
}

#[test]
fn to_control_response_reject_edit_maps_to_deny_with_reason() {
    let gate = make_edit_gate();
    let d = reject("session-edit", &gate, "do not apply");
    let resp = to_control_response(&d);
    let debug = format!("{:?}", resp);
    assert!(
        debug.to_lowercase().contains("deny"),
        "expected deny behavior, got: {debug}"
    );
    assert!(
        debug.contains("do not apply"),
        "expected reason as message, got: {debug}"
    );
}

// ---------------------------------------------------------------------------
// route — yields exactly the decision's session_id
// ---------------------------------------------------------------------------

#[test]
fn route_approve_returns_exact_session_id() {
    let gate = make_permission_gate();
    let d = approve("session-route-test", &gate);
    assert_eq!(route(&d), "session-route-test");
}

#[test]
fn route_reject_returns_exact_session_id() {
    let gate = make_plan_gate();
    let d = reject("session-route-reject", &gate, "reason");
    assert_eq!(route(&d), "session-route-reject");
}

#[test]
fn route_edit_approve_returns_exact_session_id() {
    let gate = make_edit_gate();
    let d = approve("session-edit-route", &gate);
    assert_eq!(route(&d), "session-edit-route");
}

// ---------------------------------------------------------------------------
// GateDecision clone/debug/partialeq (derived traits)
// ---------------------------------------------------------------------------

#[test]
fn gate_decision_implements_clone_and_debug() {
    let gate = make_permission_gate();
    let d = approve("session-abc", &gate);
    let d2 = d.clone();
    assert_eq!(d, d2);
    let _s = format!("{d:?}");
}

#[test]
fn plan_step_implements_clone_and_debug() {
    let s = PlanStep { title: "t".to_string(), detail: "d".to_string() };
    let s2 = s.clone();
    assert_eq!(s, s2);
    let _dbg = format!("{s:?}");
}

#[test]
fn gate_permission_implements_clone_and_debug() {
    let g = make_permission_gate();
    let g2 = g.clone();
    assert_eq!(g, g2);
    let _dbg = format!("{g:?}");
}

#[test]
fn gate_plan_implements_clone_and_debug() {
    let g = make_plan_gate();
    let g2 = g.clone();
    assert_eq!(g, g2);
    let _dbg = format!("{g:?}");
}

#[test]
fn gate_edit_implements_clone_and_debug() {
    let g = make_edit_gate();
    let g2 = g.clone();
    assert_eq!(g, g2);
    let _dbg = format!("{g:?}");
}
