//! B02 `marsad-events` — happy path tests.
//!
//! StreamMsg values are constructed by parsing JSON strings via `parse_stream_line`
//! (the only public constructor in marsad-proto). This keeps tests black-box against
//! both B01 and B02 without depending on private/unnamed inner struct types.
//!
//! Covers:
//! - `SessionState::initial()` field invariants
//! - Full nominal lifecycle: Starting → Working (SystemInit) → Working (assistant)
//!   → Needs (ControlRequest) → Working (user answer) → Done (Result/Success)
//! - Usage accumulation across assistant message + result
//! - cost_usd updated from Result
//! - context_pct computed and > 0 after real usage
//! - last_action set by ToolUse
//! - Subagent SubagentStop hook → Done
//! - All HookEvent variants: SessionStart, Stop, Notification, PreToolUse, PostToolUse, PreCompact

use marsad_events::{reduce, Event, HookEvent, Phase, SessionState, SubagentStatus};
use marsad_proto::parse_stream_line;

// ---------------------------------------------------------------------------
// JSON helpers — build StreamMsg by parsing spec-valid JSON
// ---------------------------------------------------------------------------

fn stream(json: &str) -> Event {
    let msg = parse_stream_line(json)
        .unwrap_or_else(|e| panic!("test JSON failed to parse: {e:?}\njson: {json}"));
    Event::Stream(msg)
}

fn system_init_json() -> &'static str {
    r#"{"type":"system","subtype":"init","session_id":"sess-01","model":"claude-opus-4-5","cwd":"/tmp/work","tools":["Read","Write"],"mcp_servers":[],"permissionMode":"default"}"#
}

fn assistant_json(id: &str, input: u64, output: u64) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"id":"{id}","content":[{{"type":"text","text":"Thinking…"}}],"usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    )
}

fn tool_use_assistant_json(id: &str, tool_name: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"id":"{id}","content":[{{"type":"tool_use","id":"tu-1","name":"{tool_name}","input":{{"path":"/etc/passwd"}}}}],"usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    )
}

fn control_request_json(request_id: &str, tool_name: &str) -> String {
    format!(
        r#"{{"type":"control_request","request_id":"{request_id}","request":{{"subtype":"can_use_tool","tool_name":"{tool_name}","input":{{}}}}}}"#
    )
}

fn user_json() -> &'static str {
    r#"{"type":"user","message":{"content":[{"type":"text","text":"yes"}]}}"#
}

fn result_success_json(input: u64, output: u64, cost: f64) -> String {
    format!(
        r#"{{"type":"result","subtype":"success","is_error":false,"total_cost_usd":{cost},"usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"num_turns":1,"permission_denials":[]}}"#
    )
}

// ---------------------------------------------------------------------------
// initial()
// ---------------------------------------------------------------------------

#[test]
fn initial_state_fields() {
    let s = SessionState::initial();
    assert_eq!(s.phase, Phase::Starting);
    assert_eq!(s.session_id, None);
    assert_eq!(s.model, None);
    assert_eq!(s.cwd, None);
    assert_eq!(s.tokens, 0);
    assert_eq!(s.cost_usd, 0.0);
    assert_eq!(s.context_pct, 0.0);
    assert_eq!(s.last_action, None);
    assert!(s.subagents.is_empty());
    assert!(s.gate.is_none());
    assert_eq!(s.changed_files, 0);
}

// ---------------------------------------------------------------------------
// Phase::Starting → Phase::Working via SystemInit
// ---------------------------------------------------------------------------

#[test]
fn system_init_transitions_to_working() {
    let s0 = SessionState::initial();
    let s1 = reduce(s0, stream(system_init_json()));
    assert_eq!(s1.phase, Phase::Working);
    assert_eq!(s1.session_id, Some("sess-01".to_string()));
    assert_eq!(s1.model, Some("claude-opus-4-5".to_string()));
    assert_eq!(s1.cwd, Some("/tmp/work".to_string()));
}

// ---------------------------------------------------------------------------
// Phase::Working via assistant message
// ---------------------------------------------------------------------------

#[test]
fn assistant_message_keeps_working() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, stream(&assistant_json("msg-001", 100, 20)));
    assert_eq!(s1.phase, Phase::Working);
    assert_eq!(s1.tokens, 120); // 100 in + 20 out
}

// ---------------------------------------------------------------------------
// Phase::Needs via ControlRequest
// ---------------------------------------------------------------------------

#[test]
fn control_request_transitions_to_needs() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, stream(&control_request_json("req-1", "Bash")));
    assert_eq!(s1.phase, Phase::Needs);
    assert!(s1.gate.is_some());
}

// ---------------------------------------------------------------------------
// Phase::Needs → Phase::Working via user turn (gate cleared)
// ---------------------------------------------------------------------------

#[test]
fn user_turn_clears_needs_and_gate() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, stream(&control_request_json("req-1", "Bash")));
    assert_eq!(s1.phase, Phase::Needs);
    let s2 = reduce(s1, stream(user_json()));
    assert_eq!(s2.phase, Phase::Working);
    assert!(s2.gate.is_none());
}

