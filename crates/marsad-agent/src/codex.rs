//! Codex JSONL → claude-stream-json event translator.
//!
//! `codex_event_to_stream_json` translates one JSONL line emitted by
//! `codex exec --json` into the claude-stream-json event shape that Core and
//! GUI expect, so they require zero codex-specific changes.
//!
//! ## Event vocabulary (docs/specs/codex-research.md §a)
//!
//! Codex emits:
//!   `thread.started {thread_id}`
//!   `turn.started`
//!   `item.completed {item: {type, …}}`
//!     - `agent_message {text}`
//!     - `command_execution {command?, aggregated_output?, exit_code?}`
//!     - `file_change {path?, diff?, operation?}`
//!     - `mcp_tool_call {tool_name?, …}`
//!     - `reasoning {text?}`
//!     - `error {message?}`
//!   `turn.completed {usage: {input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens}}`
//!   `turn.failed {error?}`
//!
//! ## Translation mapping
//!
//! | Codex event           | Claude-stream-json output           |
//! |----------------------|-------------------------------------|
//! | `thread.started`     | `system/init`                       |
//! | `item.completed agent_message` | `assistant` (text)        |
//! | `item.completed command_execution` | `assistant` (tool_use Bash) |
//! | `item.completed file_change` | `assistant` (tool_use Edit) |
//! | `item.completed mcp_tool_call` | `assistant` (tool_use by name) |
//! | `item.completed reasoning` | `None` (skipped)              |
//! | `item.completed error` | `system/codex_error` (non-fatal)  |
//! | `turn.started`       | `None` (skipped)                    |
//! | `turn.completed`     | `result/success`                    |
//! | `turn.failed`        | `result/error`                      |
//! | unknown / other      | `None` (skipped)                    |
//!
//! ## Core compatibility
//!
//! Every synthesized event passes `marsad_proto::parse_stream_line` — this is
//! asserted by the unit tests in this module. All `tool_use` blocks carry a
//! minimal `usage` in the assistant message (zeros) to satisfy Core's
//! `AssistantMsg` schema; `result` carries a zeroed total cost and num_turns=1.

use serde_json::{json, Value};

