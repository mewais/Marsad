//! ACP → Marsad stream-json translation + gate mapping.
//!
//! Turns ACP JSON-RPC messages (from `acp.rs`) into the claude-stream-json
//! `Value`s the rest of Marsad already understands, and maps
//! `session/request_permission` into a Marsad `Gate::Permission`.
//!
//! See docs/specs/opencode-acp.md §3/§4/§8.

use serde_json::{json, Value};

// ── Update → stream-json ─────────────────────────────────────────────────────

/// Translate one `session/update` update object into a claude-stream-json
/// `Value`, or `None` to skip it (unknown update type, or types we silently
/// ignore such as `user_message_chunk`).
///
/// # Parameters
/// - `update`     — the `update` field of the `session/update` notification.
/// - `session_id` — the ACP sessionId (for tagging synthesized events).
/// - `model`      — the model name to embed in synthesized events.
///
/// Every `Some(v)` returned MUST pass `marsad_proto::parse_stream_line`.
pub fn acp_update_to_stream_json(update: &Value, session_id: &str, _model: &str) -> Option<Value> {
    // ACP tags the update kind in `sessionUpdate` (real opencode wire field).
    // Fall back to `type` for legacy/synthetic callers. Reading only `type` — as
    // the original code did — meant EVERY real opencode update returned None, so
    // nothing (assistant text, tool calls, plans) ever reached the transcript.
    let type_str = update
        .get("sessionUpdate")
        .or_else(|| update.get("type"))
        .and_then(|t| t.as_str())?;

    match type_str {
        // ── agent_message_chunk → assistant text block ────────────────────────
        "agent_message_chunk" => {
            let message_id = update
                .get("messageId")
                .and_then(|v| v.as_str())
                .unwrap_or("acp-msg");
            let content = update.get("content")?;
            let content_type = content.get("type").and_then(|t| t.as_str()).unwrap_or("");
            // Only handle text content blocks
            if content_type != "text" {
                return None;
            }
            let text = content
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // Use the messageId as the message id so chunks with the same
            // messageId map to the same assistant message id (for dedup/accumulation).
            Some(json!({
                "type": "assistant",
                "message": {
                    "id": format!("msg_{}", message_id),
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
            }))
        }

        // ── agent_thought_chunk → assistant thinking block ────────────────────
        "agent_thought_chunk" => {
            let thinking = update
                .get("thinking")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let message_id = update
                .get("messageId")
                .and_then(|v| v.as_str())
                .unwrap_or("acp-thought");
            Some(json!({
                "type": "assistant",
                "message": {
                    "id": format!("msg_thought_{}", message_id),
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": thinking,
                            "signature": "",
                        }
                    ],
                    "usage": {
                        "input_tokens": 0,
                        "output_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "cache_creation_input_tokens": 0,
                    }
                }
            }))
        }

        // ── tool_call → assistant tool_use block ──────────────────────────────
        "tool_call" => {
            // opencode's tool_call: { toolCallId, title, kind, status, rawInput }
            // — NOT claude's toolName/toolUseId/input. `title` is the human label
            // ("glob", "README.md"), `kind` the category, `rawInput` the args.
            let tool_id = update
                .get("toolCallId")
                .and_then(|v| v.as_str())
                .unwrap_or("toolu-acp-0");
            let tool_name = update
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| update.get("kind").and_then(|v| v.as_str()))
                .unwrap_or("tool");
            let input = update.get("rawInput").cloned().unwrap_or_else(|| json!({}));
            Some(json!({
                "type": "assistant",
                "message": {
                    "id": format!("msg_tool_{}", tool_id),
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": tool_id,
                            "name": tool_name,
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
            }))
        }

        // ── tool_call_update → assistant tool_use/tool_result block ──────────
        "tool_call_update" => {
            // opencode's tool_call_update: { toolCallId, status, title?, kind?,
            // rawInput?, content? }. On completion it carries a `content` array of
            // { type:"content", content:{ type:"text", text } }. Same field names as
            // tool_call (NOT toolName/toolUseId/input).
            let tool_id = update
                .get("toolCallId")
                .and_then(|v| v.as_str())
                .unwrap_or("toolu-acp-upd");
            let tool_name = update
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| update.get("kind").and_then(|v| v.as_str()))
                .unwrap_or("tool");
            let input = update.get("rawInput").cloned().unwrap_or_else(|| json!({}));

            // content is an array of items; we pass them through so the transcript
            // can show the tool's output.
            let content_arr = update
                .get("content")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            Some(json!({
                "type": "assistant",
                "message": {
                    "id": format!("msg_toolupd_{}", tool_id),
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": tool_id,
                            "name": tool_name,
                            "input": input,
                            "content": content_arr,
                        }
                    ],
                    "usage": {
                        "input_tokens": 0,
                        "output_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "cache_creation_input_tokens": 0,
                    }
                }
            }))
        }

        // ── plan → plan payload ───────────────────────────────────────────────
        "plan" => {
            // Flatten plan.entries into a top-level plan_entries array of
            // {content, priority} objects, as the events reducer expects.
            let entries = update
                .get("plan")
                .and_then(|p| p.get("entries"))
                .and_then(|e| e.as_array())
                .cloned()
                .unwrap_or_default();
            let plan_entries: Vec<Value> = entries
                .into_iter()
                .map(|e| {
                    json!({
                        "content": e.get("content").cloned().unwrap_or(json!("")),
                        "priority": e.get("priority").cloned().unwrap_or(json!(0)),
                    })
                })
                .collect();
            Some(json!({
                "type": "system",
                "subtype": "plan_update",
                "session_id": session_id,
                "plan_entries": plan_entries,
            }))
        }

        // ── available_commands_update → slash commands catalog ────────────────
        "available_commands_update" => {
            let raw_commands = update
                .get("availableCommands")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            // Map each ACP availableCommand {name, description, input?} to
            // {name, description, argument_hint} as the events reducer expects.
            let slash_commands: Vec<Value> = raw_commands
                .into_iter()
                .map(|cmd| {
                    let hint = cmd
                        .get("input")
                        .and_then(|inp| inp.get("hint"))
                        .and_then(|h| h.as_str())
                        .unwrap_or("")
                        .to_string();
                    json!({
                        "name": cmd.get("name").cloned().unwrap_or(json!("")),
                        "description": cmd.get("description").cloned().unwrap_or(json!("")),
                        "argument_hint": hint,
                    })
                })
                .collect();
            Some(json!({
                "type": "system",
                "subtype": "slash_update",
                "session_id": session_id,
                "slash_commands": slash_commands,
            }))
        }

        // ── current_mode_update → current mode ────────────────────────────────
        "current_mode_update" => {
            let current_mode = update
                .get("currentModeId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(json!({
                "type": "system",
                "subtype": "mode_update",
                "session_id": session_id,
                "current_mode": current_mode,
            }))
        }

        // ── usage_update → system/usage_update canonical event ───────────────
        "usage_update" => {
            // ACP shape: {type:"usage_update", cost:{amount, currency}, size, used}
            // Emit canonical: {type:"system", subtype:"usage_update",
            //   cost_usd:<amount if currency==USD else 0>, context_used:<used>, context_size:<size>}
            let cost = update.get("cost");
            let currency = cost
                .and_then(|c| c.get("currency"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let cost_usd = if currency == "USD" {
                cost.and_then(|c| c.get("amount"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            let context_used = update.get("used").and_then(|v| v.as_u64()).unwrap_or(0);
            let context_size = update.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            Some(json!({
                "type": "system",
                "subtype": "usage_update",
                "cost_usd": cost_usd,
                "context_used": context_used,
                "context_size": context_size,
            }))
        }

        // ── user_message_chunk → skip (echoed user content) ──────────────────
        "user_message_chunk" => None,

        // ── context_compaction → compaction event (detected separately via
        //    detect_compaction; here we return None so the caller can use
        //    detect_compaction independently, per the spec §3 + §5 separation).
        //    But if the caller uses acp_update_to_stream_json to get a Value
        //    for broadcast, emit something useful.
        "context_compaction" => {
            // Compaction is handled separately by detect_compaction.
            // Emit the canonical compact_boundary shape the events reducer already
            // handles (increments compactions counter).
            Some(json!({
                "type": "system",
                "subtype": "compact_boundary",
                "session_id": session_id,
            }))
        }

        // ── unknown / future update types → skip ──────────────────────────────
        _ => None,
    }
}

// ── Initialize + session/new → system/init ───────────────────────────────────

/// Synthesize a `system/init` stream-json event from the `initialize` result
/// and the `session/new` (or `session/load`) result.
///
/// # Parameters
/// - `init_result`    — the `result` of the `initialize` response.
/// - `session_result` — the `result` of the `session/new` or `session/load` response.
/// - `model`          — the model name from `LaunchSpec`.
/// - `cwd`            — the project directory.
/// - `project`        — the session name / project label.
///
/// The returned value MUST pass `marsad_proto::parse_stream_line` as a `SystemInit`.
pub fn acp_init_to_system_init(
    _init_result: &Value,
    session_result: &Value,
    model: &str,
    cwd: &str,
    _project: &str,
) -> Value {
    let session_id = session_result
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    json!({
        "type": "system",
        "subtype": "init",
        "session_id": session_id,
        "model": model,
        "cwd": cwd,
        "tools": [],
        "mcp_servers": [],
        "slash_commands": [],
        // Per spec §3: permissionMode — opencode manages its own permissions,
        // we set a sensible default.
        "permissionMode": "default",
    })
}

// ── session/prompt result → result event ─────────────────────────────────────

/// Map a `session/prompt` result's `stopReason` string to a result stream-json
/// event.
///
/// | stopReason                 | Output                                      |
/// |----------------------------|---------------------------------------------|
/// | `"completed"`              | `result/success`                            |
/// | `"cancelled"`              | `result` with interrupt/cancelled semantics |
/// | `"auth_required"`          | `result/error_during_execution`             |
/// | `"request_limit_exceeded"` | `result/error_during_execution`             |
/// | anything else              | `result/error_during_execution`             |
///
/// Build a full `result` stream event from a `session/prompt` RESULT object,
/// mapping BOTH the `stopReason` AND the (UNSTABLE, optional) per-turn
/// `usage: {inputTokens, outputTokens, cachedReadTokens, cachedWriteTokens,
/// thoughtTokens}` into the claude-shaped result usage. Missing `usage` → zeros
/// (defensive: some ACP agents don't populate it yet). Contract signature —
/// implemented in Phase-2 usage hookup; body filled by the coder.
pub fn acp_prompt_result_to_result(result: &Value) -> Value {
    // Extract stopReason and build the base result event using the existing helper.
    let stop_reason = result
        .get("stopReason")
        .and_then(|v| v.as_str())
        .unwrap_or("completed");
    let mut out = acp_stop_reason_to_result(stop_reason);

    // Extract per-turn usage (UNSTABLE/optional — default to zeros if absent).
    let usage = result.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("inputTokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = usage
        .and_then(|u| u.get("outputTokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read = usage
        .and_then(|u| u.get("cachedReadTokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_write = usage
        .and_then(|u| u.get("cachedWriteTokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // Overlay the mapped usage block on the result event.
    // thoughtTokens and totalTokens are informational only — not mapped.
    out["usage"] = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_read_input_tokens": cache_read,
        "cache_creation_input_tokens": cache_write,
    });

    out
}

/// The returned value MUST pass `marsad_proto::parse_stream_line` as a `Result`.
pub fn acp_stop_reason_to_result(stop_reason: &str) -> Value {
    match stop_reason {
        "completed" => json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "total_cost_usd": 0.0,
            "num_turns": 1,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            "permission_denials": [],
        }),
        "cancelled" => json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "total_cost_usd": 0.0,
            "num_turns": 1,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            "permission_denials": [],
        }),
        "auth_required" => json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "result": "Authentication required: please log in to continue.",
            "total_cost_usd": 0.0,
            "num_turns": 1,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            "permission_denials": [],
        }),
        "request_limit_exceeded" => json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "result": "Request limit exceeded: too many requests. Please wait and try again.",
            "total_cost_usd": 0.0,
            "num_turns": 1,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            "permission_denials": [],
        }),
        // The ACP PromptResponse stopReasons — end_turn (the normal success!),
        // max_tokens, max_turn_requests, refusal — and ANY other/unknown reason are
        // turn COMPLETIONS, not errors. Flagging them as errors marked every
        // successful opencode turn as "Error" (only auth_required /
        // request_limit_exceeded above are genuine failures). Default to success so
        // a new/unmapped reason is never false-flagged.
        _ => json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "total_cost_usd": 0.0,
            "num_turns": 1,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
            "permission_denials": [],
        }),
    }
}

