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

/// Control request with a caller-supplied `input` object (raw JSON).
fn control_request_json_with_input(request_id: &str, tool_name: &str, input: &str) -> String {
    format!(
        r#"{{"type":"control_request","request_id":"{request_id}","request":{{"subtype":"can_use_tool","tool_name":"{tool_name}","input":{input}}}}}"#
    )
}

/// Extract the gate's command + explanation for assertions.
fn gate_command_explanation(s: &SessionState) -> (String, String) {
    match s.gate.as_ref().expect("gate present") {
        marsad_gates::Gate::Permission {
            command,
            explanation,
            ..
        } => (command.clone(), explanation.clone()),
        other => panic!("expected Permission gate, got {other:?}"),
    }
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

#[test]
fn bash_gate_shows_the_actual_command_not_the_tool_name() {
    // Regression: the gate used to display the tool name ("Bash") instead of the
    // shell command the user is being asked to approve.
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        stream(&control_request_json_with_input(
            "req-1",
            "Bash",
            r#"{"command":"rm -rf /tmp/x","description":"Delete the temp dir"}"#,
        )),
    );
    let (command, explanation) = gate_command_explanation(&s1);
    assert_eq!(command, "rm -rf /tmp/x");
    assert_eq!(explanation, "Delete the temp dir");
}

#[test]
fn file_tool_gate_shows_tool_and_path() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        stream(&control_request_json_with_input(
            "req-2",
            "Read",
            r#"{"file_path":"/etc/hosts"}"#,
        )),
    );
    let (command, explanation) = gate_command_explanation(&s1);
    assert_eq!(command, "Read /etc/hosts");
    // No description in input → generic explanation.
    assert_eq!(explanation, "Tool use request: Read");
}

#[test]
fn web_tool_gate_shows_tool_and_url() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        stream(&control_request_json_with_input(
            "req-3",
            "WebFetch",
            r#"{"url":"https://example.com"}"#,
        )),
    );
    let (command, _) = gate_command_explanation(&s1);
    assert_eq!(command, "WebFetch https://example.com");
}

#[test]
fn unknown_tool_gate_falls_back_to_tool_name() {
    // Empty input (and an empty command string) → bare tool name, never blank.
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        stream(&control_request_json_with_input(
            "req-4",
            "SomeMcpTool",
            r#"{"command":"","other":1}"#,
        )),
    );
    let (command, _) = gate_command_explanation(&s1);
    assert_eq!(command, "SomeMcpTool");
}

fn elicitation_request_json(request_id: &str, server: &str, message: &str) -> String {
    let msg = serde_json::Value::String(message.to_string()).to_string();
    format!(
        r#"{{"type":"control_request","request_id":"{request_id}","request":{{"subtype":"elicitation","mcp_server_name":"{server}","message":{msg},"mode":"form","requested_schema":{{"type":"object","properties":{{}}}}}}}}"#
    )
}

#[test]
fn elicitation_gate_extracts_fenced_command_and_keeps_message() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let message = "**Command requires approval**\n\nPattern matched: `*git push*`\n\n```\ncd /proj && git push\n```\n\nAllow it to run?";
    let s1 = reduce(
        s0,
        stream(&elicitation_request_json("eli-1", "hares", message)),
    );
    assert_eq!(s1.phase, Phase::Needs);
    let (command, explanation) = gate_command_explanation(&s1);
    assert_eq!(command, "cd /proj && git push");
    assert_eq!(explanation, "Approval requested by MCP server: hares");
}

#[test]
fn elicitation_gate_without_fence_uses_first_nonempty_line() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        stream(&elicitation_request_json("eli-2", "srv", "\n  Approve writing to disk?  \nmore")),
    );
    let (command, _) = gate_command_explanation(&s1);
    assert_eq!(command, "Approve writing to disk?");
}

#[test]
fn elicitation_gate_empty_message_is_blank_not_panicking() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(s0, stream(&elicitation_request_json("eli-3", "srv", "")));
    let (command, _) = gate_command_explanation(&s1);
    assert_eq!(command, "");
}

#[test]
fn elicitation_gate_unterminated_fence_falls_back_to_line() {
    // Opening fence but no closing fence → fall through to first non-empty line.
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        stream(&elicitation_request_json("eli-4", "srv", "Run this?\n```\nrm -rf x")),
    );
    let (command, _) = gate_command_explanation(&s1);
    assert_eq!(command, "Run this?");
}

#[test]
fn elicitation_gate_fence_with_only_whitespace_falls_back() {
    // Fenced block present but empty/whitespace → fall through to first line.
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        stream(&elicitation_request_json("eli-5", "srv", "Header\n```\n   \n```")),
    );
    let (command, _) = gate_command_explanation(&s1);
    assert_eq!(command, "Header");
}

#[test]
fn elicitation_gate_fence_without_following_newline_falls_back() {
    // An opening ``` with no newline after it (no real block) → first-line fallback.
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        stream(&elicitation_request_json("eli-6", "srv", "Allow it? ```inline")),
    );
    let (command, _) = gate_command_explanation(&s1);
    assert_eq!(command, "Allow it? ```inline");
}

#[test]
fn empty_primary_arg_falls_back_to_tool_name() {
    // A present-but-empty primary arg (e.g. file_path:"") must not produce a
    // trailing-space "Read " — it falls through to the bare tool name.
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let s1 = reduce(
        s0,
        stream(&control_request_json_with_input(
            "req-5",
            "Read",
            r#"{"file_path":""}"#,
        )),
    );
    let (command, _) = gate_command_explanation(&s1);
    assert_eq!(command, "Read");
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

// ---------------------------------------------------------------------------
// model_usage — populated from result event's modelUsage map
// ---------------------------------------------------------------------------

/// A result event with modelUsage populates state.model_usage (replace).
#[test]
fn result_with_model_usage_populates_state() {
    let s0 = reduce(SessionState::initial(), stream(system_init_json()));
    let result_json = r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.268,"usage":{"input_tokens":11,"output_tokens":155,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"num_turns":1,"permission_denials":[],"modelUsage":{"claude-sonnet-4.6[1m]":{"inputTokens":11,"outputTokens":155,"costUSD":0.268}}}"#;
    let s1 = reduce(s0, stream(result_json));
    assert!(!s1.model_usage.is_empty(), "model_usage must be populated from result");
    let mu = s1.model_usage.get("claude-sonnet-4.6[1m]").expect("key must exist");
    assert_eq!(mu.input_tokens, 11);
    assert_eq!(mu.output_tokens, 155);
    assert!((mu.cost_usd - 0.268).abs() < 1e-9);
}
