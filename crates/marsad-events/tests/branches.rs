//! marsad-events — branch coverage over `AgentEvent`.
//!
//! Covers every Phase variant, the usage-SNAPSHOT invariant (Wave 1 replaced the
//! old per-message-id DEDUP accumulation — usage now arrives as authoritative
//! snapshots via UsageUpdate/TurnCompleted), context_pct clamp to 100,
//! context_window_for's two branches ([1m] vs default), every HookEvent variant,
//! Stale/Detached preservation, and Question/Plan gate shaping.

mod common;
use common::*;

use marsad_events::{reduce, Event, HookEvent, Phase, SessionState, SubagentStatus};
use wasit_core::EndReason as WEndReason;

fn init_with_model(model: &str) -> SessionState {
    after_init(session_init("sess-br", model, "/tmp"))
}

// ---------------------------------------------------------------------------
// Every Phase reachable
// ---------------------------------------------------------------------------

#[test]
fn phase_starting_is_initial() {
    assert_eq!(SessionState::initial().phase, Phase::Starting);
}

#[test]
fn phase_working_reachable_via_session_init() {
    assert_eq!(init_with_model("claude-opus-4-5").phase, Phase::Working);
}

#[test]
fn phase_needs_reachable_via_gate() {
    let s1 = reduce(init_with_model("claude-opus-4-5"), ev(permission_gate("req-1", "ls", "list")));
    assert_eq!(s1.phase, Phase::Needs);
}

#[test]
fn phase_needs_reachable_via_notification_hook() {
    let s1 = reduce(
        init_with_model("claude-opus-4-5"),
        Event::Hook(HookEvent::Notification {
            message: "Something needs attention".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Needs);
}

#[test]
fn phase_error_reachable_via_execution_error() {
    let s1 = reduce(
        init_with_model("claude-opus-4-5"),
        ev(turn_end(WEndReason::ExecutionError, usage(10, 2, 0, 0, 0.01))),
    );
    assert_eq!(s1.phase, Phase::Error);
}

#[test]
fn phase_error_reachable_via_max_turns() {
    let s1 = reduce(
        init_with_model("claude-opus-4-5"),
        ev(turn_end(WEndReason::MaxTurns, usage(10, 2, 0, 0, 0.01))),
    );
    assert_eq!(s1.phase, Phase::Error);
}

#[test]
fn phase_done_reachable_via_turn_success() {
    let s1 = reduce(init_with_model("claude-opus-4-5"), ev(turn_success(usage(100, 20, 0, 0, 0.5))));
    assert_eq!(s1.phase, Phase::Done);
}

#[test]
fn phase_done_reachable_via_stop_hook() {
    let s1 = reduce(init_with_model("claude-opus-4-5"), Event::Hook(HookEvent::Stop));
    assert_eq!(s1.phase, Phase::Done);
}

#[test]
fn phase_stale_set_externally_and_preserved_by_hook() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(s, Event::Hook(HookEvent::PreCompact));
    assert_eq!(s1.phase, Phase::Stale);
}

#[test]
fn phase_detached_set_externally_and_preserved_by_hook() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(s, Event::Hook(HookEvent::PreCompact));
    assert_eq!(s1.phase, Phase::Detached);
}

#[test]
fn phase_stale_preserved_by_unknown_event() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(
        s,
        ev(wasit_core::AgentEvent::Unknown {
            raw: serde_json::json!({"type": "future_msg"}),
        }),
    );
    assert_eq!(s1.phase, Phase::Stale);
}

#[test]
fn phase_detached_preserved_by_unknown_event() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(
        s,
        ev(wasit_core::AgentEvent::Unknown {
            raw: serde_json::json!({"type": "future_msg"}),
        }),
    );
    assert_eq!(s1.phase, Phase::Detached);
}

// ---------------------------------------------------------------------------
// Usage SNAPSHOT (Wave 1) — UsageUpdate/TurnCompleted replace, never accumulate.
// ---------------------------------------------------------------------------

#[test]
fn usage_update_snapshot_replaces_not_accumulates() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, ev(usage_update(usage(100, 20, 0, 0, 0.0), None)));
    assert_eq!(s1.tokens, 120, "first snapshot: 100+20=120");
    // A repeat/refresh snapshot with the same totals stays 120 (replace).
    let s2 = reduce(s1, ev(usage_update(usage(100, 20, 0, 0, 0.0), None)));
    assert_eq!(s2.tokens, 120, "a repeated snapshot must not double-count");
}

