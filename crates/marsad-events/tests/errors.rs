//! B02 `marsad-events` — error paths and out-of-order robustness.
//!
//! StreamMsg values constructed via `parse_stream_line` (public API only).
//!
//! Covers:
//! - ResultSubtype::ErrorDuringExecution → Phase::Error
//! - ResultSubtype::ErrorMaxTurns → Phase::Error
//! - is_error=true on Result → Phase::Error
//! - Out-of-order: Result before SystemInit (no panic, total)
//! - Out-of-order: Assistant before SystemInit (no panic, tokens non-negative)
//! - Out-of-order: ControlRequest before SystemInit (no panic)
//! - Double-Result after Done: no panic, context_pct ≤ 100, tokens non-negative
//! - Unknown StreamMsg: state preserved, no panic
//! - Stale phase: out-of-order events do not panic

use marsad_events::{reduce, EndReason, Event, Phase, SessionState};
use marsad_proto::parse_stream_line;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn stream(json: &str) -> Event {
    let msg = parse_stream_line(json)
        .unwrap_or_else(|e| panic!("test JSON failed to parse: {e:?}\njson: {json}"));
    Event::Stream(msg)
}

fn system_init_json() -> &'static str {
    r#"{"type":"system","subtype":"init","session_id":"sess-err","model":"claude-opus-4-5","cwd":"/tmp","tools":[],"mcp_servers":[],"permissionMode":"default"}"#
}

