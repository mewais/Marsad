// B05 marsad-format — branch-coverage tests
// Closes remaining regions:
//   - fmt_tokens: ".0" strip vs not; <1000 plain; >=1000 with decimal
//   - fmt_elapsed: <60m vs >=60m; zero-padding
//   - ctx_color: exact thresholds 74/75/89/90/100
//   - state_meta: every Phase including Stale/Detached
//   - gate_meta: each Gate variant; empty plan steps edge case

use marsad_format::{ctx_color, fmt_elapsed, fmt_tokens, gate_meta, state_meta, Phase, Token};
use marsad_gates::{Gate, PlanStep};

// ===== fmt_tokens branches =====

#[test]
fn fmt_tokens_below_1000_is_plain() {
    // <1000 path — returns raw integer string
    assert_eq!(fmt_tokens(500), "500");
    assert_eq!(fmt_tokens(1), "1");
    assert_eq!(fmt_tokens(0), "0");
}

#[test]
fn fmt_tokens_exactly_1000_strips_dot_zero() {
    // 1000/1000 = 1.0 → "1k" (trailing ".0" stripped)
    assert_eq!(fmt_tokens(1000), "1k");
}

#[test]
fn fmt_tokens_2000_strips_dot_zero() {
    // 2000/1000 = 2.0 → "2k"
    assert_eq!(fmt_tokens(2000), "2k");
}

#[test]
fn fmt_tokens_1500_keeps_decimal() {
    // 1500/1000 = 1.5 → "1.5k" (not stripped — not ".0")
    assert_eq!(fmt_tokens(1500), "1.5k");
}

#[test]
fn fmt_tokens_84200_keeps_one_decimal() {
    assert_eq!(fmt_tokens(84200), "84.2k");
}

// NOTE: toFixed(1) rounds half-up. 1050/1000 = 1.05 → toFixed(1) = "1.1"
#[test]
fn fmt_tokens_rounding() {
    assert_eq!(fmt_tokens(1050), "1.1k");
}

// ===== fmt_elapsed branches =====

#[test]
fn fmt_elapsed_minutes_and_seconds_mode() {
    // < 60 minutes → "Mm SSs" format with zero-padded seconds
    assert_eq!(fmt_elapsed(0), "0m 00s");
    assert_eq!(fmt_elapsed(60), "1m 00s");
    assert_eq!(fmt_elapsed(61), "1m 01s");
    assert_eq!(fmt_elapsed(3599), "59m 59s");
}

#[test]
fn fmt_elapsed_zero_pad_single_digit_seconds() {
    // seconds < 10 must be zero-padded
    assert_eq!(fmt_elapsed(65), "1m 05s");
    assert_eq!(fmt_elapsed(9), "0m 09s");
}

#[test]
fn fmt_elapsed_no_pad_double_digit_seconds() {
    // seconds >= 10 should not be double-padded
    assert_eq!(fmt_elapsed(75), "1m 15s");
}

#[test]
fn fmt_elapsed_exactly_60_minutes_switches_to_hours() {
    // 3600s = 60m exactly → "1h 0m"
    assert_eq!(fmt_elapsed(3600), "1h 0m");
}

#[test]
fn fmt_elapsed_above_60_minutes_hours_mode() {
    // 7500s = 125m → 2h 5m
    assert_eq!(fmt_elapsed(7500), "2h 5m");
    // 3660s = 61m → 1h 1m
    assert_eq!(fmt_elapsed(3660), "1h 1m");
}

// ===== ctx_color thresholds (authoritative corrected behavior) =====

/// The corrected spec: 90+ → Error; 75–89 → Needs; else → Accent.
/// The prototype had a bug in ordering (checked >=75 first so >=90 hit Needs).
/// We test the FIXED thresholds as required by the spec.

#[test]
fn ctx_color_74_is_accent() {
    assert_eq!(ctx_color(74.0), Token::Accent);
}

#[test]
fn ctx_color_75_is_needs() {
    assert_eq!(ctx_color(75.0), Token::Needs);
}

