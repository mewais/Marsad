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
    acp_config_model_values, acp_config_modes_update, acp_gate_routing_id,
    acp_permission_to_control_request_event, acp_prompt_result_to_result, acp_update_to_stream_json,
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

/// A subagent (`task` tool) lifecycle, replaying the reader's stateful
/// translation, must produce a Running-then-Done subagent grouped by the core —
/// exactly like a claude `Task` tool_use + `tool_result`. These are the REAL
/// wire lines captured from a live opencode session (title "task", enriched to a
/// description, completed via tool_call_update status). Before the fix these
/// arrived as plain "task" tool rows and never created a subagent.
#[test]
fn real_opencode_subagent_task_groups_running_then_done() {
    use marsad_agent::acp_translate::{
        acp_subagent_parse_update, acp_subagent_pending_id, acp_subagent_task_tool_use,
        acp_subagent_tool_result,
    };
    use marsad_events::SubagentStatus;
    use std::collections::HashSet;

    let mut state = SessionState::initial();
    let mut ids: HashSet<String> = HashSet::new();
    let mut created: HashSet<String> = HashSet::new();
    let mut done: HashSet<String> = HashSet::new();

    // Mirror the reader's subagent state machine, reducing every emitted event.
    let mut feed = |state: SessionState, raw: &str| -> SessionState {
        let msg = parse_acp_line(raw).expect("subagent line parses as ACP");
        let AcpMessage::Notification { params, .. } = msg else {
            panic!("expected notification");
        };
        let update = extract_update(&params).expect("has update");
        let mut st = state;
        let mut emit = |st: SessionState, sj: serde_json::Value| -> SessionState {
            let line = serde_json::to_string(&sj).unwrap();
            match parse_stream_line(&line) {
                Ok(m) => reduce(st, Event::Stream(m)),
                Err(e) => panic!("emitted stream-json failed to parse: {e}\n{line}"),
            }
        };
        if let Some(id) = acp_subagent_pending_id(update) {
            ids.insert(id);
            return st;
        }
        if let Some(su) = acp_subagent_parse_update(update) {
            if ids.contains(&su.tool_call_id) {
                if !created.contains(&su.tool_call_id) {
                    if let Some(name) = &su.name {
                        st = emit(st, acp_subagent_task_tool_use(&su.tool_call_id, name, &su.raw_input));
                        created.insert(su.tool_call_id.clone());
                    }
                }
                if su.status == "completed" && !done.contains(&su.tool_call_id) {
                    if !created.contains(&su.tool_call_id) {
                        let name = su.name.clone().unwrap_or_else(|| "subagent".to_string());
                        st = emit(st, acp_subagent_task_tool_use(&su.tool_call_id, &name, &su.raw_input));
                        created.insert(su.tool_call_id.clone());
                    }
                    let text = su.result_text.clone().unwrap_or_default();
                    st = emit(st, acp_subagent_tool_result(&su.tool_call_id, &text));
                    done.insert(su.tool_call_id.clone());
                }
            }
        }
        st
    };

    // 1. The bare `task` tool_call: marks the subagent, creates NOTHING yet.
    state = feed(state, r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_real","update":{"sessionUpdate":"tool_call","toolCallId":"toolu_task_1","title":"task","kind":"think","status":"pending","locations":[],"rawInput":{}}}}"#);
    assert!(state.subagents.is_empty(), "bare `task` call must not create a subagent yet");

    // 2. Enriched update carries the description → one Running subagent, named.
    state = feed(state, r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_real","update":{"sessionUpdate":"tool_call_update","toolCallId":"toolu_task_1","title":"Explore ArchAttention repo","kind":"think","status":"in_progress","rawInput":{"description":"Explore ArchAttention repo","prompt":"Explore the repository thoroughly."}}}}"#);
    assert_eq!(state.subagents.len(), 1, "enriched update creates exactly one subagent");
    assert_eq!(state.subagents[0].name, "Explore ArchAttention repo", "name comes from the task description");
    assert_eq!(state.subagents[0].status, SubagentStatus::Running, "subagent is Running while in_progress");

    // 3. A second in_progress update must NOT create a duplicate subagent.
    state = feed(state, r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_real","update":{"sessionUpdate":"tool_call_update","toolCallId":"toolu_task_1","title":"Explore ArchAttention repo","kind":"think","status":"in_progress","rawInput":{"description":"Explore ArchAttention repo","prompt":"..."}}}}"#);
    assert_eq!(state.subagents.len(), 1, "repeat updates must not duplicate the subagent");

    // 4. Completion → the subagent flips to Done (via the emitted tool_result).
    state = feed(state, r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_real","update":{"sessionUpdate":"tool_call_update","toolCallId":"toolu_task_1","title":"Explore ArchAttention repo","status":"completed","content":[{"type":"content","content":{"type":"text","text":"<task_result>Report on the repo.</task_result>"}}]}}}"#);
    assert_eq!(state.subagents.len(), 1, "still exactly one subagent after completion");
    assert_eq!(state.subagents[0].status, SubagentStatus::Done, "completed update marks the subagent Done");
}

/// SEAM TEST on REAL captured bytes: the exact `session/new` and
/// `session/set_config_option` result objects from a live opencode 1.18 session
/// (model options trimmed to 2 for readability; the `mode` group is verbatim).
/// Drives them through the SAME path the reader uses — acp_config_modes_update →
/// parse_stream_line → reduce — proving the available mode list surfaces and a
/// live mode switch (build → plan) reflects. This is the integration seam the
/// isolated unit tests don't cover.
#[test]
fn real_opencode_config_surfaces_modes_and_reflects_switch() {
    // Real `session/new` result.result (currentValue mode = build).
    let new_result: serde_json::Value = serde_json::from_str(
        r#"{"sessionId":"ses_07d65356dffet1wWuTdzwI41aa","configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"amd-anthropic/Claude-Sonnet-5","options":[{"value":"amd-anthropic/Claude-Haiku-4.5","name":"AMD Gateway (Anthropic format)/Claude Haiku 4.5"},{"value":"amd-anthropic/Claude-Opus-4.8","name":"AMD Gateway (Anthropic format)/Claude Opus 4.8"}]},{"id":"mode","name":"Session Mode","category":"mode","type":"select","currentValue":"build","options":[{"value":"build","name":"build","description":"The default agent. Executes tools based on configured permissions."},{"value":"plan","name":"plan","description":"Plan mode. Disallows all edit tools."}]}]}"#,
    )
    .unwrap();

    // Read path: the reader emits acp_config_modes_update(sid, new_result).
    let mu = acp_config_modes_update(SID, &new_result).expect("mode group present on real result");
    let line = serde_json::to_string(&mu).unwrap();
    let mut state = SessionState::initial();
    match parse_stream_line(&line) {
        Ok(m) => state = reduce(state, Event::Stream(m)),
        Err(e) => panic!("real modes_update failed to parse: {e}\n{line}"),
    }
    let ids: Vec<String> = state.available_modes.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, vec!["build", "plan"], "real opencode modes surface");
    assert_eq!(state.current_mode.as_deref(), Some("build"), "initial mode = build");

    // Launch model-apply gate: the pick must be a REAL option value.
    let (current, valid) = acp_config_model_values(&new_result);
    assert_eq!(current.as_deref(), Some("amd-anthropic/Claude-Sonnet-5"));
    assert!(valid.iter().any(|v| v == "amd-anthropic/Claude-Opus-4.8"));

    // Write path result: real `session/set_config_option` result (mode → plan),
    // fed through the SAME translation the reader applies to it.
    let setcfg: serde_json::Value = serde_json::from_str(
        r#"{"configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"amd-anthropic/Claude-Sonnet-5","options":[{"value":"amd-anthropic/Claude-Haiku-4.5","name":"AMD Gateway (Anthropic format)/Claude Haiku 4.5"},{"value":"amd-anthropic/Claude-Opus-4.8","name":"AMD Gateway (Anthropic format)/Claude Opus 4.8"}]},{"id":"mode","name":"Session Mode","category":"mode","type":"select","currentValue":"plan","options":[{"value":"build","name":"build","description":"The default agent. Executes tools based on configured permissions."},{"value":"plan","name":"plan","description":"Plan mode. Disallows all edit tools."}]}]}"#,
    )
    .unwrap();
    let mu2 = acp_config_modes_update(SID, &setcfg).expect("mode group on set result");
    let line2 = serde_json::to_string(&mu2).unwrap();
    match parse_stream_line(&line2) {
        Ok(m) => state = reduce(state, Event::Stream(m)),
        Err(e) => panic!("set_config modes_update failed to parse: {e}\n{line2}"),
    }
    assert_eq!(
        state.current_mode.as_deref(),
        Some("plan"),
        "a live mode switch (set_config_option → plan) reflects in state"
    );
    assert_eq!(
        state.available_modes.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        vec!["build", "plan"],
        "the list is unchanged by the switch"
    );
}
