// B05 marsad-format — happy path tests
// Covers nominal paths for every public fn/variant.

use marsad_format::{
    ctx_color, fmt_elapsed, fmt_tokens, gate_meta, state_meta, GateMeta, Phase, Token,
};
use marsad_gates::{Gate, PlanStep};

// ---------- fmt_tokens ----------

#[test]
fn fmt_tokens_plain_small() {
    assert_eq!(fmt_tokens(0), "0");
}

#[test]
fn fmt_tokens_plain_999() {
    assert_eq!(fmt_tokens(999), "999");
}

#[test]
fn fmt_tokens_1234() {
    // 1234 / 1000 = 1.2  (one decimal, no trailing .0)
    assert_eq!(fmt_tokens(1234), "1.2k");
}

#[test]
fn fmt_tokens_84200() {
    // 84200 / 1000 = 84.2
    assert_eq!(fmt_tokens(84200), "84.2k");
}

// ---------- fmt_elapsed ----------

#[test]
fn fmt_elapsed_123_seconds() {
    // 123s = 2m 03s (zero-padded seconds)
    assert_eq!(fmt_elapsed(123), "2m 03s");
}

#[test]
fn fmt_elapsed_65_seconds() {
    // 65s = 1m 05s
    assert_eq!(fmt_elapsed(65), "1m 05s");
}

#[test]
fn fmt_elapsed_59_minutes_59_seconds() {
    // 3599s = 59m 59s (still below 60m threshold)
    assert_eq!(fmt_elapsed(3599), "59m 59s");
}

#[test]
fn fmt_elapsed_2h5m() {
    // 7500s = 125m = 2h 5m
    assert_eq!(fmt_elapsed(7500), "2h 5m");
}

// ---------- state_meta ----------

#[test]
fn state_meta_starting() {
    let m = state_meta(Phase::Starting);
    assert_eq!(m.label, "Starting");
}

#[test]
fn state_meta_working() {
    let m = state_meta(Phase::Working);
    assert_eq!(m.label, "Working");
    assert_eq!(m.color, Token::Accent);
}

#[test]
fn state_meta_needs() {
    let m = state_meta(Phase::Needs);
    assert_eq!(m.label, "Needs you");
    assert_eq!(m.color, Token::Needs);
}

#[test]
fn state_meta_error() {
    let m = state_meta(Phase::Error);
    assert_eq!(m.label, "Error");
    assert_eq!(m.color, Token::Error);
}

#[test]
fn state_meta_done() {
    let m = state_meta(Phase::Done);
    assert_eq!(m.label, "Done");
    assert_eq!(m.color, Token::Done);
}

// ---------- ctx_color ----------

#[test]
fn ctx_color_below_75_is_accent() {
    assert_eq!(ctx_color(50.0), Token::Accent);
}

#[test]
fn ctx_color_at_75_is_needs() {
    assert_eq!(ctx_color(75.0), Token::Needs);
}

#[test]
fn ctx_color_at_90_is_error() {
    assert_eq!(ctx_color(90.0), Token::Error);
}

// ---------- gate_meta: permission ----------

#[test]
fn gate_meta_permission_fields() {
    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: String::from("ls -la"),
        cwd: String::from("/tmp"),
        risk: String::from("low"),
        explanation: String::from("list files"),
        is_elicitation: false,
    };
    let gm: GateMeta = gate_meta(&gate);
    assert_eq!(gm.kind, "Permission");
    assert_eq!(gm.icon, "⌘");
    assert_eq!(gm.lead, "Wants to run");
    assert_eq!(gm.summary, "$ ls -la");
    assert_eq!(gm.approve_label, "Approve & run");
}

// ---------- gate_meta: plan ----------

#[test]
fn gate_meta_plan_fields() {
    let gate = Gate::Plan {
        request_id: "req-plan-happy".to_string(),
        summary: String::from("Refactor module"),
        steps: vec![
            PlanStep {
                title: String::from("Read source"),
                detail: String::from("Open the files"),
            },
            PlanStep {
                title: String::from("Write tests"),
                detail: String::from("Add coverage"),
            },
        ],
    };
    let gm: GateMeta = gate_meta(&gate);
    assert_eq!(gm.kind, "Plan review");
    assert_eq!(gm.icon, "❑");
    assert_eq!(gm.lead, "Proposed plan");
    assert_eq!(gm.summary, "2 steps · Read source…");
    assert_eq!(gm.approve_label, "Approve plan");
}

// ---------- gate_meta: edit ----------

#[test]
fn gate_meta_edit_fields() {
    use marsad_gates::{DiffLine, Sig};
    let gate = Gate::Edit {
        request_id: "req-edit-happy".to_string(),
        file: String::from("/home/user/project/src/main.rs"),
        diff: vec![
            DiffLine {
                sig: Sig::Add,
                text: String::from("fn foo() {}"),
                ln: String::from("1"),
            },
            DiffLine {
                sig: Sig::Add,
                text: String::from("fn bar() {}"),
                ln: String::from("2"),
            },
            DiffLine {
                sig: Sig::Del,
                text: String::from("fn old() {}"),
                ln: String::from("3"),
            },
        ],
    };
    let gm: GateMeta = gate_meta(&gate);
    assert_eq!(gm.kind, "File change");
    assert_eq!(gm.icon, "±");
    assert_eq!(gm.lead, "Wants to edit");
    // basename = "main.rs", 2 adds, 1 del
    assert_eq!(gm.summary, "main.rs · +2 −1");
    assert_eq!(gm.approve_label, "Approve change");
}
