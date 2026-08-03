//! marsad-events — error paths and out-of-order robustness, over `AgentEvent`.
//!
//! Covers:
//! - TurnCompleted(ExecutionError) → Phase::Error
//! - TurnCompleted(MaxTurns) → Phase::Error
//! - interrupt classification: interrupt_requested / was_interrupted → Cancelled/Done
//! - Out-of-order events before SessionInit (no panic, total)
//! - Double TurnCompleted after Done: snapshot tokens, no panic, pct ≤ 100
//! - Unknown event: state preserved, no panic
//! - Stale/Detached: non-init events are totality no-ops

mod common;
use common::*;

use marsad_events::{reduce, EndReason, Phase, SessionState};
use wasit_core::EndReason as WEndReason;

fn init() -> SessionState {
    after_init(session_init("sess-err", "claude-opus-4-5", "/tmp"))
}

// ---------------------------------------------------------------------------
// ExecutionError → Phase::Error
// ---------------------------------------------------------------------------

#[test]
fn execution_error_gives_error_phase() {
    let s1 = reduce(init(), ev(turn_end(WEndReason::ExecutionError, usage(50, 10, 0, 0, 0.01))));
    assert_eq!(s1.phase, Phase::Error);
    assert_eq!(s1.end_reason, Some(EndReason::ExecutionError));
}

// ---------------------------------------------------------------------------
// MaxTurns → Phase::Error
// ---------------------------------------------------------------------------

#[test]
fn max_turns_gives_error_phase() {
    let s1 = reduce(init(), ev(turn_end(WEndReason::MaxTurns, usage(50, 10, 0, 0, 0.01))));
    assert_eq!(s1.phase, Phase::Error);
    assert_eq!(s1.end_reason, Some(EndReason::MaxTurns));
}

// ---------------------------------------------------------------------------
// CompactionNeeded (opencode-only, no marsad EndReason) → Error/ExecutionError
// ---------------------------------------------------------------------------

#[test]
fn compaction_needed_maps_to_execution_error() {
    let s1 = reduce(
        init(),
        ev(turn_end(WEndReason::CompactionNeeded, usage(50, 10, 0, 0, 0.01))),
    );
    assert_eq!(s1.phase, Phase::Error);
    assert_eq!(
        s1.end_reason,
        Some(EndReason::ExecutionError),
        "CompactionNeeded has no marsad EndReason; it maps to ExecutionError"
    );
}

// ---------------------------------------------------------------------------
// interrupt classification: cancel vs real error
// ---------------------------------------------------------------------------

/// The core-set `interrupt_requested` flag turns an ExecutionError result into a
/// calm Cancel (Done, Cancelled) and consumes the one-shot flag.
#[test]
fn interrupt_requested_is_cancelled_not_error() {
    let mut s = init();
    s.interrupt_requested = true;
    let s1 = reduce(s, ev(turn_end(WEndReason::ExecutionError, usage(50, 10, 0, 0, 0.01))));
    assert_eq!(s1.phase, Phase::Done, "user cancel is Done, not Error");
    assert_eq!(s1.end_reason, Some(EndReason::Cancelled));
    assert!(!s1.interrupt_requested, "interrupt flag consumed by the turn");
    assert_eq!(EndReason::Cancelled.text(), "Cancelled by the user");
}

/// The driver's own `was_interrupted` flag (TurnCompleted) is likewise a calm Cancel.
#[test]
fn was_interrupted_is_cancelled_not_error() {
    let s1 = reduce(init(), ev(turn_interrupted(usage(50, 10, 0, 0, 0.01))));
    assert_eq!(s1.phase, Phase::Done);
    assert_eq!(s1.end_reason, Some(EndReason::Cancelled));
}

/// A user interrupt while a gate is OPEN: the cancelled turn must resolve
/// (Done + Cancelled) AND drop the pending gate, not get held at Needs.
#[test]
fn interrupt_while_gate_open_cancels_and_clears_gate() {
    let mut s = reduce(init(), ev(permission_gate("req-x", "ls", "list")));
    assert_eq!(s.phase, Phase::Needs);
    assert!(s.gate.is_some());
    s.interrupt_requested = true;
    let s1 = reduce(s, ev(turn_end(WEndReason::ExecutionError, usage(50, 10, 0, 0, 0.01))));
    assert_eq!(s1.phase, Phase::Done, "cancelled turn is Done, not stuck Needs");
    assert_eq!(s1.end_reason, Some(EndReason::Cancelled));
    assert!(s1.gate.is_none(), "the abandoned gate must be cleared on cancel");
}

/// `was_interrupted` while a gate is open likewise cancels and clears the gate.
#[test]
fn was_interrupted_while_gate_open_clears_gate() {
    let s = reduce(init(), ev(permission_gate("req-y", "ls", "list")));
    assert!(s.gate.is_some());
    let s1 = reduce(s, ev(turn_interrupted(usage(1, 1, 0, 0, 0.0))));
    assert_eq!(s1.phase, Phase::Done);
    assert_eq!(s1.end_reason, Some(EndReason::Cancelled));
    assert!(s1.gate.is_none());
}