// ── session/request_permission → Gate ────────────────────────────────────────


/// Extract the ACP `options[]` array from a `session/request_permission` params
/// and return it as a `Vec<Value>` suitable for `permission_suggestions`.
///
/// Each element is the raw ACP option object `{ optionId, name, kind }`;
/// they are carried VERBATIM — never reshaped.
pub fn acp_options_to_suggestions(options: &Value) -> Vec<Value> {
    options
        .as_array()
        .map(|arr| arr.clone())
        .unwrap_or_default()
}

/// If `update` is an `agent_message_chunk` carrying a text block, return
/// `(messageId, raw_text)`. Text is RAW (untrimmed) so consecutive streamed deltas
/// concatenate with correct spacing. `None` for any other update (or non-text
/// content). Used by the reader to ACCUMULATE opencode's many per-turn deltas into
/// one assistant message instead of emitting one transcript bubble per delta.
pub fn acp_text_chunk(update: &Value) -> Option<(String, String)> {
    let kind = update
        .get("sessionUpdate")
        .or_else(|| update.get("type"))
        .and_then(|v| v.as_str())?;
    if kind != "agent_message_chunk" {
        return None;
    }
    let content = update.get("content")?;
    if content.get("type").and_then(|v| v.as_str()) != Some("text") {
        return None;
    }
    let text = content.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let msg_id = update
        .get("messageId")
        .and_then(|v| v.as_str())
        .unwrap_or("acp-msg")
        .to_string();
    Some((msg_id, text.to_string()))
}

