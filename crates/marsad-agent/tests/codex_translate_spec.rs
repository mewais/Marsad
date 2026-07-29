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

// ── Phase B: approval requests → control_request gates ──────────────────────
use marsad_agent::codex_translate::{codex_approval_to_control_request, codex_gate_routing_id};

#[test]
fn command_approval_becomes_a_can_use_tool_gate() {
    let params = serde_json::json!({
        "itemId":"item-9","threadId":"t","turnId":"u","command":"rm -rf /tmp/x","cwd":"/tmp","reason":"delete files"
    });
    let gid = codex_gate_routing_id(SID, "0", &params);
    assert_eq!(gid, "item-9", "routing id = unique itemId");
    let ev = codex_approval_to_control_request(&gid, "item/commandExecution/requestApproval", &params);
    assert_eq!(ev["request"]["subtype"], "can_use_tool");
    assert_eq!(ev["request"]["tool_name"], "shell");
    assert_eq!(ev["request"]["input"]["command"], "rm -rf /tmp/x");
    assert_eq!(ev["request_id"], "item-9");
    // Round-trips through the proto control_request parser (the gate contract).
    match parse_stream_line(&serde_json::to_string(&ev).unwrap()) {
        Ok(StreamMsg::ControlRequest(_)) => {}
        other => panic!("expected ControlRequest, got {other:?}"),
    }
}

#[test]
fn file_change_approval_is_apply_patch_gate() {
    let params = serde_json::json!({"itemId":"item-1","changes":{"a.rs":{"update":"..."}}});
    let ev = codex_approval_to_control_request("item-1", "item/fileChange/requestApproval", &params);
    assert_eq!(ev["request"]["tool_name"], "apply_patch");
    assert!(ev["request"]["input"].get("changes").is_some());
}

#[test]
fn gate_routing_id_falls_back_without_item_id() {
    let params = serde_json::json!({"command":"ls"});
    assert_eq!(codex_gate_routing_id("sid-7", "3", &params), "sid-7:codex:3");
}

// ── Phase D: collaborationMode/list → modes_update ──────────────────────────
#[test]
fn collab_mode_list_becomes_modes_update() {
    use marsad_agent::codex_translate::codex_modes_from_collab_list;
    let result = serde_json::json!({"data":[{"mode":"build","settings":{}},{"mode":"plan","settings":{}}]});
    let mu = codex_modes_from_collab_list(&result, SID, Some("build"));
    assert_eq!(mu["subtype"], "modes_update");
    assert_eq!(mu["current_mode"], "build");
    let st = feed(SessionState::initial(), "ignored", serde_json::json!({}));
    let _ = st; // silence unused in this focused test
    // round-trips into a ModesUpdate the reducer accepts
    match parse_stream_line(&serde_json::to_string(&mu).unwrap()) {
        Ok(StreamMsg::ModesUpdate(_)) => {}
        other => panic!("expected ModesUpdate, got {other:?}"),
    }
    let ids: Vec<String> = mu["available_modes"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, vec!["build","plan"]);
}

// ── Gap-closing: questions, elicitation, permissions ────────────────────────
use marsad_agent::codex_translate::{
    codex_question_to_control_request, codex_elicitation_to_control_request,
    codex_answers_by_id, codex_permissions_response,
};

#[test]
fn request_user_input_becomes_askuserquestion_gate() {
    let params = serde_json::json!({
        "itemId":"q-1","threadId":"t","turnId":"u",
        "questions":[{"id":"q1","header":"Pick","question":"Which env?","options":[
            {"label":"prod","description":"production"},{"label":"dev","description":"development"}]}]
    });
    let ev = codex_question_to_control_request("q-1", &params);
    assert_eq!(ev["request"]["tool_name"], "AskUserQuestion");
    assert_eq!(ev["request"]["input"]["questions"][0]["question"], "Which env?");
    assert_eq!(ev["request"]["input"]["questions"][0]["options"][0]["label"], "prod");
    match parse_stream_line(&serde_json::to_string(&ev).unwrap()) {
        Ok(StreamMsg::ControlRequest(_)) => {}
        other => panic!("expected ControlRequest, got {other:?}"),
    }
}

#[test]
fn answers_map_back_to_codex_question_ids() {
    // Frontend answers by question TEXT; codex wants them by question ID.
    let params = serde_json::json!({"questions":[{"id":"q1","question":"Which env?"},{"id":"q2","question":"Confirm?"}]});
    let by_text = serde_json::json!({"Which env?":"prod","Confirm?":"yes"});
    let out = codex_answers_by_id(&params, &by_text);
    assert_eq!(out["q1"], "prod");
    assert_eq!(out["q2"], "yes");
}

#[test]
fn elicitation_becomes_elicitation_gate() {
    let params = serde_json::json!({"message":"Grant access?","mode":"form","requestedSchema":{"type":"object"}});
    let ev = codex_elicitation_to_control_request("e-1", &params);
    assert_eq!(ev["request"]["subtype"], "elicitation");
    assert_eq!(ev["request"]["message"], "Grant access?");
    match parse_stream_line(&serde_json::to_string(&ev).unwrap()) {
        Ok(StreamMsg::ControlRequest(_)) => {}
        other => panic!("expected ControlRequest, got {other:?}"),
    }
}

#[test]
fn permissions_response_grants_on_allow_denies_on_reject() {
    let params = serde_json::json!({"additionalPermissions":{"network":{}}});
    let allow = codex_permissions_response(true, &params);
    assert!(allow["permissions"].get("network").is_some());
    let deny = codex_permissions_response(false, &params);
    assert_eq!(deny["permissions"], serde_json::json!({}));
}
