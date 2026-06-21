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

use marsad_events::{reduce, Event, Phase, SessionState};
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
            r#"{"type":"control","subtype":"can_use_tool","tool_name":"Bash","input":{},"request_id":"req-oo"}"#,
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