/// Translate one codex JSONL object into a claude-stream-json `Value`, or
/// `None` to skip the event (caller does not broadcast it).
///
/// # Parameters
/// - `codex_line`: the already-parsed codex JSONL object.
/// - `model`: the model name to embed in the synthesized `system/init` event.
/// - `project`: the working directory (cwd) to embed in `system/init`.
/// - `posture_mode`: the permission mode string to embed in `system/init`
///   (`"bypassPermissions"` / `"acceptEdits"` / `"default"`).
pub fn codex_event_to_stream_json(
    codex_line: &Value,
    model: &str,
    project: &str,
    posture_mode: &str,
) -> Option<Value> {
    let type_str = codex_line
        .get("type")
        .and_then(|t| t.as_str())?;

    match type_str {
        // ── thread.started → system/init ─────────────────────────────────────
        "thread.started" => {
            let thread_id = codex_line
                .get("thread_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(json!({
                "type": "system",
                "subtype": "init",
                "session_id": thread_id,
                "model": model,
                "cwd": project,
                "tools": [],
                "mcp_servers": [],
                "slash_commands": [],
                "permissionMode": posture_mode,
            }))
        }

        // ── turn.started → skip ───────────────────────────────────────────────
        "turn.started" => None,

        // ── item.completed → dispatch by item.type ────────────────────────────
        "item.completed" => {
            let item = codex_line.get("item")?;
            let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let item_id = item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("item_0")
                .to_string();

            match item_type {
                // agent_message → assistant text block
                "agent_message" => {
                    let text = item
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(make_assistant_text(&text))
                }

                // reasoning → skip (not shown as a normal message)
                "reasoning" => None,

                // error → system/codex_error (non-fatal note)
                "error" => {
                    let msg = item
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("codex error")
                        .to_string();
                    Some(json!({
                        "type": "system",
                        "subtype": "codex_error",
                        "text": msg,
                    }))
                }

                // command_execution → assistant tool_use "Bash"
                "command_execution" => {
                    let cmd = item
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let output = item
                        .get("aggregated_output")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let exit_code = item
                        .get("exit_code")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    Some(make_assistant_tool_use(
                        &item_id,
                        "Bash",
                        json!({
                            "command": cmd,
                            "output": output,
                            "exit_code": exit_code,
                        }),
                    ))
                }

                // file_change → assistant tool_use "Edit"
                "file_change" => {
                    let path = item
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let diff = item.get("diff").cloned().unwrap_or(Value::Null);
                    let op = item
                        .get("operation")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(make_assistant_tool_use(
                        &item_id,
                        "Edit",
                        json!({
                            "path": path,
                            "operation": op,
                            "diff": diff,
                        }),
                    ))
                }

                // mcp_tool_call → assistant tool_use by the tool's name
                "mcp_tool_call" => {
                    let tool_name = item
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("mcp_tool")
                        .to_string();
                    // Pass the full item as input (best-effort; fields vary per tool).
                    Some(make_assistant_tool_use(&item_id, &tool_name, item.clone()))
                }

                // Unknown item type → skip
                _ => None,
            }
        }

        // ── turn.completed → result/success ───────────────────────────────────
        "turn.completed" => {
            let usage = codex_line.get("usage");
            let input_tokens = usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output_tokens = usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache_read = usage
                .and_then(|u| u.get("cached_input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            Some(json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "total_cost_usd": 0.0,
                "num_turns": 1,
                "usage": {
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "cache_read_input_tokens": cache_read,
                    "cache_creation_input_tokens": 0,
                },
                "permission_denials": [],
            }))
        }

        // ── turn.failed → result/error ────────────────────────────────────────
        "turn.failed" => {
            let msg = codex_line
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("codex turn failed")
                .to_string();
            Some(json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "result": msg,
                "total_cost_usd": 0.0,
                "num_turns": 1,
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "cache_read_input_tokens": 0,
                    "cache_creation_input_tokens": 0,
                },
                "permission_denials": [],
            }))
        }

        // ── Unknown / other → skip ────────────────────────────────────────────
        _ => None,
    }
}

/// Build an `assistant` event with a single text content block.
fn make_assistant_text(text: &str) -> Value {
    json!({
        "type": "assistant",
        "message": {
            "id": "msg_codex",
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": text,
                }
            ],
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            }
        }
    })
}

/// Build an `assistant` event with a single `tool_use` content block.
fn make_assistant_tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({
        "type": "assistant",
        "message": {
            "id": "msg_codex",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input,
                }
            ],
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            }
        }
    })
}

