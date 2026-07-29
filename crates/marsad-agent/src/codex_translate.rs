//! Translate codex `app-server` notifications → canonical stream-json (the same
//! shapes `marsad_proto::parse_stream_line` + `marsad_events::reduce` consume for
//! claude/opencode). Pure and unit-testable.
//!
//! Unlike opencode, codex emits BOTH streaming `*/delta` notifications AND a final
//! `item/completed` carrying the full item. Marsad's transcript is built from
//! COMPLETE messages (claude's model), so we translate `item/completed` and IGNORE
//! the deltas — no coalescing needed, and it matches claude's behavior exactly.
//!
//! Item vocabulary + shapes are schema-exact for codex-cli 0.142.5.

use crate::acp_translate::{acp_assistant_message, acp_stop_reason_to_result};
use serde_json::{json, Value};

/// Translate a codex notification (`method` + `params`) into a canonical
/// stream-json event, or `None` to skip (deltas, echoed user messages, unknown).
pub fn codex_notification_to_stream_json(
    method: &str,
    params: &Value,
    session_id: &str,
) -> Option<Value> {
    match method {
        "item/completed" => codex_item_to_stream_json(params.get("item")?),
        "turn/plan/updated" => Some(codex_plan_update(params, session_id)),
        "thread/tokenUsage/updated" => Some(codex_usage_update(params)),
        "turn/completed" => Some(codex_turn_result(params)),
        "thread/compacted" => Some(json!({
            "type": "system",
            "subtype": "compact_boundary",
            "session_id": session_id,
        })),
        // Streaming deltas + echoed user input + everything else → skip; the
        // final `item/completed` (or a dedicated arm) is the source of truth.
        _ => None,
    }
}

/// Translate a completed `ThreadItem` into a transcript stream-json event.
fn codex_item_to_stream_json(item: &Value) -> Option<Value> {
    let itype = item.get("type")?.as_str()?;
    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("codex-item");
    match itype {
        "agentMessage" => {
            let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() {
                return None;
            }
            Some(acp_assistant_message(id, text))
        }
        "reasoning" => {
            // content + summary are arrays of text-bearing parts; flatten.
            let mut text = flatten_text(item.get("summary"));
            if text.is_empty() {
                text = flatten_text(item.get("content"));
            }
            if text.is_empty() {
                return None;
            }
            Some(assistant_thinking(id, &text))
        }
        "commandExecution" => {
            let command = item.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let cwd = item.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            Some(assistant_tool_use(id, "shell", json!({ "command": command, "cwd": cwd })))
        }
        "fileChange" => {
            let changes = item.get("changes").cloned().unwrap_or_else(|| json!([]));
            Some(assistant_tool_use(id, "apply_patch", json!({ "changes": changes })))
        }
        "mcpToolCall" | "dynamicToolCall" => {
            let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
            let args = item.get("arguments").cloned().unwrap_or_else(|| json!({}));
            Some(assistant_tool_use(id, tool, args))
        }
        // userMessage is the echoed prompt (core already recorded it on send, like
        // claude); hookPrompt/webSearch/etc. are not transcript-worthy here.
        _ => None,
    }
}

/// `turn/plan/updated` → canonical `system/plan_update`. Codex's `plan` is an array
/// of steps; map each to `{content, priority}` defensively (field names vary).
fn codex_plan_update(params: &Value, session_id: &str) -> Value {
    let entries = params.get("plan").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let plan_entries: Vec<Value> = entries
        .into_iter()
        .map(|e| {
            let content = e
                .get("content")
                .or_else(|| e.get("step"))
                .or_else(|| e.get("text"))
                .cloned()
                .unwrap_or_else(|| {
                    // a bare string entry
                    if e.is_string() { e.clone() } else { json!("") }
                });
            let priority = e.get("priority").or_else(|| e.get("status")).cloned().unwrap_or(json!(""));
            json!({ "content": content, "priority": priority })
        })
        .collect();
    json!({
        "type": "system",
        "subtype": "plan_update",
        "session_id": session_id,
        "plan_entries": plan_entries,
    })
}