#[test]
fn later_snapshot_reflects_new_cumulative_total() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, ev(usage_update(usage(100, 20, 0, 0, 0.0), None))); // 120
    let s2 = reduce(s1, ev(usage_update(usage(300, 50, 0, 0, 0.0), None))); // 350 (new cumulative)
    assert_eq!(s2.tokens, 350);
}

#[test]
fn turn_completed_snapshots_cumulative_total() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, ev(turn_success(usage(50, 10, 0, 0, 0.0)))); // 60
    let s2 = reduce(s1, ev(turn_success(usage(50, 10, 0, 0, 0.0)))); // still 60 (snapshot)
    assert_eq!(s2.tokens, 60);
}

// ---------------------------------------------------------------------------
// context_pct clamp to 100 (derived from tokens when no explicit context_pct)
// ---------------------------------------------------------------------------

#[test]
fn context_pct_computed_roughly_at_half_window() {
    let model = "claude-opus-4-5";
    let window = SessionState::context_window_for(model);
    assert!(window > 0);
    let half = window / 2;
    let s1 = reduce(init_with_model(model), ev(usage_update(usage(half, 0, 0, 0, 0.0), None)));
    assert!(
        (s1.context_pct - 50.0).abs() < 2.0,
        "expected ~50% context at half-window, got {}",
        s1.context_pct
    );
}

#[test]
fn context_pct_clamped_to_100_on_overflow() {
    let model = "claude-opus-4-5";
    let window = SessionState::context_window_for(model);
    let s1 = reduce(init_with_model(model), ev(usage_update(usage(window * 2, 0, 0, 0, 0.0), None)));
    assert_eq!(s1.context_pct, 100.0, "context_pct must clamp to 100.0 on overflow");
}

#[test]
fn context_pct_from_explicit_value_is_clamped() {
    // opencode reports context_pct directly; the reducer clamps to [0,100].
    let s1 = reduce(
        init_with_model("gpt-4o"),
        ev(usage_update(usage(1, 1, 0, 0, 0.0), Some(150.0))),
    );
    assert_eq!(s1.context_pct, 100.0);
}

// ---------------------------------------------------------------------------
// context_window_for: [1m] suffix vs 200_000 default
// ---------------------------------------------------------------------------

#[test]
fn context_window_for_named_models_without_1m_suffix_default_to_200_000() {
    let models = [
        "claude-opus-4-5",
        "claude-sonnet-4-5",
        "claude-sonnet-4-6",
        "claude-haiku-3-5",
        "claude-opus-4-8",
        "hypothetical-future-model-xyz-999",
    ];
    for model in &models {
        assert_eq!(
            SessionState::context_window_for(model),
            200_000,
            "context_window_for({model}) must default to 200_000"
        );
    }
}

#[test]
fn context_window_for_1m_suffix_returns_1_000_000() {
    assert_eq!(SessionState::context_window_for("claude-opus-4.8[1m]"), 1_000_000);
}

#[test]
fn context_window_for_1m_suffix_on_any_model_name_returns_1_000_000() {
    assert_eq!(SessionState::context_window_for("claude-sonnet-4-6[1m]"), 1_000_000);
}

// ---------------------------------------------------------------------------
// Every HookEvent variant: no panic, expected outcome
// ---------------------------------------------------------------------------

#[test]
fn hook_all_variants_total() {
    let hooks = vec![
        Event::Hook(HookEvent::Notification {
            message: "test".to_string(),
        }),
        Event::Hook(HookEvent::Stop),
        Event::Hook(HookEvent::SubagentStop {
            name: "agent-x".to_string(),
        }),
        Event::Hook(HookEvent::PreToolUse {
            tool: "Write".to_string(),
        }),
        Event::Hook(HookEvent::PostToolUse {
            tool: "Write".to_string(),
        }),
        Event::Hook(HookEvent::SessionStart),
        Event::Hook(HookEvent::PreCompact),
    ];
    for hook in hooks {
        let _s1 = reduce(init_with_model("claude-opus-4-5"), hook);
    }
}

