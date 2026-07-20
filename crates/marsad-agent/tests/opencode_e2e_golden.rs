//! Golden end-to-end test for the opencode/ACP path.
//!
//! Drives REAL opencode 1.18 message shapes (captured from a live session) through
//! the FULL Marsad pipeline — parse_acp_line → extract_update → acp_translate →
//! marsad_proto::parse_stream_line → marsad_events::reduce — and asserts the
//! resulting session state. This is the guard the earlier unit tests lacked: they
//! used guessed wire shapes (`type`/`toolName`) so real opencode (which uses
//! `sessionUpdate`/`toolCallId`/`title`, and `stopReason:"end_turn"`) silently
//! produced an empty transcript + false "Error". A wire-shape drift fails HERE.

use marsad_agent::acp::{extract_update, parse_acp_line, AcpMessage};
use marsad_agent::acp_translate::{
    acp_gate_routing_id, acp_permission_to_control_request_event, acp_prompt_result_to_result,
    acp_update_to_stream_json,
};
use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_proto::{parse_stream_line, StreamMsg};

const SID: &str = "ses_real";

/// Translate an opencode `session/update` line and reduce it into `state`.
fn feed_update(state: SessionState, raw: &str) -> SessionState {
    let msg = parse_acp_line(raw).expect("real opencode line must parse as ACP");
    let AcpMessage::Notification { params, .. } = msg else {
        panic!("expected a session/update notification");
    };
    let update = extract_update(&params).expect("notification has an update");
    match acp_update_to_stream_json(update, SID, "") {
        Some(sj) => {
            let line = serde_json::to_string(&sj).unwrap();
            match parse_stream_line(&line) {
                Ok(m) => reduce(state, Event::Stream(m)),
                Err(e) => panic!("translated stream-json failed to parse: {e}\n{line}"),
            }
        }
        // A real, known update translating to None means the sessionUpdate wiring
        // is broken (the bug where the whole transcript vanished).
        None => panic!("real opencode update translated to None (dropped): {raw}"),
    }
}

#[test]
fn real_opencode_turn_renders_text_tool_gate_and_ends_without_error() {
    let mut state = SessionState::initial();

    // 1. agent_message_chunk → assistant text. If the sessionUpdate wiring is
    //    wrong this panics in feed_update (translated to None), and phase would
    //    never leave Starting.
    state = feed_update(
        state,
        r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_real","update":{"sessionUpdate":"agent_message_chunk","messageId":"msg_1","content":{"type":"text","text":"Looking at the codebase."}}}}"#,
    );
    assert_eq!(state.phase, Phase::Working, "assistant chunk → Working (translation reached the reducer)");

    // 2. tool_call → assistant tool_use whose NAME is the human title ("glob"),
    //    never the old "unknown_tool".
    let tool_line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_real","update":{"sessionUpdate":"tool_call","toolCallId":"toolu_1","title":"glob","kind":"search","status":"pending","rawInput":{"pattern":"*"}}}}"#;
    let tmsg = parse_acp_line(tool_line).unwrap();
    let AcpMessage::Notification { params, .. } = tmsg else { panic!("notif") };
    let sj = acp_update_to_stream_json(extract_update(&params).unwrap(), SID, "").expect("tool_call translates");
    match parse_stream_line(&serde_json::to_string(&sj).unwrap()).unwrap() {
        StreamMsg::Assistant(m) => {
            let has_glob = m.message.content.iter().any(|b| matches!(
                b, marsad_proto::ContentBlock::ToolUse { name, .. } if name == "glob"
            ));
            assert!(has_glob, "tool_use name must be the title 'glob', not unknown_tool");
        }
        other => panic!("tool_call must become an Assistant message, got {other:?}"),
    }
    state = reduce(state, Event::Stream(parse_stream_line(&serde_json::to_string(&sj).unwrap()).unwrap()));

    // 3. session/request_permission → a Needs gate carrying the 3 ACP options.
    let perm = r#"{"jsonrpc":"2.0","id":0,"method":"session/request_permission","params":{"sessionId":"ses_real","toolCall":{"toolCallId":"toolu_perm","title":"/work/x","kind":"other","rawInput":{"filepath":"/work/x"}},"options":[{"optionId":"once","name":"Allow once","kind":"allow_once"},{"optionId":"always","name":"Always allow","kind":"allow_always"},{"optionId":"reject","name":"Reject","kind":"reject_once"}]}}"#;
    let AcpMessage::Request { id, params, .. } = parse_acp_line(perm).unwrap() else {
        panic!("expected an agent→client Request");
    };
    let rid = acp_gate_routing_id(SID, &id.to_string(), &params);
    assert_ne!(rid, "0", "gate routing id must be unique, never the raw ACP id 0");
    let ev = acp_permission_to_control_request_event(&rid, &params);
    state = reduce(
        state,
        Event::Stream(parse_stream_line(&serde_json::to_string(&ev).unwrap()).expect("gate event parses")),
    );
    assert_eq!(state.phase, Phase::Needs, "request_permission → Needs");
    match state.gate.as_ref().expect("gate must be set") {
        marsad_gates::Gate::Permission { permission_suggestions, request_id, .. } => {
            assert_eq!(permission_suggestions.len(), 3, "3 ACP options carried to the gate");
            assert_eq!(request_id, &rid, "gate id = the unique routing id");
        }
        other => panic!("expected a Permission gate, got {other:?}"),
    }

    // 4. prompt result end_turn → a SUCCESS result, never an error.
    let result = serde_json::json!({"stopReason":"end_turn","usage":{"inputTokens":2,"outputTokens":855}});
    let done = acp_prompt_result_to_result(&result);
    assert_eq!(done["is_error"], false, "end_turn must not be flagged as an error");
    match parse_stream_line(&serde_json::to_string(&done).unwrap()).unwrap() {
        StreamMsg::Result(r) => assert!(!r.is_error, "end_turn → Result.is_error == false"),
        other => panic!("prompt result must parse as StreamMsg::Result, got {other:?}"),
    }
}
