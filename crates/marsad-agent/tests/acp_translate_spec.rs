//! TDD red-phase tests for `marsad_agent::acp_translate`.
//!
//! All functions under test are `todo!()` stubs, so every test here MUST:
//!   1. Compile without errors.
//!   2. Panic at runtime (todo! panic), making the test FAIL — not ERROR.
//!
//! Spec reference: docs/specs/opencode-acp.md §2/§3/§4/§5/§8.
//!
//! Pattern mirrors `crates/marsad-agent/src/codex.rs` tests:
//!   synthesise a Value, serialise to string, parse through
//!   `marsad_proto::parse_stream_line`, assert the correct `StreamMsg` variant.

use marsad_agent::acp_translate::{
    acp_init_to_system_init, acp_options_to_suggestions, acp_permission_to_gate,
    acp_stop_reason_to_result, acp_update_to_stream_json, detect_compaction,
};
use marsad_gates::Gate;
use marsad_proto::{parse_stream_line, StreamMsg};
use serde_json::{json, Value};

// ── helpers ───────────────────────────────────────────────────────────────────

const SESSION_ID: &str = "acp-sess-0001";
const MODEL: &str = "claude-sonnet-4";
const CWD: &str = "/home/user/myproject";
const PROJECT: &str = "myproject";

/// Serialise `v` and parse it through `parse_stream_line`, panicking on error.
fn parse(v: &Value) -> StreamMsg {
    let s = serde_json::to_string(v).expect("Value is always serialisable");
    parse_stream_line(&s).expect("synthesised event must parse as a StreamMsg")
}

// =============================================================================
// acp_update_to_stream_json — per-variant coverage
// =============================================================================

// ── agent_message_chunk (text ContentBlock) ───────────────────────────────────

