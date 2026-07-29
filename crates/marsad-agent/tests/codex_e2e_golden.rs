//! Golden end-to-end test for the codex app-server path.
//!
//! Drives REAL codex 0.142.5 wire shapes (captured live / from the generated
//! schema) through the FULL pipeline the reader uses — codex_appserver::parse_line
//! → codex_translate → marsad_proto::parse_stream_line → marsad_events::reduce —
//! and asserts the resulting session state. This is the seam guard: a wire-shape
//! drift (codex omitting `jsonrpc`, an item type, or the approval shape) fails HERE.
//!
//! The one thing this can't cover is a LIVE model turn (needs the gateway key), but
//! it exercises every translation + the approval/decision round-trip on real bytes.

use marsad_agent::acp::AcpMessage;
use marsad_agent::codex_appserver as cx;
use marsad_agent::codex_translate as ct;
use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_proto::{parse_stream_line, StreamMsg};

const SID: &str = "thread-real";

/// Feed a codex NOTIFICATION line through parse_line → translate → reduce.
fn feed_notif(state: SessionState, line: &str) -> SessionState {
    let AcpMessage::Notification { method, params } =
        cx::parse_line(line).expect("codex line parses")
    else {
        panic!("expected a notification: {line}");
    };
    match ct::codex_notification_to_stream_json(&method, &params, SID) {
        Some(sj) => match parse_stream_line(&serde_json::to_string(&sj).unwrap()) {
            Ok(m) => reduce(state, Event::Stream(m)),
            Err(e) => panic!("translated stream-json failed to parse: {e}"),
        },
        None => state,
    }
}

#[test]
fn real_codex_turn_renders_text_gate_and_ends_clean() {
    let mut state = SessionState::initial();

    // 1. thread/started (no jsonrpc field — the codex-specific quirk).
    let started = r#"{"method":"thread/started","params":{"thread":{"id":"thread-real","sessionId":"thread-real"}}}"#;
    assert!(matches!(cx::parse_line(started).unwrap(), AcpMessage::Notification { .. }));

    // 2. assistant text (item/completed agentMessage) → transcript + Working.
    state = feed_notif(
        state,
        r#"{"method":"item/completed","params":{"threadId":"thread-real","turnId":"t1","completedAtMs":1,"item":{"type":"agentMessage","id":"a1","text":"I'll run a command."}}}"#,
    );
    assert_eq!(state.phase, Phase::Working, "assistant text drives Working");

    // 3. a REAL command-approval ServerRequest (no jsonrpc) → gate.
    let approval_line = r#"{"id":0,"method":"item/commandExecution/requestApproval","params":{"itemId":"cmd-1","threadId":"thread-real","turnId":"t1","command":"echo hi","cwd":"/tmp","startedAtMs":1}}"#;
    let AcpMessage::Request { id, method, params } = cx::parse_line(approval_line).unwrap() else {
        panic!("approval must parse as a Request");
    };
    assert!(cx::is_command_approval(&method));
    let gate_id = ct::codex_gate_routing_id(SID, &id.to_string(), &params);
    assert_eq!(gate_id, "cmd-1", "routing id = unique itemId");
    let gate_event = ct::codex_approval_to_control_request(&gate_id, &method, &params);
    state = match parse_stream_line(&serde_json::to_string(&gate_event).unwrap()) {
        Ok(m) => reduce(state, Event::Stream(m)),
        Err(e) => panic!("gate control_request failed to parse: {e}"),
    };
    assert_eq!(state.phase, Phase::Needs, "an approval request blocks on a gate");
    assert!(state.gate.is_some(), "the gate is captured in state");

    // 4. the decision reply encodes correctly (accept), addressed to the raw id 0.
    let decision = cx::approval_decision(true, false);
    assert_eq!(decision["decision"], "accept");
    let reply = marsad_agent::acp::encode_response(id.clone(), decision);
    assert!(reply.contains("\"id\":0"), "reply targets the raw JSON-RPC id 0: {reply}");
    assert!(reply.contains("\"decision\":\"accept\""));

    // 5. turn/completed (success) → not an error, turn resolves.
    state = feed_notif(
        state,
        r#"{"method":"turn/completed","params":{"threadId":"thread-real","turn":{"id":"t1","items":[],"status":"completed"}}}"#,
    );
    assert!(!matches!(state.phase, Phase::Error), "a completed turn is not an error");
}

#[test]
fn real_codex_plan_and_usage_reduce() {
    let mut state = SessionState::initial();
    state = feed_notif(
        state,
        r#"{"method":"turn/plan/updated","params":{"threadId":"thread-real","turnId":"t1","plan":[{"content":"step one","status":"pending"}]}}"#,
    );
    assert_eq!(state.plan_entries.len(), 1);
    assert_eq!(state.plan_entries[0].content, "step one");

    // token usage doesn't error and is accepted by the reducer.
    let _ = feed_notif(
        state,
        r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"thread-real","turnId":"t1","tokenUsage":{"total":{"inputTokens":10,"outputTokens":5},"modelContextWindow":200000}}}"#,
    );
}