#[test]
fn hook_notification_gives_needs() {
    let s1 = reduce(
        init_with_model("claude-opus-4-5"),
        Event::Hook(HookEvent::Notification {
            message: "confirm tool use".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Needs);
}

#[test]
fn hook_stop_gives_done() {
    let s1 = reduce(init_with_model("claude-opus-4-5"), Event::Hook(HookEvent::Stop));
    assert_eq!(s1.phase, Phase::Done);
}

#[test]
fn hook_subagent_stop_records_agent() {
    let s1 = reduce(
        init_with_model("claude-opus-4-5"),
        Event::Hook(HookEvent::SubagentStop {
            name: "search-agent".to_string(),
        }),
    );
    let agent = s1.subagents.iter().find(|a| a.name == "search-agent");
    assert!(agent.is_some());
    assert_eq!(agent.unwrap().status, SubagentStatus::Done);
}

#[test]
fn hook_pre_tool_use_keeps_working() {
    let s1 = reduce(
        init_with_model("claude-opus-4-5"),
        Event::Hook(HookEvent::PreToolUse {
            tool: "Read".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Working);
}

#[test]
fn hook_post_tool_use_keeps_working() {
    let s1 = reduce(
        init_with_model("claude-opus-4-5"),
        Event::Hook(HookEvent::PostToolUse {
            tool: "Read".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Working);
}

#[test]
fn hook_session_start_total_from_starting() {
    let s1 = reduce(SessionState::initial(), Event::Hook(HookEvent::SessionStart));
    let _ = s1.phase;
}

#[test]
fn hook_pre_compact_preserves_working() {
    let s1 = reduce(init_with_model("claude-opus-4-5"), Event::Hook(HookEvent::PreCompact));
    assert_eq!(s1.phase, Phase::Working);
}

// ---------------------------------------------------------------------------
// Stale / Detached: superseded ONLY by a real SessionInit.
// ---------------------------------------------------------------------------

#[test]
fn stale_superseded_by_new_session_init() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(s, ev(session_init("sess-new", "claude-opus-4-5", "/tmp")));
    assert_eq!(s1.phase, Phase::Working, "SessionInit must supersede Stale");
}

#[test]
fn detached_superseded_by_new_session_init() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(s, ev(session_init("sess-new2", "claude-opus-4-5", "/tmp")));
    assert_eq!(s1.phase, Phase::Working, "SessionInit must supersede Detached");
}

#[test]
fn stale_preserved_by_assistant_delta() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(s, ev(assistant_delta("x")));
    assert_eq!(s1.phase, Phase::Stale);
}

#[test]
fn detached_preserved_by_tool_call() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(s, ev(tool_call("Read", "t", serde_json::json!({}))));
    assert_eq!(s1.phase, Phase::Detached);
}

#[test]
fn stale_preserved_by_turn_completed() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Stale;
    let s1 = reduce(s, ev(turn_success(usage(100, 20, 0, 0, 0.5))));
    assert_eq!(s1.phase, Phase::Stale);
}

#[test]
fn detached_preserved_by_gate() {
    let mut s = init_with_model("claude-opus-4-5");
    s.phase = Phase::Detached;
    let s1 = reduce(s, ev(permission_gate("req-detach", "ls", "list")));
    assert_eq!(s1.phase, Phase::Detached);
    assert!(s1.gate.is_none(), "a gate must not be enqueued while Detached");
}

#[test]
fn stale_preserved_by_all_hooks() {
    let hooks = vec![
        Event::Hook(HookEvent::Stop),
        Event::Hook(HookEvent::SubagentStop { name: "a".to_string() }),
        Event::Hook(HookEvent::PreToolUse { tool: "Bash".to_string() }),
        Event::Hook(HookEvent::PostToolUse { tool: "Bash".to_string() }),
        Event::Hook(HookEvent::SessionStart),
        Event::Hook(HookEvent::PreCompact),
    ];
    for hook in hooks {
        let mut s = init_with_model("claude-opus-4-5");
        s.phase = Phase::Stale;
        assert_eq!(reduce(s, hook).phase, Phase::Stale);
    }
}

// ---------------------------------------------------------------------------
// Derives
// ---------------------------------------------------------------------------

#[test]
fn session_state_derives_clone_eq_debug() {
    let s = SessionState::initial();
    let s2 = s.clone();
    assert_eq!(s, s2);
    let _ = format!("{s:?}");
}

#[test]
fn phase_all_variants_derive_clone_eq_debug() {
    let phases = [
        Phase::Starting,
        Phase::Working,
        Phase::Needs,
        Phase::Error,
        Phase::Done,
        Phase::Stale,
        Phase::Detached,
    ];
    for p in &phases {
        let p2 = p.clone();
        assert_eq!(p, &p2);
        let _ = format!("{p:?}");
    }
}

#[test]
fn subagent_status_derives_clone_eq_debug() {
    let running = SubagentStatus::Running;
    let done = SubagentStatus::Done;
    assert_eq!(running.clone(), SubagentStatus::Running);
    assert_eq!(done.clone(), SubagentStatus::Done);
    assert_ne!(running, done);
    let _ = format!("{running:?} {done:?}");
}

// ---------------------------------------------------------------------------
// Question / Plan gate shaping from typed GateRequest
// ---------------------------------------------------------------------------

#[test]
fn question_gate_single_select_builds_question_gate() {
    let s0 = init_with_model("claude-opus-4-5");
    let q = qprompt(
        "Which option?",
        "Choose",
        false,
        vec![("A", "First"), ("B", "Second")],
    );
    let s1 = reduce(s0, ev(question_gate("req-aq-1", vec![q])));
    assert_eq!(s1.phase, Phase::Needs);
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Question { request_id, questions } => {
            assert_eq!(request_id, "req-aq-1");
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].question, "Which option?");
            assert_eq!(questions[0].header, "Choose");
            assert!(!questions[0].multi_select);
            assert_eq!(questions[0].options.len(), 2);
            assert_eq!(questions[0].options[0].label, "A");
            assert_eq!(questions[0].options[0].description, "First");
        }
        other => panic!("expected Question gate, got {other:?}"),
    }
}