/// `thread/tokenUsage/updated` → canonical `system/usage_update`. `tokenUsage`
/// holds `{total: TokenUsageBreakdown, modelContextWindow}`; sum the breakdown for
/// context_used (defensive on field names) and use modelContextWindow for size.
fn codex_usage_update(params: &Value) -> Value {
    let tu = params.get("tokenUsage");
    let total = tu.and_then(|t| t.get("total"));
    let field = |k: &str| total.and_then(|t| t.get(k)).and_then(|v| v.as_u64()).unwrap_or(0);
    let context_used = field("inputTokens")
        .saturating_add(field("outputTokens"))
        .saturating_add(field("cachedInputTokens"))
        .max(field("totalTokens"));
    let context_size = tu
        .and_then(|t| t.get("modelContextWindow"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    json!({
        "type": "system",
        "subtype": "usage_update",
        "cost_usd": 0.0,
        "context_used": context_used,
        "context_size": context_size,
    })
}

/// `turn/completed` → canonical `result`. Map codex `turn.status`
/// (completed | failed | interrupted | …) to the claude stop-reason vocabulary.
pub fn codex_turn_result(params: &Value) -> Value {
    let status = params
        .get("turn")
        .and_then(|t| t.get("status"))
        .and_then(|v| v.as_str())
        .unwrap_or("completed");
    match status {
        // completed / interrupted(=calm cancel) → reuse the success shapes.
        "completed" => acp_stop_reason_to_result("completed"),
        "interrupted" | "cancelled" | "canceled" => acp_stop_reason_to_result("cancelled"),
        // failed / unknown → a genuine error turn. Build it directly (the codex
        // status isn't in the claude stop-reason vocabulary, so we don't route it
        // through acp_stop_reason_to_result, whose default is success).
        _ => {
            let msg = params
                .get("turn")
                .and_then(|t| t.get("error"))
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("Codex turn failed.");
            json!({
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": true,
                "result": msg,
                "total_cost_usd": 0.0,
                "num_turns": 1,
                "usage": zero_usage(),
                "permission_denials": [],
            })
        }
    }
}

/// Build a canonical `system/modes_update` from a `collaborationMode/list` result
/// (`{data:[CollaborationMode{mode, settings}]}`). Each `mode` becomes an available
/// mode; `current` is the mode whose id equals `current_mode` (or None).
pub fn codex_modes_from_collab_list(result: &Value, session_id: &str, current: Option<&str>) -> Value {
    let modes: Vec<Value> = result
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("mode").and_then(|v| v.as_str())?;
                    Some(json!({ "id": id, "label": id, "description": "" }))
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "type": "system",
        "subtype": "modes_update",
        "session_id": session_id,
        "available_modes": modes,
        "current_mode": current,
    })
}

// ── gates: codex approval ServerRequest → control_request ─────────────────────

/// The globally-unique ROUTING id for a codex approval gate — the request's
/// `itemId` (unique per item) when present, else a session-scoped composite from
/// the raw JSON-RPC id. This becomes the frontend decision_id + core dedup key, so
/// it must be unique (the raw id can repeat across sessions).
pub fn codex_gate_routing_id(sid: &str, raw_id: &str, params: &Value) -> String {
    params
        .get("itemId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{sid}:codex:{raw_id}"))
}

/// Build the stream-json `control_request` that surfaces a codex approval request
/// (`item/commandExecution|fileChange|permissions/requestApproval`, or the legacy
/// `execCommandApproval`/`applyPatchApproval`) as a `can_use_tool` gate. Shape
/// matches `marsad_proto::parse_control_request` (top-level `request_id`, inner
/// `subtype:"can_use_tool"`), identical to the opencode path.
pub fn codex_approval_to_control_request(request_id: &str, method: &str, params: &Value) -> Value {
    let (tool_name, mut input) = match method {
        "item/fileChange/requestApproval" | "applyPatchApproval" => {
            let changes = params
                .get("changes")
                .or_else(|| params.get("fileChanges"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            ("apply_patch", json!({ "changes": changes }))
        }
        "item/permissions/requestApproval" => {
            ("permissions", params.clone())
        }
        // command execution (new + legacy) → a shell gate.
        _ => {
            let command = params.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let cwd = params.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            ("shell", json!({ "command": command, "cwd": cwd }))
        }
    };
    // A human-readable description for the gate body (codex may supply a `reason`).
    if let Some(obj) = input.as_object_mut() {
        let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        let desc = if reason.is_empty() {
            format!("codex requests approval to {tool_name}")
        } else {
            reason.to_string()
        };
        obj.insert("description".to_string(), json!(desc));
    }
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": tool_name,
            "input": input,
            "permission_suggestions": [],
        }
    })
}