// =============================================================================
// Unit tests — 100% line/region coverage of every arm
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use marsad_proto::{parse_stream_line, StreamMsg};

    const MODEL: &str = "gpt-5-codex";
    const PROJECT: &str = "/tmp/myproject";
    const MODE: &str = "bypassPermissions";

    fn tr(v: &Value) -> Option<StreamMsg> {
        let s = serde_json::to_string(v).unwrap();
        parse_stream_line(&s).ok()
    }

    // ── thread.started ────────────────────────────────────────────────────────

    #[test]
    fn thread_started_produces_system_init() {
        let line = json!({
            "type": "thread.started",
            "thread_id": "019f-abc",
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["type"], "system");
        assert_eq!(out["subtype"], "init");
        assert_eq!(out["session_id"], "019f-abc");
        assert_eq!(out["model"], MODEL);
        assert_eq!(out["cwd"], PROJECT);
        assert_eq!(out["permissionMode"], MODE);
        // Core compatibility: parse_stream_line must recognize as SystemInit
        assert!(matches!(tr(&out), Some(StreamMsg::SystemInit(_))));
    }

    #[test]
    fn thread_started_missing_thread_id_defaults_empty() {
        let line = json!({ "type": "thread.started" });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["session_id"], "");
    }

    // ── turn.started ──────────────────────────────────────────────────────────

    #[test]
    fn turn_started_is_skipped() {
        let line = json!({ "type": "turn.started" });
        assert!(codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).is_none());
    }

    // ── item.completed agent_message ──────────────────────────────────────────

    #[test]
    fn item_agent_message_produces_assistant_text() {
        let line = json!({
            "type": "item.completed",
            "item": {
                "id": "item_0",
                "type": "agent_message",
                "text": "OK",
            }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["type"], "assistant");
        let content = &out["message"]["content"][0];
        assert_eq!(content["type"], "text");
        assert_eq!(content["text"], "OK");
        // Core compatibility
        assert!(matches!(tr(&out), Some(StreamMsg::Assistant(_))));
    }

    #[test]
    fn item_agent_message_missing_text_defaults_empty() {
        let line = json!({
            "type": "item.completed",
            "item": { "type": "agent_message" }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["message"]["content"][0]["text"], "");
    }

    // ── item.completed reasoning ──────────────────────────────────────────────

    #[test]
    fn item_reasoning_is_skipped() {
        let line = json!({
            "type": "item.completed",
            "item": { "type": "reasoning", "text": "I am thinking..." }
        });
        assert!(codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).is_none());
    }

    // ── item.completed error ──────────────────────────────────────────────────

    #[test]
    fn item_error_produces_codex_error_system_event() {
        let line = json!({
            "type": "item.completed",
            "item": { "type": "error", "message": "model metadata not found" }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["type"], "system");
        assert_eq!(out["subtype"], "codex_error");
        assert_eq!(out["text"], "model metadata not found");
        // Core sees it as Unknown (not a fatal result) — forward-compat
        assert!(matches!(tr(&out), Some(StreamMsg::Unknown(_))));
    }

    #[test]
    fn item_error_missing_message_defaults() {
        let line = json!({
            "type": "item.completed",
            "item": { "type": "error" }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["text"], "codex error");
    }

    // ── item.completed command_execution ─────────────────────────────────────

    #[test]
    fn item_command_execution_produces_bash_tool_use() {
        let line = json!({
            "type": "item.completed",
            "item": {
                "id": "item_1",
                "type": "command_execution",
                "command": "echo hello",
                "aggregated_output": "hello\n",
                "exit_code": 0,
            }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["type"], "assistant");
        let content = &out["message"]["content"][0];
        assert_eq!(content["type"], "tool_use");
        assert_eq!(content["name"], "Bash");
        assert_eq!(content["input"]["command"], "echo hello");
        assert_eq!(content["input"]["output"], "hello\n");
        assert_eq!(content["input"]["exit_code"], 0);
        // Core compatibility
        assert!(matches!(tr(&out), Some(StreamMsg::Assistant(_))));
    }

    #[test]
    fn item_command_execution_missing_fields_best_effort() {
        let line = json!({
            "type": "item.completed",
            "item": { "type": "command_execution" }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["message"]["content"][0]["name"], "Bash");
        assert_eq!(out["message"]["content"][0]["input"]["command"], "");
    }

    // ── item.completed file_change ────────────────────────────────────────────

    #[test]
    fn item_file_change_produces_edit_tool_use() {
        let line = json!({
            "type": "item.completed",
            "item": {
                "id": "item_2",
                "type": "file_change",
                "path": "/tmp/foo.txt",
                "operation": "write",
                "diff": "+hello",
            }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["type"], "assistant");
        let content = &out["message"]["content"][0];
        assert_eq!(content["name"], "Edit");
        assert_eq!(content["input"]["path"], "/tmp/foo.txt");
        assert_eq!(content["input"]["operation"], "write");
        // Core compatibility
        assert!(matches!(tr(&out), Some(StreamMsg::Assistant(_))));
    }

    #[test]
    fn item_file_change_missing_fields_best_effort() {
        let line = json!({
            "type": "item.completed",
            "item": { "type": "file_change" }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["message"]["content"][0]["name"], "Edit");
        assert_eq!(out["message"]["content"][0]["input"]["path"], "");
    }

    // ── item.completed mcp_tool_call ──────────────────────────────────────────

    #[test]
    fn item_mcp_tool_call_produces_named_tool_use() {
        let line = json!({
            "type": "item.completed",
            "item": {
                "id": "item_3",
                "type": "mcp_tool_call",
                "tool_name": "wasl_send",
            }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["type"], "assistant");
        let content = &out["message"]["content"][0];
        assert_eq!(content["name"], "wasl_send");
        // Core compatibility
        assert!(matches!(tr(&out), Some(StreamMsg::Assistant(_))));
    }

    #[test]
    fn item_mcp_tool_call_missing_name_defaults() {
        let line = json!({
            "type": "item.completed",
            "item": { "type": "mcp_tool_call" }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["message"]["content"][0]["name"], "mcp_tool");
    }

    // ── item.completed unknown type ───────────────────────────────────────────

    #[test]
    fn item_unknown_type_is_skipped() {
        let line = json!({
            "type": "item.completed",
            "item": { "type": "web_search", "query": "rust" }
        });
        assert!(codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).is_none());
    }

    // ── item.completed missing item ───────────────────────────────────────────

    #[test]
    fn item_completed_missing_item_is_skipped() {
        let line = json!({ "type": "item.completed" });
        assert!(codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).is_none());
    }

    // ── turn.completed ────────────────────────────────────────────────────────

    #[test]
    fn turn_completed_produces_result_success() {
        let line = json!({
            "type": "turn.completed",
            "usage": {
                "input_tokens": 9065,
                "cached_input_tokens": 3456,
                "output_tokens": 15,
                "reasoning_output_tokens": 8,
            }
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["type"], "result");
        assert_eq!(out["subtype"], "success");
        assert_eq!(out["is_error"], false);
        assert_eq!(out["usage"]["input_tokens"], 9065);
        assert_eq!(out["usage"]["output_tokens"], 15);
        assert_eq!(out["usage"]["cache_read_input_tokens"], 3456);
        // Core compatibility
        assert!(matches!(tr(&out), Some(StreamMsg::Result(_))));
    }

    #[test]
    fn turn_completed_missing_usage_defaults_zeros() {
        let line = json!({ "type": "turn.completed" });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["usage"]["input_tokens"], 0);
        assert_eq!(out["usage"]["output_tokens"], 0);
        assert_eq!(out["usage"]["cache_read_input_tokens"], 0);
    }

    // ── turn.failed ───────────────────────────────────────────────────────────

    #[test]
    fn turn_failed_produces_result_error() {
        let line = json!({
            "type": "turn.failed",
            "error": "rate limit exceeded",
        });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["type"], "result");
        assert_eq!(out["subtype"], "error_during_execution");
        assert_eq!(out["is_error"], true);
        assert_eq!(out["result"], "rate limit exceeded");
        // Core compatibility
        assert!(matches!(tr(&out), Some(StreamMsg::Result(_))));
    }

    #[test]
    fn turn_failed_missing_error_defaults() {
        let line = json!({ "type": "turn.failed" });
        let out = codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).unwrap();
        assert_eq!(out["result"], "codex turn failed");
    }

    // ── unknown / other ───────────────────────────────────────────────────────

    #[test]
    fn unknown_type_is_skipped() {
        let line = json!({ "type": "something.else", "data": 42 });
        assert!(codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).is_none());
    }

    #[test]
    fn missing_type_is_skipped() {
        let line = json!({ "data": 42 });
        assert!(codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).is_none());
    }

    // ── item.started (common non-completed type) ──────────────────────────────

    #[test]
    fn item_started_is_skipped() {
        let line = json!({ "type": "item.started", "item": { "type": "agent_message" } });
        assert!(codex_event_to_stream_json(&line, MODEL, PROJECT, MODE).is_none());
    }
}