/// A genuine error (no interrupt) stays Error with a real reason.
#[test]
fn error_results_carry_a_reason() {
    let s_exec = reduce(init(), ev(turn_end(WEndReason::ExecutionError, usage(1, 1, 0, 0, 0.0))));
    assert_eq!(s_exec.phase, Phase::Error);
    assert_eq!(s_exec.end_reason, Some(EndReason::ExecutionError));

    let s_max = reduce(init(), ev(turn_end(WEndReason::MaxTurns, usage(1, 1, 0, 0, 0.0))));
    assert_eq!(s_max.phase, Phase::Error);
    assert_eq!(s_max.end_reason, Some(EndReason::MaxTurns));
}

/// A successful turn clears any reason; a new turn (TurnStarted) after an error
/// also clears the stale reason.
#[test]
fn success_and_new_turn_clear_end_reason() {
    let s_ok = reduce(init(), ev(turn_success(usage(1, 1, 0, 0, 0.0))));
    assert_eq!(s_ok.phase, Phase::Done);
    assert_eq!(s_ok.end_reason, None);

    let s_err = reduce(init(), ev(turn_end(WEndReason::MaxTurns, usage(1, 1, 0, 0, 0.0))));
    assert_eq!(s_err.end_reason, Some(EndReason::MaxTurns));
    // A new turn starting clears the stale reason.
    let s_next = reduce(s_err, ev(turn_started()));
    assert_eq!(s_next.phase, Phase::Working);
    assert_eq!(s_next.end_reason, None, "new turn clears the stale reason");
}

// A local shim: TurnStarted with no native id.
fn turn_started() -> wasit_core::AgentEvent {
    wasit_core::AgentEvent::TurnStarted {
        native_turn_id: None,
    }
}

// ---------------------------------------------------------------------------
// Out-of-order: events before SessionInit — total, no panic
// ---------------------------------------------------------------------------

#[test]
fn turn_completed_before_init_does_not_panic() {
    let s1 = reduce(SessionState::initial(), ev(turn_success(usage(1, 1, 0, 0, 0.0))));
    assert!(
        matches!(
            s1.phase,
            Phase::Done | Phase::Error | Phase::Starting | Phase::Working
        ),
        "unexpected phase: {:?}",
        s1.phase
    );
}

#[test]
fn usage_update_before_init_counts_tokens() {
    let s1 = reduce(SessionState::initial(), ev(usage_update(usage(50, 10, 0, 0, 0.0), None)));
    assert_eq!(s1.tokens, 60);
    assert!(s1.context_pct <= 100.0);
}

#[test]
fn gate_before_init_does_not_panic() {
    let s1 = reduce(SessionState::initial(), ev(permission_gate("req-oo", "ls", "list")));
    let _ = s1.phase;
    assert!(s1.gate.is_some());
}

// ---------------------------------------------------------------------------
// Double TurnCompleted: snapshot tokens, no panic, invariants hold
// ---------------------------------------------------------------------------

#[test]
fn double_turn_completed_does_not_panic_or_go_negative() {
    let s0 = init();
    let s1 = reduce(s0, ev(turn_success(usage(50, 10, 0, 0, 0.0))));
    assert_eq!(s1.phase, Phase::Done);
    assert_eq!(s1.tokens, 60);

    let s2 = reduce(s1, ev(turn_success(usage(50, 10, 0, 0, 0.0))));
    // TurnCompleted snapshots (replaces) tokens with the authoritative total.
    assert_eq!(s2.tokens, 60, "double TurnCompleted token count must remain 60");
    assert!(s2.context_pct <= 100.0);
}

// ---------------------------------------------------------------------------
// Unknown event: state preserved (Working), no panic
// ---------------------------------------------------------------------------

#[test]
fn unknown_event_is_total() {
    let s0 = init();
    let s1 = reduce(
        s0,
        ev(wasit_core::AgentEvent::Unknown {
            raw: serde_json::json!({"type": "future_unrecognized"}),
        }),
    );
    assert_eq!(s1.phase, Phase::Working);
}

#[test]
fn process_exited_is_noop() {
    let s0 = init();
    let s1 = reduce(s0, ev(wasit_core::AgentEvent::ProcessExited { code: Some(0) }));
    // Liveness is owned elsewhere — the reducer leaves phase untouched.
    assert_eq!(s1.phase, Phase::Working);
}

// ---------------------------------------------------------------------------
// Stale phase: non-init events preserve Stale (totality no-ops)
// ---------------------------------------------------------------------------

#[test]
fn stale_preserved_by_assistant_delta() {
    let mut s = init();
    s.phase = Phase::Stale;
    let s1 = reduce(s, ev(assistant_delta("work")));
    assert_eq!(s1.phase, Phase::Stale, "assistant must not escape Stale");
    assert!(s1.context_pct <= 100.0);
}

#[test]
fn stale_preserved_by_usage_update_and_tokens_unchanged() {
    let mut s = init();
    s.phase = Phase::Stale;
    let s1 = reduce(s, ev(usage_update(usage(50, 10, 0, 0, 0.0), None)));
    assert_eq!(s1.phase, Phase::Stale);
    assert_eq!(s1.tokens, 0, "Stale no-op must not accumulate tokens");
}

#[test]
fn detached_phase_preserved_on_unknown_event() {
    let mut s = init();
    s.phase = Phase::Detached;
    let s1 = reduce(
        s,
        ev(wasit_core::AgentEvent::Unknown {
            raw: serde_json::json!({"type": "unknown_future"}),
        }),
    );
    assert_eq!(s1.phase, Phase::Detached);
}