/// `item/tool/requestUserInput` → an AskUserQuestion `control_request` (the SAME
/// gate claude's questions use). Codex `questions:[{id,question,header,options}]`
/// map to `input.questions:[{question,header,multiSelect,options:[{label,description}]}]`.
/// The codex `id`s are recovered from the stored params at answer time.
pub fn codex_question_to_control_request(gate_id: &str, params: &Value) -> Value {
    let questions: Vec<Value> = params
        .get("questions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|q| {
                    let options: Vec<Value> = q
                        .get("options")
                        .and_then(|v| v.as_array())
                        .map(|opts| {
                            opts.iter()
                                .map(|o| {
                                    json!({
                                        "label": o.get("label").or_else(|| o.get("value")).cloned().unwrap_or(json!("")),
                                        "description": o.get("description").cloned().unwrap_or(json!("")),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    json!({
                        "question": q.get("question").cloned().unwrap_or(json!("")),
                        "header": q.get("header").cloned().unwrap_or(json!("")),
                        "multiSelect": false,
                        "options": options,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "type": "control_request",
        "request_id": gate_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "input": { "questions": questions },
        }
    })
}

/// `mcpServer/elicitation/request` → an `elicitation` control_request (the SAME
/// gate claude's MCP elicitations use). Answered with `{action, content}`.
pub fn codex_elicitation_to_control_request(gate_id: &str, params: &Value) -> Value {
    json!({
        "type": "control_request",
        "request_id": gate_id,
        "request": {
            "subtype": "elicitation",
            "mcp_server_name": params.get("mode").and_then(|v| v.as_str()).unwrap_or("codex"),
            "message": params.get("message").cloned().unwrap_or(json!("")),
            "requested_schema": params.get("requestedSchema").cloned(),
        }
    })
}

/// Build codex's `{permissions}` response for an `item/permissions/requestApproval`.
/// On allow, grant the requested profile (best-effort passthrough of the params'
/// proposed additions); on deny, grant nothing. `GrantedPermissionProfile` is
/// `{fileSystem?, network?}` (both nullable), so an empty object is a valid "deny".
pub fn codex_permissions_response(allow: bool, params: &Value) -> Value {
    if allow {
        let profile = params
            .get("additionalPermissions")
            .or_else(|| params.get("permissions"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        json!({ "permissions": profile })
    } else {
        json!({ "permissions": {} })
    }
}

/// Build codex's `{answers}` map (keyed by the codex question `id`) from the
/// frontend's answers-by-question-text map + the stored requestUserInput params.
pub fn codex_answers_by_id(params: &Value, answers_by_text: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let (Some(qs), Some(ans)) = (
        params.get("questions").and_then(|v| v.as_array()),
        answers_by_text.as_object(),
    ) {
        for q in qs {
            let text = q.get("question").and_then(|v| v.as_str()).unwrap_or("");
            let id = q.get("id").and_then(|v| v.as_str()).unwrap_or(text);
            if let Some(v) = ans.get(text) {
                out.insert(id.to_string(), v.clone());
            }
        }
    }
    Value::Object(out)
}

// ── canonical builders ────────────────────────────────────────────────────────

fn assistant_thinking(id: &str, thinking: &str) -> Value {
    json!({
        "type": "assistant",
        "message": {
            "id": format!("msg_reasoning_{}", id),
            "role": "assistant",
            "content": [ { "type": "thinking", "thinking": thinking, "signature": "" } ],
            "usage": zero_usage(),
        }
    })
}

fn assistant_tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({
        "type": "assistant",
        "message": {
            "id": format!("msg_tool_{}", id),
            "role": "assistant",
            "content": [ { "type": "tool_use", "id": id, "name": name, "input": input } ],
            "usage": zero_usage(),
        }
    })
}

fn zero_usage() -> Value {
    json!({
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_input_tokens": 0,
        "cache_creation_input_tokens": 0,
    })
}

/// Flatten a codex text-part array (reasoning `content`/`summary`) into a string:
/// concatenates any `{text}` fields or bare strings.
fn flatten_text(arr: Option<&Value>) -> String {
    let Some(a) = arr.and_then(|v| v.as_array()) else { return String::new() };
    let mut out = String::new();
    for part in a {
        if let Some(s) = part.as_str() {
            out.push_str(s);
        } else if let Some(s) = part.get("text").and_then(|v| v.as_str()) {
            out.push_str(s);
        }
    }
    out
}