/// A text `agent_message_chunk` must produce an `assistant` message whose first
/// content block is `{ "type": "text", "text": <value> }`.
#[test]
fn agent_message_chunk_text_produces_assistant_text() {
    let update = json!({
        "type": "agent_message_chunk",
        "messageId": "msg-acp-001",
        "content": {
            "type": "text",
            "text": "Hello from opencode!",
        }
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    assert_eq!(out["type"], "assistant", "outer type must be 'assistant'");
    let content = &out["message"]["content"][0];
    assert_eq!(content["type"], "text");
    assert_eq!(content["text"], "Hello from opencode!");
    // parse_stream_line round-trip
    assert!(
        matches!(parse(&out), StreamMsg::Assistant(_)),
        "must parse as StreamMsg::Assistant"
    );
}

/// Two chunks with the same `messageId` must produce the same `message.id` so
/// the reducer can deduplicate / accumulate them as one assistant message.
#[test]
fn agent_message_chunk_same_message_id_maps_to_same_assistant_id() {
    let chunk_a = json!({
        "type": "agent_message_chunk",
        "messageId": "msg-shared-42",
        "content": { "type": "text", "text": "part one " }
    });
    let chunk_b = json!({
        "type": "agent_message_chunk",
        "messageId": "msg-shared-42",
        "content": { "type": "text", "text": "part two" }
    });
    let out_a = acp_update_to_stream_json(&chunk_a, SESSION_ID, MODEL).unwrap();
    let out_b = acp_update_to_stream_json(&chunk_b, SESSION_ID, MODEL).unwrap();
    let id_a = out_a["message"]["id"].as_str().unwrap_or("");
    let id_b = out_b["message"]["id"].as_str().unwrap_or("");
    assert!(!id_a.is_empty(), "message id must not be empty");
    assert_eq!(id_a, id_b, "same messageId → same assistant message.id");
    // Both must parse as Assistant
    assert!(matches!(parse(&out_a), StreamMsg::Assistant(_)));
    assert!(matches!(parse(&out_b), StreamMsg::Assistant(_)));
}

/// Different `messageId` values must produce different `message.id` values.
#[test]
fn agent_message_chunk_different_message_ids_map_to_different_assistant_ids() {
    let chunk_a = json!({
        "type": "agent_message_chunk",
        "messageId": "msg-aaa",
        "content": { "type": "text", "text": "turn one" }
    });
    let chunk_b = json!({
        "type": "agent_message_chunk",
        "messageId": "msg-bbb",
        "content": { "type": "text", "text": "turn two" }
    });
    let out_a = acp_update_to_stream_json(&chunk_a, SESSION_ID, MODEL).unwrap();
    let out_b = acp_update_to_stream_json(&chunk_b, SESSION_ID, MODEL).unwrap();
    let id_a = out_a["message"]["id"].as_str().unwrap_or("x");
    let id_b = out_b["message"]["id"].as_str().unwrap_or("y");
    assert_ne!(id_a, id_b, "different messageIds → different assistant message.ids");
}

// ── agent_thought_chunk ───────────────────────────────────────────────────────

/// An `agent_thought_chunk` must NOT produce a normal assistant text message.
/// Per spec §3 it must be a thinking/thought event (a thinking block or
/// dedicated thought event). It MUST NOT be `None` (it is rendered somewhere)
/// but it MUST NOT carry a plain `text` content block.
#[test]
fn agent_thought_chunk_is_not_a_normal_assistant_text_message() {
    let update = json!({
        "type": "agent_thought_chunk",
        "messageId": "thought-001",
        "thinking": "Let me reason about this carefully...",
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL);
    // Must produce SOME output (not None — thought events are rendered)
    let out = out.expect("agent_thought_chunk must produce Some(Value)");
    // Must parse without error
    let parsed = parse(&out);
    // The thought event may be Assistant(thinking block) or Unknown;
    // what it must NOT be is a plain text content block.
    if let StreamMsg::Assistant(msg) = &parsed {
        for block in &msg.message.content {
            match block {
                marsad_proto::ContentBlock::Text { .. } => {
                    panic!("agent_thought_chunk must not produce a plain text content block")
                }
                _ => {}
            }
        }
    }
    // If it's Unknown, that's also fine (forward-compat thought shape).
}

/// A thought chunk must not count as a normal assistant message:
/// its content type in the assistant block (if any) is `thinking`, not `text`.
#[test]
fn agent_thought_chunk_produces_thinking_content_block() {
    let update = json!({
        "type": "agent_thought_chunk",
        "messageId": "thought-002",
        "thinking": "Deep thought here.",
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    // If it's an assistant message, the content block type must be "thinking".
    if out["type"] == "assistant" {
        let block_type = out["message"]["content"][0]["type"].as_str().unwrap_or("");
        assert_eq!(
            block_type, "thinking",
            "thought chunk assistant block must have type=thinking"
        );
    }
    // Parses without error either way.
    parse(&out);
}

// ── tool_call ─────────────────────────────────────────────────────────────────

/// A `tool_call` update must produce an `assistant` message with a `tool_use`
/// content block carrying the correct id, name, and parsed input.
#[test]
fn tool_call_produces_assistant_with_tool_use_block() {
    let update = json!({
        "type": "tool_call",
        "toolName": "Bash",
        "toolUseId": "toolu-acp-bash-001",
        "input": "{\"command\": \"echo hello\"}"
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    assert_eq!(out["type"], "assistant");
    let content = &out["message"]["content"][0];
    assert_eq!(content["type"], "tool_use");
    assert_eq!(content["id"], "toolu-acp-bash-001");
    assert_eq!(content["name"], "Bash");
    // Input must be parsed from the JSON string
    assert_eq!(content["input"]["command"], "echo hello");
    // parse_stream_line round-trip
    assert!(matches!(parse(&out), StreamMsg::Assistant(_)));
}

/// `tool_call` for a non-Bash tool (e.g. a custom MCP tool) must also work.
#[test]
fn tool_call_custom_tool_name_is_preserved() {
    let update = json!({
        "type": "tool_call",
        "toolName": "mcp__files__read_file",
        "toolUseId": "toolu-mcp-read-007",
        "input": "{\"path\": \"/etc/hosts\"}"
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    let content = &out["message"]["content"][0];
    assert_eq!(content["name"], "mcp__files__read_file");
    assert_eq!(content["id"], "toolu-mcp-read-007");
    assert_eq!(content["input"]["path"], "/etc/hosts");
    assert!(matches!(parse(&out), StreamMsg::Assistant(_)));
}

// ── tool_call_update with diff ────────────────────────────────────────────────

/// A `tool_call_update` with a `diff` content item maps to the file-change /
/// tool-result shape.  The output must parse as an `assistant` message
/// (carrying the tool_use or tool_result block).
#[test]
fn tool_call_update_diff_produces_file_change_shape() {
    let update = json!({
        "type": "tool_call_update",
        "toolUseId": "toolu-edit-acp-002",
        "toolName": "Edit",
        "input": "{\"path\": \"/home/user/myproject/main.rs\"}",
        "content": [
            {
                "type": "diff",
                "file": "/home/user/myproject/main.rs",
                "patch": "@@ -1,3 +1,4 @@\n fn main() {\n+    println!(\"hello\");\n }\n"
            }
        ]
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    // Must be an assistant message (tool_use or tool_result block)
    assert_eq!(out["type"], "assistant", "diff tool_call_update must be assistant");
    assert!(matches!(parse(&out), StreamMsg::Assistant(_)));
    // The diff info must be present somewhere in the content
    let content_arr = out["message"]["content"].as_array().unwrap();
    assert!(!content_arr.is_empty(), "content must not be empty");
}

// ── tool_call_update with text ────────────────────────────────────────────────

/// A `tool_call_update` with a `text` content item maps to tool output.
/// Must parse as an `assistant` message (tool_use progress / tool_result).
#[test]
fn tool_call_update_text_produces_tool_output_shape() {
    let update = json!({
        "type": "tool_call_update",
        "toolUseId": "toolu-bash-acp-003",
        "toolName": "Bash",
        "input": "{\"command\": \"ls -la\"}",
        "content": [
            {
                "type": "text",
                "text": "total 16\ndrwxr-xr-x  2 user user 4096 Jan  1 00:00 .\n"
            }
        ]
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    assert_eq!(out["type"], "assistant", "text tool_call_update must be assistant");
    assert!(matches!(parse(&out), StreamMsg::Assistant(_)));
    let content_arr = out["message"]["content"].as_array().unwrap();
    assert!(!content_arr.is_empty(), "content must not be empty");
}

// ── plan update ───────────────────────────────────────────────────────────────

/// A `plan` update with `entries` must produce a payload carrying those entries
/// with their `content` and `priority` values preserved.
#[test]
fn plan_update_carries_entries_with_content_and_priority() {
    let update = json!({
        "type": "plan",
        "plan": {
            "entries": [
                { "content": "Research the problem", "priority": "high" },
                { "content": "Write unit tests",    "priority": "critical" },
                { "content": "Implement the fix",   "priority": "medium" },
                { "content": "Update docs",          "priority": "low" }
            ]
        }
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    // Must produce *some* output
    assert!(!out.is_null(), "plan update must produce a non-null Value");
    // Entries and priorities must be preserved — check at least the first two.
    // The exact nesting path is spec-defined; we search the serialized JSON for
    // the expected strings as a forward-compat approach.
    let serialised = serde_json::to_string(&out).unwrap();
    assert!(
        serialised.contains("Research the problem"),
        "entry content 'Research the problem' must be present: {serialised}"
    );
    assert!(
        serialised.contains("critical"),
        "priority 'critical' must be present: {serialised}"
    );
    assert!(
        serialised.contains("Write unit tests"),
        "entry content 'Write unit tests' must be present: {serialised}"
    );
    // Must parse without error (may be Unknown, that's forward-compat)
    parse(&out);
}

/// Priority values from the ACP spec must survive the translation verbatim.
#[test]
fn plan_update_all_four_priorities_preserved() {
    let update = json!({
        "type": "plan",
        "plan": {
            "entries": [
                { "content": "a", "priority": "critical" },
                { "content": "b", "priority": "high" },
                { "content": "c", "priority": "medium" },
                { "content": "d", "priority": "low" }
            ]
        }
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    let s = serde_json::to_string(&out).unwrap();
    for prio in &["critical", "high", "medium", "low"] {
        assert!(s.contains(prio), "priority '{prio}' missing from plan output: {s}");
    }
}

// ── available_commands_update ─────────────────────────────────────────────────

/// An `available_commands_update` must produce a slash-commands payload.
/// The command names must be present in the output.
#[test]
fn available_commands_update_produces_slash_command_shape() {
    let update = json!({
        "type": "available_commands_update",
        "availableCommands": [
            { "name": "/compact", "description": "Compact context window" },
            { "name": "/clear",   "description": "Clear conversation" },
            { "name": "/plan",    "description": "Enter plan mode", "input": {} }
        ]
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    assert!(!out.is_null(), "available_commands_update must produce a non-null Value");
    let s = serde_json::to_string(&out).unwrap();
    assert!(s.contains("/compact"), "slash command '/compact' must be present: {s}");
    assert!(s.contains("/clear"),   "slash command '/clear' must be present: {s}");
    assert!(s.contains("/plan"),    "slash command '/plan' must be present: {s}");
    parse(&out);
}

// ── current_mode_update ───────────────────────────────────────────────────────

/// A `current_mode_update` must produce output that carries the `currentModeId`.
#[test]
fn current_mode_update_carries_mode_id() {
    let update = json!({
        "type": "current_mode_update",
        "currentModeId": "plan"
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    assert!(!out.is_null());
    let s = serde_json::to_string(&out).unwrap();
    assert!(s.contains("plan"), "currentModeId 'plan' must be present: {s}");
    parse(&out);
}

#[test]
fn current_mode_update_build_mode_carried() {
    let update = json!({
        "type": "current_mode_update",
        "currentModeId": "build"
    });
    let out = acp_update_to_stream_json(&update, SESSION_ID, MODEL).unwrap();
    let s = serde_json::to_string(&out).unwrap();
    assert!(s.contains("build"), "currentModeId 'build' must be present: {s}");
}

// ── user_message_chunk — skipped ─────────────────────────────────────────────

/// `user_message_chunk` is echoed user content that the spec says to skip.
#[test]
fn user_message_chunk_is_skipped() {
    let update = json!({
        "type": "user_message_chunk",
        "content": { "type": "text", "text": "Hello" }
    });
    let result = acp_update_to_stream_json(&update, SESSION_ID, MODEL);
    assert!(result.is_none(), "user_message_chunk must be skipped (return None)");
}

// ── unknown update type — skipped ─────────────────────────────────────────────

/// An unrecognised update `type` must return `None` (forward-compatible skip).
#[test]
fn unknown_update_type_returns_none() {
    let update = json!({
        "type": "some_future_update_type_not_in_spec",
        "data": { "foo": 42 }
    });
    let result = acp_update_to_stream_json(&update, SESSION_ID, MODEL);
    assert!(result.is_none(), "unknown update type must return None");
}

/// Another unknown variant — belt-and-braces for forward compatibility.
#[test]
fn another_unknown_update_type_returns_none() {
    let update = json!({
        "type": "config_option_update",
        "configOptions": {}
    });
    // config_option_update is tracked internally but not translated to stream-json
    // (per spec §2, it feeds session config options; no stream-json shape exists).
    // Either None (skip) is acceptable; this test only asserts it doesn't panic.
    // If the implementation decides to emit something, it must parse.
    let result = acp_update_to_stream_json(&update, SESSION_ID, MODEL);
    if let Some(ref v) = result {
        parse(v); // must be valid stream-json if emitted
    }
}

// ── Missing / empty type ──────────────────────────────────────────────────────

#[test]
fn update_with_missing_type_returns_none() {
    let update = json!({ "content": { "type": "text", "text": "oops" } });
    let result = acp_update_to_stream_json(&update, SESSION_ID, MODEL);
    assert!(result.is_none(), "update with missing type must return None");
}

// =============================================================================
// acp_stop_reason_to_result — all four stop reasons
// =============================================================================

/// `"completed"` → a result/success event (is_error=false).
#[test]
fn stop_reason_completed_produces_result_success() {
    let out = acp_stop_reason_to_result("completed");
    assert_eq!(out["type"], "result", "must be type=result");
    assert_eq!(out["subtype"], "success", "completed → subtype=success");
    assert_eq!(out["is_error"], false, "completed → is_error=false");
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// `"cancelled"` → a result with cancel/interrupt semantics (not a hard error).
#[test]
fn stop_reason_cancelled_produces_result_with_cancel_semantics() {
    let out = acp_stop_reason_to_result("cancelled");
    assert_eq!(out["type"], "result", "must be type=result");
    // Cancelled may map to success (interrupt-done) or error; we just assert
    // it's a valid result event and is_error=false (interrupt, not crash).
    let is_error = out["is_error"].as_bool().unwrap_or(true);
    assert!(!is_error, "cancelled → is_error=false (interrupt semantics)");
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// `"auth_required"` → a result/error with a real human-readable reason.
#[test]
fn stop_reason_auth_required_produces_result_error() {
    let out = acp_stop_reason_to_result("auth_required");
    assert_eq!(out["type"], "result");
    assert_eq!(out["subtype"], "error_during_execution");
    assert_eq!(out["is_error"], true, "auth_required → is_error=true");
    // Must carry a non-empty reason
    let reason = out["result"].as_str().unwrap_or("");
    assert!(!reason.is_empty(), "auth_required must carry a non-empty reason");
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// `"request_limit_exceeded"` → a result/error with a real reason.
#[test]
fn stop_reason_request_limit_exceeded_produces_result_error() {
    let out = acp_stop_reason_to_result("request_limit_exceeded");
    assert_eq!(out["type"], "result");
    assert_eq!(out["subtype"], "error_during_execution");
    assert_eq!(out["is_error"], true);
    let reason = out["result"].as_str().unwrap_or("");
    assert!(!reason.is_empty(), "request_limit_exceeded must carry a non-empty reason");
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// An unrecognised stop reason falls back to result/error.
#[test]
fn stop_reason_unknown_falls_back_to_result_error() {
    let out = acp_stop_reason_to_result("something_unexpected");
    assert_eq!(out["type"], "result");
    assert_eq!(out["is_error"], true);
    assert!(matches!(parse(&out), StreamMsg::Result(_)));
}

/// Every result event must carry a `usage` block with at minimum the four
/// token fields (may all be 0).
#[test]
fn stop_reason_result_events_carry_usage_block() {
    for reason in &["completed", "cancelled", "auth_required", "request_limit_exceeded"] {
        let out = acp_stop_reason_to_result(reason);
        assert!(
            out.get("usage").is_some(),
            "result for stop_reason={reason} must carry a usage block"
        );
        // parse must succeed
        assert!(
            matches!(parse(&out), StreamMsg::Result(_)),
            "result for stop_reason={reason} must parse as StreamMsg::Result"
        );
    }
}

// =============================================================================
// acp_init_to_system_init
// =============================================================================

/// The synthesised `system/init` event must parse as `StreamMsg::SystemInit`
/// and carry the correct model, cwd, and permissionMode.
#[test]
fn acp_init_produces_system_init_with_correct_fields() {
    let init_result = json!({
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": [],
            "sessionCapabilities": []
        }
    });
    let session_result = json!({
        "sessionId": SESSION_ID,
        "modes": ["build", "plan"],
        "configOptions": {}
    });
    let out = acp_init_to_system_init(
        &init_result,
        &session_result,
        MODEL,
        CWD,
        PROJECT,
    );
    assert_eq!(out["type"], "system");
    assert_eq!(out["subtype"], "init");
    assert_eq!(out["model"], MODEL, "model must be embedded");
    assert_eq!(out["cwd"], CWD, "cwd must be embedded");
    // permissionMode must be present and non-empty
    let perm_mode = out["permissionMode"].as_str().unwrap_or("");
    assert!(!perm_mode.is_empty(), "permissionMode must be present and non-empty");
    // Must parse as SystemInit
    assert!(
        matches!(parse(&out), StreamMsg::SystemInit(_)),
        "system/init must parse as StreamMsg::SystemInit"
    );
}

/// The `session_id` field in system/init must be propagated from the session result.
#[test]
fn acp_init_session_id_comes_from_session_result() {
    let init_result = json!({ "agentCapabilities": {} });
    let session_result = json!({ "sessionId": "sess-xyz-789" });
    let out = acp_init_to_system_init(
        &init_result,
        &session_result,
        "claude-haiku-4",
        "/tmp/proj",
        "proj",
    );
    let session_id = out["session_id"].as_str().unwrap_or("");
    assert_eq!(session_id, "sess-xyz-789", "session_id must come from session_result.sessionId");
    assert!(matches!(parse(&out), StreamMsg::SystemInit(_)));
}

/// `slash_commands` in the init event must be an empty array (seeded empty per spec §3).
#[test]
fn acp_init_slash_commands_seeded_empty() {
    let init_result = json!({ "agentCapabilities": {} });
    let session_result = json!({ "sessionId": "s1" });
    let out = acp_init_to_system_init(
        &init_result,
        &session_result,
        MODEL,
        CWD,
        PROJECT,
    );
    let cmds = out["slash_commands"].as_array();
    assert!(
        cmds.map_or(true, |a| a.is_empty()),
        "slash_commands must be seeded empty: {:?}",
        out["slash_commands"]
    );
    assert!(matches!(parse(&out), StreamMsg::SystemInit(_)));
}

/// `mcp_servers` in the init event must be present (may be empty).
#[test]
fn acp_init_mcp_servers_field_present() {
    let init_result = json!({ "agentCapabilities": {} });
    let session_result = json!({ "sessionId": "s2" });
    let out = acp_init_to_system_init(
        &init_result,
        &session_result,
        MODEL,
        CWD,
        PROJECT,
    );
    assert!(
        out.get("mcp_servers").is_some(),
        "mcp_servers field must be present in system/init"
    );
    assert!(matches!(parse(&out), StreamMsg::SystemInit(_)));
}

// =============================================================================
// acp_permission_to_gate
// =============================================================================

fn make_permission_params() -> Value {
    json!({
        "sessionId": SESSION_ID,
        "toolCall": {
            "toolName": "Bash",
            "toolUseId": "toolu-perm-001",
            "input": "{\"command\": \"rm -rf /tmp/junk\"}"
        },
        "options": [
            { "optionId": "opt-allow-once",   "name": "Allow once",    "kind": "allow_once"   },
            { "optionId": "opt-allow-always",  "name": "Allow always",  "kind": "allow_always" },
            { "optionId": "opt-reject-once",   "name": "Reject once",   "kind": "reject_once"  },
            { "optionId": "opt-reject-always", "name": "Reject always", "kind": "reject_always"}
        ]
    })
}

/// The returned gate must be a `Gate::Permission` (not Plan, Edit, or Question).
#[test]
fn acp_permission_to_gate_returns_permission_variant() {
    let params = make_permission_params();
    let gate = acp_permission_to_gate(&params);
    assert!(
        matches!(gate, Gate::Permission { .. }),
        "acp_permission_to_gate must return Gate::Permission, got: {gate:?}"
    );
}

/// The ACP `options[]` must be carried VERBATIM in `permission_suggestions`,
/// preserving `optionId`, `name`, and `kind` for each entry.
#[test]
fn acp_permission_to_gate_options_verbatim_in_permission_suggestions() {
    let params = make_permission_params();
    let gate = acp_permission_to_gate(&params);
    let Gate::Permission { permission_suggestions, .. } = gate else {
        panic!("expected Gate::Permission");
    };
    assert_eq!(
        permission_suggestions.len(),
        4,
        "all 4 ACP options must be present in permission_suggestions"
    );
    // Verify each option by checking optionId, name, kind
    let expected = [
        ("opt-allow-once",   "Allow once",    "allow_once"),
        ("opt-allow-always", "Allow always",  "allow_always"),
        ("opt-reject-once",  "Reject once",   "reject_once"),
        ("opt-reject-always","Reject always", "reject_always"),
    ];
    for (i, (exp_id, exp_name, exp_kind)) in expected.iter().enumerate() {
        let opt = &permission_suggestions[i];
        assert_eq!(
            opt["optionId"].as_str().unwrap_or(""),
            *exp_id,
            "option[{i}].optionId mismatch"
        );
        assert_eq!(
            opt["name"].as_str().unwrap_or(""),
            *exp_name,
            "option[{i}].name mismatch"
        );
        assert_eq!(
            opt["kind"].as_str().unwrap_or(""),
            *exp_kind,
            "option[{i}].kind mismatch"
        );
    }
}

/// The gate `command` field must reflect the toolCall (toolName + input).
#[test]
fn acp_permission_to_gate_command_reflects_tool_call() {
    let params = make_permission_params();
    let gate = acp_permission_to_gate(&params);
    let Gate::Permission { command, .. } = &gate else {
        panic!("expected Gate::Permission");
    };
    // The command must contain the tool name "Bash" at minimum.
    assert!(
        command.contains("Bash") || command.contains("rm -rf"),
        "gate command must reflect the toolCall (Bash / rm -rf), got: {command:?}"
    );
}

/// The gate `explanation` must be non-empty.
#[test]
fn acp_permission_to_gate_explanation_non_empty() {
    let params = make_permission_params();
    let gate = acp_permission_to_gate(&params);
    let Gate::Permission { explanation, .. } = &gate else {
        panic!("expected Gate::Permission");
    };
    assert!(
        !explanation.is_empty(),
        "gate explanation must not be empty"
    );
}

/// The gate `request_id` must be non-empty (it routes the human's answer back
/// to the pending ACP JSON-RPC request).
#[test]
fn acp_permission_to_gate_request_id_non_empty() {
    let params = make_permission_params();
    let gate = acp_permission_to_gate(&params);
    let Gate::Permission { request_id, .. } = &gate else {
        panic!("expected Gate::Permission");
    };
    assert!(!request_id.is_empty(), "gate request_id must not be empty");
}

/// Options must not be renamed, invented, or re-ordered. This test builds a
/// params with a custom option list and verifies verbatim preservation.
#[test]
fn acp_permission_to_gate_no_invented_options() {
    let params = json!({
        "sessionId": SESSION_ID,
        "toolCall": {
            "toolName": "Edit",
            "toolUseId": "toolu-edit-perm-999",
            "input": "{\"path\": \"/etc/passwd\"}"
        },
        "options": [
            { "optionId": "custom-opt-A", "name": "Proceed carefully", "kind": "allow_once" },
            { "optionId": "custom-opt-B", "name": "Never again",        "kind": "reject_always" }
        ]
    });
    let gate = acp_permission_to_gate(&params);
    let Gate::Permission { permission_suggestions, .. } = gate else {
        panic!("expected Gate::Permission");
    };
    // Exactly the two provided options, verbatim
    assert_eq!(permission_suggestions.len(), 2, "must carry exactly 2 options");
    assert_eq!(
        permission_suggestions[0]["optionId"],
        "custom-opt-A",
        "first option must be 'custom-opt-A'"
    );
    assert_eq!(
        permission_suggestions[1]["optionId"],
        "custom-opt-B",
        "second option must be 'custom-opt-B'"
    );
    assert_eq!(permission_suggestions[0]["name"], "Proceed carefully");
    assert_eq!(permission_suggestions[1]["name"], "Never again");
}

// =============================================================================
// acp_options_to_suggestions
// =============================================================================

/// The function must return a `Vec<Value>` with each option object verbatim.
#[test]
fn acp_options_to_suggestions_verbatim_passthrough() {
    let options = json!([
        { "optionId": "o1", "name": "Option One",   "kind": "allow_once"    },
        { "optionId": "o2", "name": "Option Two",   "kind": "allow_always"  },
        { "optionId": "o3", "name": "Option Three", "kind": "reject_once"   },
        { "optionId": "o4", "name": "Option Four",  "kind": "reject_always" }
    ]);
    let suggestions = acp_options_to_suggestions(&options);
    assert_eq!(suggestions.len(), 4, "must return all 4 options");
    // Verify verbatim preservation for each
    for (i, (exp_id, exp_name, exp_kind)) in [
        ("o1", "Option One",   "allow_once"),
        ("o2", "Option Two",   "allow_always"),
        ("o3", "Option Three", "reject_once"),
        ("o4", "Option Four",  "reject_always"),
    ].iter().enumerate() {
        assert_eq!(suggestions[i]["optionId"], *exp_id,   "option[{i}].optionId");
        assert_eq!(suggestions[i]["name"],     *exp_name, "option[{i}].name");
        assert_eq!(suggestions[i]["kind"],     *exp_kind, "option[{i}].kind");
    }
}

/// An empty options array must return an empty vec.
#[test]
fn acp_options_to_suggestions_empty_array() {
    let options = json!([]);
    let suggestions = acp_options_to_suggestions(&options);
    assert!(suggestions.is_empty(), "empty options array → empty vec");
}

/// A single option must round-trip correctly.
#[test]
fn acp_options_to_suggestions_single_option() {
    let options = json!([
        { "optionId": "sole", "name": "The only choice", "kind": "allow_once" }
    ]);
    let suggestions = acp_options_to_suggestions(&options);
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0]["optionId"], "sole");
    assert_eq!(suggestions[0]["name"], "The only choice");
    assert_eq!(suggestions[0]["kind"], "allow_once");
}

/// The raw option object is passed through without reshaping — no field is
/// renamed or dropped.
#[test]
fn acp_options_to_suggestions_no_field_renaming() {
    // Include an extra field that is NOT in the spec — it must still survive.
    let options = json!([
        {
            "optionId": "x",
            "name": "Xtra",
            "kind": "allow_once",
            "description": "an extra field that must survive"
        }
    ]);
    let suggestions = acp_options_to_suggestions(&options);
    assert_eq!(suggestions.len(), 1);
    assert_eq!(
        suggestions[0]["description"],
        "an extra field that must survive",
        "extra fields on option objects must not be dropped"
    );
}

// =============================================================================
// detect_compaction
// =============================================================================

/// A compaction notification must return `true`.
/// Per spec §3 and §5, opencode emits compaction as a `session/update` update
/// with either a `_meta` marker or a dedicated `type` field.
/// We test the `_meta` shape:
#[test]
fn detect_compaction_meta_shape_returns_true() {
    let update = json!({
        "type": "context_compaction",
        "_meta": {
            "compacted": true,
            "pre_tokens": 80000,
            "post_tokens": 12000
        }
    });
    assert!(
        detect_compaction(&update),
        "compaction update with _meta must return true"
    );
}

/// A compaction notification with a dedicated type field must also return `true`.
#[test]
fn detect_compaction_dedicated_type_returns_true() {
    let update = json!({
        "type": "context_compaction",
        "reason": "auto"
    });
    assert!(
        detect_compaction(&update),
        "update with type=context_compaction must return true"
    );
}

/// A normal `agent_message_chunk` update must NOT trigger compaction detection.
#[test]
fn detect_compaction_normal_message_chunk_returns_false() {
    let update = json!({
        "type": "agent_message_chunk",
        "messageId": "msg-999",
        "content": { "type": "text", "text": "This is not a compaction event" }
    });
    assert!(
        !detect_compaction(&update),
        "agent_message_chunk must not be detected as compaction"
    );
}

/// A `tool_call` update must NOT trigger compaction detection.
#[test]
fn detect_compaction_tool_call_returns_false() {
    let update = json!({
        "type": "tool_call",
        "toolName": "Bash",
        "toolUseId": "toolu-000",
        "input": "{}"
    });
    assert!(!detect_compaction(&update), "tool_call must not be detected as compaction");
}

/// A `plan` update must NOT trigger compaction detection.
#[test]
fn detect_compaction_plan_update_returns_false() {
    let update = json!({
        "type": "plan",
        "plan": { "entries": [] }
    });
    assert!(!detect_compaction(&update), "plan update must not be detected as compaction");
}

/// An update with `_meta.compacted = false` must NOT trigger compaction detection.
#[test]
fn detect_compaction_meta_compacted_false_returns_false() {
    let update = json!({
        "type": "some_update",
        "_meta": {
            "compacted": false
        }
    });
    assert!(
        !detect_compaction(&update),
        "_meta.compacted=false must not be treated as compaction"
    );
}

/// An empty update object must NOT trigger compaction detection.
#[test]
fn detect_compaction_empty_update_returns_false() {
    let update = json!({});
    assert!(!detect_compaction(&update), "empty update must not be detected as compaction");
}