fn assistant_json(id: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"id":"{id}","content":[{{"type":"text","text":"work"}}],"usage":{{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    )
}

fn result_json(subtype: &str, is_error: bool) -> String {
    format!(
        r#"{{"type":"result","subtype":"{subtype}","is_error":{is_error},"total_cost_usd":0.01,"usage":{{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"num_turns":1,"permission_denials":[]}}"#
    )
}

fn after_init() -> SessionState {
    reduce(SessionState::initial(), stream(system_init_json()))
}

// ---------------------------------------------------------------------------
// ErrorDuringExecution → Phase::Error
// ---------------------------------------------------------------------------

#[test]
fn result_error_during_execution_gives_error_phase() {
    let s0 = after_init();
    let s1 = reduce(s0, stream(&result_json("error_during_execution", false)));
    assert_eq!(s1.phase, Phase::Error);
}

// ---------------------------------------------------------------------------
// ErrorMaxTurns → Phase::Error
// ---------------------------------------------------------------------------

#[test]
fn result_error_max_turns_gives_error_phase() {
    let s0 = after_init();
    let s1 = reduce(s0, stream(&result_json("error_max_turns", false)));
    assert_eq!(s1.phase, Phase::Error);
}

// ---------------------------------------------------------------------------
// is_error=true on a success subtype still gives Phase::Error
// ---------------------------------------------------------------------------

#[test]
fn result_is_error_flag_gives_error_phase() {
    let s0 = after_init();
    let s1 = reduce(s0, stream(&result_json("success", true)));
    assert_eq!(s1.phase, Phase::Error);
}

// ---------------------------------------------------------------------------
// EndReason: cancel vs real error
// ---------------------------------------------------------------------------

/// A user-initiated interrupt (interrupt_requested set by core) turns the error
/// result into a calm Cancel: phase Done (groups under Done), end_reason
/// Cancelled, and the one-shot interrupt flag is consumed.
#[test]
fn interrupt_result_is_cancelled_not_error() {
    let mut s = after_init();
    s.interrupt_requested = true;
    let s1 = reduce(s, stream(&result_json("error_during_execution", false)));
    assert_eq!(s1.phase, Phase::Done, "user cancel is Done, not Error");
    assert_eq!(s1.end_reason, Some(EndReason::Cancelled));
    assert!(!s1.interrupt_requested, "interrupt flag consumed by the turn");
    assert_eq!(EndReason::Cancelled.text(), "Cancelled by the user");
}

/// A user interrupt while a gate is OPEN: the cancelled error result must still
/// resolve the turn (Done + Cancelled) AND drop the pending gate, rather than
/// getting held at Needs by the "don't clobber an open gate" guard. Regression
/// for the gate-stickiness fix (a pending gate normally survives a `result`, but
/// an interrupt genuinely ends the turn and declines the gate).
#[test]
fn interrupt_while_gate_open_cancels_and_clears_gate() {
    let mut s = reduce(
        after_init(),
        stream(
            r#"{"type":"control_request","request_id":"req-x","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#,
        ),
    );
    assert_eq!(s.phase, Phase::Needs);
    assert!(s.gate.is_some());
    s.interrupt_requested = true;
    let s1 = reduce(s, stream(&result_json("error_during_execution", false)));
    assert_eq!(s1.phase, Phase::Done, "cancelled turn is Done, not stuck Needs");
    assert_eq!(s1.end_reason, Some(EndReason::Cancelled));
    assert!(s1.gate.is_none(), "the abandoned gate must be cleared on cancel");
}

/// A genuine error (no interrupt) stays Error and carries a real reason.
#[test]
fn error_results_carry_a_reason() {
    let s_exec = reduce(
        after_init(),
        stream(&result_json("error_during_execution", false)),
    );
    assert_eq!(s_exec.phase, Phase::Error);
    assert_eq!(s_exec.end_reason, Some(EndReason::ExecutionError));

    let s_max = reduce(after_init(), stream(&result_json("error_max_turns", false)));
    assert_eq!(s_max.phase, Phase::Error);
    assert_eq!(s_max.end_reason, Some(EndReason::MaxTurns));

    let s_flag = reduce(after_init(), stream(&result_json("success", true)));
    assert_eq!(s_flag.phase, Phase::Error);
    assert_eq!(s_flag.end_reason, Some(EndReason::ExecutionError));
}

/// A successful result clears any reason; a new turn (assistant) after an error
/// also clears the stale reason.
#[test]
fn success_and_new_turn_clear_end_reason() {
    let s_ok = reduce(after_init(), stream(&result_json("success", false)));
    assert_eq!(s_ok.phase, Phase::Done);
    assert_eq!(s_ok.end_reason, None);

    // After an error, the next assistant turn clears the stale reason.
    let s_err = reduce(after_init(), stream(&result_json("error_max_turns", false)));
    assert_eq!(s_err.end_reason, Some(EndReason::MaxTurns));
    let s_next = reduce(s_err, stream(&assistant_json("msg-after-err")));
    assert_eq!(s_next.phase, Phase::Working);
    assert_eq!(s_next.end_reason, None, "new turn clears the stale reason");
}

// ---------------------------------------------------------------------------
// Out-of-order: Result before SystemInit — total, no panic
// ---------------------------------------------------------------------------

#[test]
fn result_before_system_init_does_not_panic() {
    let s0 = SessionState::initial();
    let s1 = reduce(s0, stream(&result_json("success", false)));
    // Must not panic; phase should be a valid variant (Done, Error, Starting, or Working)
    assert!(
        matches!(
            s1.phase,
            Phase::Done | Phase::Error | Phase::Starting | Phase::Working
        ),
        "unexpected phase after out-of-order result: {:?}",
        s1.phase
    );
}

// ---------------------------------------------------------------------------
// Out-of-order: Assistant before SystemInit — total, tokens non-negative
// ---------------------------------------------------------------------------

#[test]
fn assistant_before_system_init_does_not_panic() {
    let s0 = SessionState::initial();
    let s1 = reduce(s0, stream(&assistant_json("msg-oo")));
    // assistant_json has input=50, output=10, cache=0 → 60 tokens counted (no dedup yet)
    assert_eq!(s1.tokens, 60, "token count must reflect assistant usage");
    // context_pct must be valid
    assert!(s1.context_pct <= 100.0, "context_pct exceeded 100");
}

// ---------------------------------------------------------------------------
// Out-of-order: ControlRequest before SystemInit — total, no panic
// ---------------------------------------------------------------------------

#[test]
fn control_request_before_system_init_does_not_panic() {
    let s0 = SessionState::initial();
    let s1 = reduce(
        s0,
        stream(
            r#"{"type":"control_request","request_id":"req-oo","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#,
        ),
    );
    // Must not panic; phase must be a valid variant
    let _ = s1.phase;
}

// ---------------------------------------------------------------------------
// Double-Result: second result after Done must not panic, invariants hold
// ---------------------------------------------------------------------------

#[test]
fn double_result_does_not_panic_or_go_negative() {
    let s0 = after_init();
    let s1 = reduce(s0, stream(&result_json("success", false)));
    assert_eq!(s1.phase, Phase::Done);

    let s2 = reduce(s1, stream(&result_json("success", false)));
    // reduce_result overwrites (not adds) tokens with authoritative usage:
    // input=50 + output=10 + cache=0 = 60; same on both results
    assert_eq!(s2.tokens, 60, "double-result token count must remain 60");
    assert!(
        s2.context_pct <= 100.0,
        "context_pct exceeded 100 on double result: {}",
        s2.context_pct
    );
}

// ---------------------------------------------------------------------------
// Unknown StreamMsg: state preserved (Working), no panic
// ---------------------------------------------------------------------------

#[test]
fn unknown_stream_msg_is_total() {
    let s0 = after_init();
    let s1 = reduce(
        s0,
        stream(r#"{"type":"future_unrecognized_type","payload":42}"#),
    );
    // Unknown events must be ignored; Working phase must be preserved
    assert_eq!(s1.phase, Phase::Working);
}

// ---------------------------------------------------------------------------
// Stale phase: non-init stream events preserve Stale (totality no-ops)
// ---------------------------------------------------------------------------

#[test]
fn stale_preserved_by_assistant_stream() {
    let mut s = after_init();
    s.phase = Phase::Stale;
    // Spec: assistant while Stale is a totality no-op that preserves the phase.
    let s1 = reduce(s, stream(&assistant_json("msg-stale")));
    assert_eq!(
        s1.phase,
        Phase::Stale,
        "assistant must not escape Stale phase"
    );
    // Stale is a no-op: assistant usage must NOT be counted (tokens stay at 0)
    assert_eq!(s1.tokens, 0, "Stale no-op must not accumulate tokens");
    assert!(s1.context_pct <= 100.0);
}

// ---------------------------------------------------------------------------
// Detached phase: out-of-order unknown event preserves Detached
// ---------------------------------------------------------------------------

#[test]
fn detached_phase_preserved_on_unknown_event() {
    let mut s = after_init();
    s.phase = Phase::Detached;
    let s1 = reduce(s, stream(r#"{"type":"unknown_future","x":1}"#));
    assert_eq!(s1.phase, Phase::Detached);
}