/// Build a complete assistant stream-json message from an accumulated text buffer
/// (same shape the `agent_message_chunk` arm produces, but for the whole message).
pub fn acp_assistant_message(msg_id: &str, text: &str) -> Value {
    json!({
        "type": "assistant",
        "message": {
            "id": format!("msg_{}", msg_id),
            "role": "assistant",
            "content": [ { "type": "text", "text": text } ],
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            }
        }
    })
}

/// The globally-unique ROUTING id for an opencode permission gate — the toolCall's
/// `toolCallId` (unique per tool call) when present, else a session-scoped
/// composite. NEVER the raw ACP JSON-RPC id: that is `0` for the first permission
/// request of every session, so using it as the gate id makes the core dedup
/// ledger treat every session's first gate as "already seen" and silently drop
/// the answer. `acp_request_id` is only the fallback discriminator.
pub fn acp_gate_routing_id(sid: &str, acp_request_id: &str, params: &Value) -> String {
    params
        .get("toolCall")
        .and_then(|tc| tc.get("toolCallId"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{sid}:acp:{acp_request_id}"))
}

/// Build the stream-json `control_request` event that surfaces an opencode ACP
/// `session/request_permission` in the UI as a `can_use_tool` gate.
///
/// The shape MUST match `marsad_proto::parse_stream_line`/`parse_control_request`:
/// `request_id` at the TOP LEVEL, and the inner `request` keyed by **`subtype`**
/// (not `type`) == `"can_use_tool"`. An earlier hand-rolled shape (nested
/// `request_id`, inner `type`) never parsed, so no gate ever surfaced and every
/// opencode permission request left the session stuck "working". This helper
/// exists so that contract is unit-tested (round-tripped through parse_stream_line).
///
/// opencode's `toolCall` carries `title`/`kind`/`toolCallId`/`rawInput` — there is
/// NO `toolName`/`input` — so we read those; `options` become
/// `permission_suggestions` verbatim (each `{ optionId, name, kind }`).
pub fn acp_permission_to_control_request_event(request_id: &str, params: &Value) -> Value {
    let tool_call = params.get("toolCall").cloned().unwrap_or_else(|| json!({}));
    let kind = tool_call.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let title = tool_call.get("title").and_then(|v| v.as_str()).unwrap_or("");

    // Build the gate `input` from opencode's `rawInput` so the UI shows WHAT is
    // being approved, not just a bare path:
    //  - `tool_name` = the action KIND (read / edit / execute / other …) so the
    //    user sees the category (the whole point of issue: "is it bash or read?").
    //  - normalize opencode's `filepath` → `path` so describe_tool_request renders
    //    "<kind> <path>" for the command line (execute already carries `command`,
    //    shown verbatim).
    //  - `description` = "<kind>: <title>" so the explanation body states the action
    //    + opencode's own human summary.
    let mut input = tool_call
        .get("rawInput")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    if !input.contains_key("command")
        && !input.contains_key("path")
        && !input.contains_key("file_path")
    {
        if let Some(fp) = input.get("filepath").cloned() {
            input.insert("path".to_string(), fp);
        }
    }
    let description = match (kind.is_empty(), title.is_empty()) {
        (false, false) => format!("{kind}: {title}"),
        (true, false) => title.to_string(),
        (false, true) => format!("permission to {kind}"),
        (true, true) => "opencode permission request".to_string(),
    };
    input.insert("description".to_string(), json!(description));

    let tool_name = if kind.is_empty() { "opencode" } else { kind };
    let permission_suggestions =
        acp_options_to_suggestions(&params.get("options").cloned().unwrap_or_else(|| json!([])));
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": tool_name,
            "input": Value::Object(input),
            "permission_suggestions": permission_suggestions,
        }
    })
}

// ── Compaction detection ──────────────────────────────────────────────────────

/// Return `true` if `update` is opencode's compaction notification.
///
/// Opencode emits compaction as a `session/update` update object. This function
/// detects it (via `_meta` or a dedicated `type` value) so the caller can
/// increment `SessionState.compactions` / `last_compaction`.
pub fn detect_compaction(update: &Value) -> bool {
    // Check if type is "context_compaction" (dedicated type field).
    if update.get("type").and_then(|t| t.as_str()) == Some("context_compaction") {
        return true;
    }

    // Check if _meta.compacted == true (meta shape).
    if let Some(meta) = update.get("_meta") {
        if meta.get("compacted").and_then(|v| v.as_bool()) == Some(true) {
            return true;
        }
    }

    false
}