#[test]
fn question_gate_multi_select_builds_question_gate() {
    let s0 = init_with_model("claude-opus-4-5");
    let q = qprompt("Pick all?", "Multi", true, vec![("X", ""), ("Y", "")]);
    let s1 = reduce(s0, ev(question_gate("req-aq-2", vec![q])));
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Question { questions, .. } => assert!(questions[0].multi_select),
        other => panic!("expected Question gate, got {other:?}"),
    }
}

#[test]
fn question_gate_empty_questions() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, ev(question_gate("req-aq-3", vec![])));
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Question { questions, .. } => assert!(questions.is_empty()),
        other => panic!("expected Question gate, got {other:?}"),
    }
}

#[test]
fn plan_gate_builds_plan_gate_with_summary() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, ev(plan_gate("req-ep-1", "## The Plan\n\n1. Step one\n2. Step two")));
    assert_eq!(s1.phase, Phase::Needs);
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Plan { summary, steps, .. } => {
            assert!(summary.contains("Step one"), "summary: {summary}");
            assert!(steps.is_empty());
        }
        other => panic!("expected Plan gate, got {other:?}"),
    }
}

#[test]
fn plan_gate_empty_summary() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, ev(plan_gate("req-ep-2", "")));
    match s1.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Plan { summary, steps, .. } => {
            assert!(summary.is_empty());
            assert!(steps.is_empty());
        }
        other => panic!("expected Plan gate, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Gate dedup + queue advance
// ---------------------------------------------------------------------------

#[test]
fn duplicate_gate_request_id_is_ignored() {
    let s0 = init_with_model("claude-opus-4-5");
    let s1 = reduce(s0, ev(permission_gate("req-dup", "ls", "list")));
    let s2 = reduce(s1, ev(permission_gate("req-dup", "ls", "list again")));
    assert!(s2.gate.is_some());
    assert!(s2.gate_queue.is_empty(), "a duplicate request_id must not queue");
}

#[test]
fn second_distinct_gate_queues_and_advances() {
    let mut s = init_with_model("claude-opus-4-5");
    s = reduce(s, ev(permission_gate("req-1", "ls", "one")));
    s = reduce(s, ev(permission_gate("req-2", "cat", "two")));
    assert_eq!(s.gate_queue.len(), 1, "second gate waits behind the first");
    // Answering the first advances to the second.
    s.clear_gate_on_answer();
    assert!(s.gate.is_some());
    assert!(s.gate_queue.is_empty());
    match s.gate.as_ref().unwrap() {
        marsad_gates::Gate::Permission { request_id, .. } => assert_eq!(request_id, "req-2"),
        other => panic!("expected the queued gate, got {other:?}"),
    }
    // Answering the last clears + flips back to Working.
    s.clear_gate_on_answer();
    assert!(s.gate.is_none());
    assert_eq!(s.phase, Phase::Working);
}
