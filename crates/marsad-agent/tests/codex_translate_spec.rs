//! Tests for codex notification → stream-json translation. Drives shapes through
//! the FULL pipeline (translate → parse_stream_line → reduce) so a wire drift or a
//! reducer-contract break fails here. Item shapes are schema-exact for 0.142.5.

use marsad_agent::codex_translate::codex_notification_to_stream_json as tr;
use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_proto::{parse_stream_line, StreamMsg};

const SID: &str = "thread-1";

fn feed(state: SessionState, method: &str, params: serde_json::Value) -> SessionState {
    match tr(method, &params, SID) {
        Some(sj) => {
            let line = serde_json::to_string(&sj).unwrap();
            match parse_stream_line(&line) {
                Ok(m) => reduce(state, Event::Stream(m)),
                Err(e) => panic!("translated stream-json failed to parse: {e}\n{line}"),
            }
        }
        None => state,
    }
}

#[test]
fn agent_message_item_renders_as_assistant_text() {
    let p = serde_json::json!({"item":{"type":"agentMessage","id":"a1","text":"Hello from codex."}});
    let sj = tr("item/completed", &p, SID).expect("agentMessage translates");
    match parse_stream_line(&serde_json::to_string(&sj).unwrap()) {
        Ok(StreamMsg::Assistant(_)) => {}
        other => panic!("expected Assistant, got {other:?}"),
    }
    let st = feed(SessionState::initial(), "item/completed", p);
    assert_eq!(st.phase, Phase::Working);
}

#[test]
fn command_execution_item_is_a_tool_use() {
    let p = serde_json::json!({"item":{"type":"commandExecution","id":"c1","command":"echo hi","cwd":"/tmp","status":"completed"}});
    let sj = tr("item/completed", &p, SID).unwrap();
    assert_eq!(sj["message"]["content"][0]["type"], "tool_use");
    assert_eq!(sj["message"]["content"][0]["name"], "shell");
    assert_eq!(sj["message"]["content"][0]["input"]["command"], "echo hi");
}

#[test]
fn file_change_item_is_a_tool_use() {
    let p = serde_json::json!({"item":{"type":"fileChange","id":"f1","changes":[{"path":"a.rs"}],"status":"completed"}});
    let sj = tr("item/completed", &p, SID).unwrap();
    assert_eq!(sj["message"]["content"][0]["name"], "apply_patch");
}

#[test]
fn reasoning_item_is_thinking() {
    let p = serde_json::json!({"item":{"type":"reasoning","id":"r1","summary":[{"type":"text","text":"Thinking about it."}],"content":[]}});
    let sj = tr("item/completed", &p, SID).unwrap();
    assert_eq!(sj["message"]["content"][0]["type"], "thinking");
    assert_eq!(sj["message"]["content"][0]["thinking"], "Thinking about it.");
}

#[test]
fn user_message_and_deltas_are_skipped() {
    assert!(tr("item/completed", &serde_json::json!({"item":{"type":"userMessage","id":"u1","content":[]}}), SID).is_none());
    assert!(tr("item/agentMessage/delta", &serde_json::json!({"delta":"x","itemId":"a1"}), SID).is_none());
    assert!(tr("item/reasoning/textDelta", &serde_json::json!({"delta":"x"}), SID).is_none());
}

#[test]
fn plan_updated_populates_plan_entries() {
    let p = serde_json::json!({"threadId":SID,"turnId":"t","plan":[{"content":"step 1","status":"pending"},{"content":"step 2","status":"in_progress"}]});
    let st = feed(SessionState::initial(), "turn/plan/updated", p);
    assert_eq!(st.plan_entries.len(), 2);
    assert_eq!(st.plan_entries[0].content, "step 1");
}

#[test]
fn token_usage_maps_to_usage_update() {
    let p = serde_json::json!({"threadId":SID,"turnId":"t","tokenUsage":{"total":{"inputTokens":100,"outputTokens":50},"modelContextWindow":200000}});
    let sj = tr("thread/tokenUsage/updated", &p, SID).unwrap();
    assert_eq!(sj["subtype"], "usage_update");
    assert_eq!(sj["context_used"], 150);
    assert_eq!(sj["context_size"], 200000);
}

#[test]
fn turn_completed_success_is_not_error_failed_is() {
    let ok = tr("turn/completed", &serde_json::json!({"threadId":SID,"turn":{"id":"t","status":"completed"}}), SID).unwrap();
    assert_eq!(ok["is_error"], false);
    let fail = tr("turn/completed", &serde_json::json!({"threadId":SID,"turn":{"id":"t","status":"failed","error":{"message":"boom"}}}), SID).unwrap();
    assert_eq!(fail["is_error"], true);
    assert_eq!(fail["result"], "boom");
    // interrupted is a calm cancel, not an error.
    let interrupted = tr("turn/completed", &serde_json::json!({"threadId":SID,"turn":{"id":"t","status":"interrupted"}}), SID).unwrap();
    assert_eq!(interrupted["is_error"], false);
}

#[test]
fn compacted_is_a_boundary() {
    let sj = tr("thread/compacted", &serde_json::json!({"threadId":SID}), SID).unwrap();
    assert_eq!(sj["subtype"], "compact_boundary");
}