#[test]
fn ctx_color_89_is_needs() {
    // 89 is >=75 but <90 → Needs
    assert_eq!(ctx_color(89.0), Token::Needs);
}

#[test]
fn ctx_color_90_is_error() {
    // 90 exactly crosses the Error threshold
    assert_eq!(ctx_color(90.0), Token::Error);
}

#[test]
fn ctx_color_100_is_error() {
    assert_eq!(ctx_color(100.0), Token::Error);
}

// ===== state_meta: every Phase =====

#[test]
fn state_meta_starting() {
    let m = state_meta(Phase::Starting);
    assert_eq!(m.label, "Starting");
    // Starting uses Neutral color per prototype: color:var(--neutral)
    assert_eq!(m.color, Token::Neutral);
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

#[test]
fn state_meta_stale() {
    // Stale is extended beyond prototype; spec says Stale→Neutral
    let m = state_meta(Phase::Stale);
    assert_eq!(m.label, "Stale");
    assert_eq!(m.color, Token::Neutral);
}

#[test]
fn state_meta_detached() {
    let m = state_meta(Phase::Detached);
    assert_eq!(m.label, "Detached");
    assert_eq!(m.color, Token::Accent2);
}

// ===== gate_meta: each Gate variant =====

#[test]
fn gate_meta_permission() {
    let gate = Gate::Permission {
        request_id: "req-test".to_string(),
        command: String::from("git push"),
        cwd: String::from("/repo"),
        risk: String::from("medium"),
        explanation: String::from("push to remote"),
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.kind, "Permission");
    assert_eq!(gm.icon, "⌘");
    assert_eq!(gm.lead, "Wants to run");
    assert_eq!(gm.summary, "$ git push");
    assert_eq!(gm.approve_label, "Approve & run");
}

#[test]
fn gate_meta_plan_with_steps() {
    let gate = Gate::Plan {
        summary: String::from("Refactor"),
        steps: vec![
            PlanStep {
                title: String::from("Analyse codebase"),
                detail: String::from("Read all files"),
            },
            PlanStep {
                title: String::from("Apply changes"),
                detail: String::from("Edit files"),
            },
            PlanStep {
                title: String::from("Run tests"),
                detail: String::from("Verify"),
            },
        ],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.kind, "Plan review");
    assert_eq!(gm.icon, "❑");
    assert_eq!(gm.lead, "Proposed plan");
    // summary = "<N> steps · <first title>…"
    assert_eq!(gm.summary, "3 steps · Analyse codebase…");
    assert_eq!(gm.approve_label, "Approve plan");
}

#[test]
fn gate_meta_plan_with_single_step() {
    // Single step: "1 steps · <title>…"
    // NOTE: The spec copies the JS prototype literally (n+" steps") with no
    // singular/plural logic; we match that exactly.
    let gate = Gate::Plan {
        summary: String::from("Fix bug"),
        steps: vec![PlanStep {
            title: String::from("Patch file"),
            detail: String::from("Edit lib.rs"),
        }],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.summary, "1 steps · Patch file…");
}

#[test]
fn gate_meta_plan_empty_steps() {
    // Edge: 0 steps — the prototype would panic on steps[0].title; the Rust impl
    // must handle this gracefully. With 0 steps the summary is derived from the
    // plan markdown body (first non-empty line), not "0 steps".
    let gate = Gate::Plan {
        summary: String::from("Empty plan"),
        steps: vec![],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.kind, "Plan review");
    assert_eq!(gm.approve_label, "Approve plan");
    // When steps is empty, summary comes from the plan markdown (first line).
    assert_eq!(gm.summary, "Empty plan");
}

#[test]
fn gate_meta_edit_basename_and_counts() {
    use marsad_gates::{DiffLine, Sig};
    let gate = Gate::Edit {
        file: String::from("/home/user/project/src/lib.rs"),
        diff: vec![
            DiffLine {
                sig: Sig::Add,
                text: String::from("+ line"),
                ln: String::from("1"),
            },
            DiffLine {
                sig: Sig::Add,
                text: String::from("+ line2"),
                ln: String::from("2"),
            },
            DiffLine {
                sig: Sig::Del,
                text: String::from("- old"),
                ln: String::from("3"),
            },
            DiffLine {
                sig: Sig::Context,
                text: String::from("  ctx"),
                ln: String::from("4"),
            },
        ],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.kind, "File change");
    assert_eq!(gm.icon, "±");
    assert_eq!(gm.lead, "Wants to edit");
    // Context lines are NOT counted; basename of file; +A −D
    assert_eq!(gm.summary, "lib.rs · +2 −1");
    assert_eq!(gm.approve_label, "Approve change");
}

#[test]
fn gate_meta_edit_zero_adds_zero_dels() {
    use marsad_gates::{DiffLine, Sig};
    // All-context diff: +0 −0
    let gate = Gate::Edit {
        file: String::from("plain.txt"),
        diff: vec![DiffLine {
            sig: Sig::Context,
            text: String::from("no change"),
            ln: String::from("1"),
        }],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.summary, "plain.txt · +0 −0");
}

#[test]
fn gate_meta_edit_empty_diff() {
    let gate = Gate::Edit {
        file: String::from("empty.rs"),
        diff: vec![],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.summary, "empty.rs · +0 −0");
}

// ---------------------------------------------------------------------------
// Gate::Question coverage
// ---------------------------------------------------------------------------

#[test]
fn gate_meta_question_single_question() {
    use marsad_gates::{Gate, QuestionOption, QuestionPrompt};
    let gate = Gate::Question {
        request_id: "req-q".to_string(),
        questions: vec![QuestionPrompt {
            question: "Which approach?".to_string(),
            header: "Strategy".to_string(),
            multi_select: false,
            options: vec![QuestionOption {
                label: "A".to_string(),
                description: "first".to_string(),
            }],
        }],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.kind, "question");
    assert_eq!(gm.icon, "?");
    assert_eq!(gm.lead, "Wants your input");
    assert_eq!(gm.summary, "Which approach?");
    assert_eq!(gm.approve_label, "Submit");
}

#[test]
fn gate_meta_question_no_questions() {
    use marsad_gates::Gate;
    let gate = Gate::Question {
        request_id: "req-q0".to_string(),
        questions: vec![],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.summary, "Question");
}

#[test]
fn gate_meta_question_multiple_questions() {
    use marsad_gates::{Gate, QuestionPrompt};
    let gate = Gate::Question {
        request_id: "req-qn".to_string(),
        questions: vec![
            QuestionPrompt {
                question: "Q1".to_string(),
                header: String::new(),
                multi_select: false,
                options: vec![],
            },
            QuestionPrompt {
                question: "Q2".to_string(),
                header: String::new(),
                multi_select: false,
                options: vec![],
            },
        ],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.summary, "2 questions");
}

// ---------------------------------------------------------------------------
// Plan with empty summary (all whitespace) → "Plan" fallback
// ---------------------------------------------------------------------------

#[test]
fn gate_meta_plan_whitespace_only_summary() {
    use marsad_gates::Gate;
    let gate = Gate::Plan {
        summary: String::from("   \n  "),
        steps: vec![],
    };
    let gm = gate_meta(&gate);
    assert_eq!(gm.summary, "Plan");
}

// ---------------------------------------------------------------------------
// Plan with summary > 60 chars (truncation)
// ---------------------------------------------------------------------------

#[test]
fn gate_meta_plan_long_summary_truncated() {
    use marsad_gates::Gate;
    // A first line of exactly 65 characters.
    let long_line = "A".repeat(65);
    let gate = Gate::Plan {
        summary: long_line.clone(),
        steps: vec![],
    };
    let gm = gate_meta(&gate);
    // Should be first 60 chars + "…"
    let expected = format!("{}…", &long_line[..60]);
    assert_eq!(gm.summary, expected);
}