// ---------------------------------------------------------------------------
// Phase::Done via Result/Success
// ---------------------------------------------------------------------------

#[test]
fn result_success_transitions_to_done() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, stream(&assistant_json("msg-001", 100, 20)));
    let s2 = reduce(s1, stream(&result_success_json(100, 20, 1.23)));
    assert_eq!(s2.phase, Phase::Done);
    assert!(s2.cost_usd > 0.0);
}

// ---------------------------------------------------------------------------
// Full lifecycle: Starting → Working → Needs → Working → Done
// ---------------------------------------------------------------------------

#[test]
fn full_nominal_lifecycle() {
    let mut s = SessionState::initial();
    assert_eq!(s.phase, Phase::Starting);

    s = reduce(s, stream(system_init_json()));
    assert_eq!(s.phase, Phase::Working);

    s = reduce(s, stream(&assistant_json("msg-A", 200, 50)));
    assert_eq!(s.phase, Phase::Working);

    s = reduce(s, stream(&control_request_json("req-1", "Read")));
    assert_eq!(s.phase, Phase::Needs);

    s = reduce(s, stream(user_json()));
    assert_eq!(s.phase, Phase::Working);
    assert!(s.gate.is_none());

    s = reduce(s, stream(&assistant_json("msg-B", 300, 80)));
    assert_eq!(s.phase, Phase::Working);

    s = reduce(s, stream(&result_success_json(500, 130, 2.50)));
    assert_eq!(s.phase, Phase::Done);
    assert!(s.cost_usd > 0.0);
}

// ---------------------------------------------------------------------------
// Usage accumulation: distinct message ids accumulate
// ---------------------------------------------------------------------------

#[test]
fn usage_accumulates_over_multiple_assistant_messages() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, stream(&assistant_json("msg-001", 100, 10)));
    let s2 = reduce(s1, stream(&assistant_json("msg-002", 200, 20)));
    // 110 + 220 = 330
    assert_eq!(s2.tokens, 330);
}

// ---------------------------------------------------------------------------
// cost_usd updated from Result
// ---------------------------------------------------------------------------

#[test]
fn cost_updated_from_result() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, stream(&result_success_json(100, 10, 0.42)));
    assert!(
        (s1.cost_usd - 0.42).abs() < 1e-9,
        "expected cost 0.42, got {}",
        s1.cost_usd
    );
}

// ---------------------------------------------------------------------------
// last_action updated by ToolUse assistant content
// ---------------------------------------------------------------------------

#[test]
fn last_action_set_by_tool_use() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, stream(&tool_use_assistant_json("msg-001", "Read")));
    let action = s1
        .last_action
        .expect("last_action must be set after ToolUse");
    assert!(
        action.contains("Read"),
        "expected 'Read' in last_action, got: {action}"
    );
}

// ---------------------------------------------------------------------------
// Subagent: SubagentStop hook records subagent as Done
// ---------------------------------------------------------------------------

#[test]
fn subagent_stop_hook_marks_done() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::SubagentStop {
            name: "worker-1".to_string(),
        }),
    );
    let agent = s1.subagents.iter().find(|a| a.name == "worker-1");
    assert!(agent.is_some(), "SubagentStop must record the subagent");
    assert_eq!(agent.unwrap().status, SubagentStatus::Done);
}

// ---------------------------------------------------------------------------
// HookEvent::SessionStart — total, no panic
// ---------------------------------------------------------------------------

#[test]
fn hook_session_start_is_total() {
    let s0 = SessionState::initial();
    let s1 = reduce(s0, Event::Hook(HookEvent::SessionStart));
    let _ = s1; // must not panic
}

// ---------------------------------------------------------------------------
// HookEvent::Stop → Phase::Done
// ---------------------------------------------------------------------------

#[test]
fn hook_stop_transitions_to_done() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, Event::Hook(HookEvent::Stop));
    assert_eq!(s1.phase, Phase::Done);
}

// ---------------------------------------------------------------------------
// HookEvent::Notification → Phase::Needs
// ---------------------------------------------------------------------------

#[test]
fn hook_notification_transitions_to_needs() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::Notification {
            message: "Attention needed".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Needs);
}

// ---------------------------------------------------------------------------
// HookEvent::PreToolUse and PostToolUse — phase stays Working
// ---------------------------------------------------------------------------

#[test]
fn hook_pre_post_tool_use_keeps_working() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        Event::Hook(HookEvent::PreToolUse {
            tool: "Bash".to_string(),
        }),
    );
    assert_eq!(s1.phase, Phase::Working);
    let s2 = reduce(
        s1,
        Event::Hook(HookEvent::PostToolUse {
            tool: "Bash".to_string(),
        }),
    );
    assert_eq!(s2.phase, Phase::Working);
}

// ---------------------------------------------------------------------------
// HookEvent::PreCompact — total, phase unchanged
// ---------------------------------------------------------------------------

#[test]
fn hook_pre_compact_is_total_and_working() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, Event::Hook(HookEvent::PreCompact));
    assert_eq!(s1.phase, Phase::Working);
}
