//! acp_protocol.rs — TDD red-phase tests for the ACP JSON-RPC protocol layer.
//!
//! Spec: docs/specs/opencode-acp.md §2 (ACP lifecycle), §8 (module/API contract),
//! §9 (test plan).
//!
//! All functions under test live in `marsad_agent::acp` (fully covered here)
//! and `marsad_agent::opencode_argv` (see the `opencode_argv` section below).
//!
//! `opencode_argv` is re-exported via `pub use driver::opencode_argv` in lib.rs.
//! All tests here compile and FAIL at runtime via todo!() panic — that is the
//! expected RED signal.

use marsad_agent::acp::{
    encode_notification, encode_request, encode_response, extract_session_id, extract_stop_reason,
    extract_update, extract_update_type, initialize_params, is_permission_request,
    parse_acp_line, request_permission_response, session_cancel_params, session_load_params,
    session_new_params, session_prompt_params, AcpError, AcpMessage,
};
use serde_json::{json, Value};

// ── parse_acp_line: Request ───────────────────────────────────────────────────

#[test]
fn parse_request_classifies_correctly() {
    let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1"}}"#;
    let msg = parse_acp_line(line).expect("should parse a request");
    match msg {
        AcpMessage::Request { id, method, params } => {
            assert_eq!(id, json!(1));
            assert_eq!(method, "initialize");
            assert_eq!(params["protocolVersion"], json!("1"));
        }
        other => panic!("expected Request, got {:?}", other),
    }
}

#[test]
fn parse_request_with_string_id() {
    let line = r#"{"jsonrpc":"2.0","id":"req-abc","method":"session/new","params":{"cwd":"/work","mcpServers":[]}}"#;
    let msg = parse_acp_line(line).expect("should parse a request with string id");
    match msg {
        AcpMessage::Request { id, method, params } => {
            assert_eq!(id, json!("req-abc"));
            assert_eq!(method, "session/new");
            assert_eq!(params["cwd"], json!("/work"));
        }
        other => panic!("expected Request, got {:?}", other),
    }
}

#[test]
fn parse_request_method_session_prompt() {
    let line = r#"{"jsonrpc":"2.0","id":42,"method":"session/prompt","params":{"sessionId":"s1","prompt":[{"type":"text","text":"hello"}]}}"#;
    let msg = parse_acp_line(line).expect("should parse session/prompt request");
    match msg {
        AcpMessage::Request { id, method, params } => {
            assert_eq!(id, json!(42));
            assert_eq!(method, "session/prompt");
            assert_eq!(params["sessionId"], json!("s1"));
        }
        other => panic!("expected Request, got {:?}", other),
    }
}

// ── parse_acp_line: Response (success) ───────────────────────────────────────

#[test]
fn parse_success_response_classifies_correctly() {
    let line = r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"sess-123","modes":null}}"#;
    let msg = parse_acp_line(line).expect("should parse a success response");
    match msg {
        AcpMessage::Response { id, result } => {
            assert_eq!(id, json!(1));
            let r = result.expect("expected Ok result");
            assert_eq!(r["sessionId"], json!("sess-123"));
        }
        other => panic!("expected Response, got {:?}", other),
    }
}

#[test]
fn parse_success_response_with_stop_reason() {
    let line = r#"{"jsonrpc":"2.0","id":10,"result":{"stopReason":"completed"}}"#;
    let msg = parse_acp_line(line).expect("should parse session/prompt response");
    match msg {
        AcpMessage::Response { id, result } => {
            assert_eq!(id, json!(10));
            let r = result.expect("expected Ok result");
            assert_eq!(r["stopReason"], json!("completed"));
        }
        other => panic!("expected Response, got {:?}", other),
    }
}

// ── parse_acp_line: Response (error) ─────────────────────────────────────────

#[test]
fn parse_error_response_classifies_correctly() {
    let line = r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32600,"message":"Invalid Request"}}"#;
    let msg = parse_acp_line(line).expect("should parse an error response");
    match msg {
        AcpMessage::Response { id, result } => {
            assert_eq!(id, json!(5));
            let err = result.expect_err("expected Err result for error response");
            assert_eq!(err["code"], json!(-32600));
            assert_eq!(err["message"], json!("Invalid Request"));
        }
        other => panic!("expected Response, got {:?}", other),
    }
}

#[test]
fn parse_error_response_with_string_id() {
    let line = r#"{"jsonrpc":"2.0","id":"req-xyz","error":{"code":-32601,"message":"Method not found"}}"#;
    let msg = parse_acp_line(line).expect("should parse error response with string id");
    match msg {
        AcpMessage::Response { id, result } => {
            assert_eq!(id, json!("req-xyz"));
            let err = result.expect_err("expected Err result");
            assert_eq!(err["message"], json!("Method not found"));
        }
        other => panic!("expected Response, got {:?}", other),
    }
}

// ── parse_acp_line: Notification ──────────────────────────────────────────────

#[test]
fn parse_notification_classifies_correctly() {
    let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"type":"agent_message_chunk","content":{"type":"text","text":"Hi"}}}}"#;
    let msg = parse_acp_line(line).expect("should parse a notification");
    match msg {
        AcpMessage::Notification { method, params } => {
            assert_eq!(method, "session/update");
            assert_eq!(params["sessionId"], json!("s1"));
        }
        other => panic!("expected Notification, got {:?}", other),
    }
}

#[test]
fn parse_notification_no_id_field() {
    // A notification MUST NOT have an id field; if no id and has method+params → Notification.
    let line = r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s2"}}"#;
    let msg = parse_acp_line(line).expect("should parse notification with no id");
    match msg {
        AcpMessage::Notification { method, params } => {
            assert_eq!(method, "session/cancel");
            assert_eq!(params["sessionId"], json!("s2"));
        }
        other => panic!("expected Notification, got {:?}", other),
    }
}

// ── parse_acp_line: Malformed / Unrecognized ──────────────────────────────────

#[test]
fn parse_invalid_json_returns_invalid_json_error() {
    let line = "not valid json at all {{{";
    let err = parse_acp_line(line).expect_err("should fail on invalid JSON");
    match err {
        AcpError::InvalidJson(_) => {} // correct variant
        AcpError::UnrecognizedShape(s) => panic!("expected InvalidJson, got UnrecognizedShape({s})"),
    }
}

#[test]
fn parse_empty_string_returns_invalid_json_error() {
    let err = parse_acp_line("").expect_err("empty string is not valid JSON");
    match err {
        AcpError::InvalidJson(_) => {}
        AcpError::UnrecognizedShape(s) => panic!("expected InvalidJson, got UnrecognizedShape({s})"),
    }
}

#[test]
fn parse_json_array_returns_unrecognized_shape() {
    // Valid JSON but not an object → not ACP-shaped.
    let err = parse_acp_line("[1,2,3]").expect_err("array is not an ACP message");
    match err {
        AcpError::UnrecognizedShape(_) => {}
        AcpError::InvalidJson(s) => panic!("expected UnrecognizedShape, got InvalidJson({s})"),
    }
}

#[test]
fn parse_json_object_without_method_or_result_returns_unrecognized_shape() {
    // Valid JSON object, has jsonrpc, but no method/result/error → unrecognized.
    let err = parse_acp_line(r#"{"jsonrpc":"2.0","foo":"bar"}"#)
        .expect_err("bare object without method/result is not ACP-shaped");
    match err {
        AcpError::UnrecognizedShape(_) => {}
        AcpError::InvalidJson(s) => panic!("expected UnrecognizedShape, got InvalidJson({s})"),
    }
}

#[test]
fn parse_json_missing_jsonrpc_version_returns_unrecognized_shape() {
    // Object has method+params but no jsonrpc field → not ACP (strict 2.0 check).
    let err = parse_acp_line(r#"{"method":"session/update","params":{}}"#)
        .expect_err("missing jsonrpc field should be unrecognized");
    match err {
        AcpError::UnrecognizedShape(_) => {}
        AcpError::InvalidJson(s) => panic!("expected UnrecognizedShape, got InvalidJson({s})"),
    }
}

// ── encode_request ────────────────────────────────────────────────────────────

#[test]
fn encode_request_produces_valid_json_rpc_2_0() {
    let encoded = encode_request(json!(1), "initialize", json!({"protocolVersion":"1"}));
    let v: Value = serde_json::from_str(&encoded).expect("encoded request must be valid JSON");
    assert_eq!(v["jsonrpc"], json!("2.0"));
    assert_eq!(v["id"], json!(1));
    assert_eq!(v["method"], json!("initialize"));
    assert_eq!(v["params"]["protocolVersion"], json!("1"));
}

#[test]
fn encode_request_is_newline_free() {
    let encoded = encode_request(json!(99), "session/new", json!({"cwd":"/tmp","mcpServers":[]}));
    assert!(!encoded.contains('\n'), "encoded request must not contain newline");
    assert!(!encoded.contains('\r'), "encoded request must not contain carriage return");
}

#[test]
fn encode_request_round_trip_via_parse() {
    let original_id = json!("corr-1");
    let original_method = "session/prompt";
    let original_params = json!({"sessionId":"s1","prompt":[{"type":"text","text":"hi"}]});
    let encoded = encode_request(original_id.clone(), original_method, original_params.clone());
    let msg = parse_acp_line(&encoded).expect("encoded request must be parseable");
    match msg {
        AcpMessage::Request { id, method, params } => {
            assert_eq!(id, original_id);
            assert_eq!(method, original_method);
            assert_eq!(params, original_params);
        }
        other => panic!("expected Request after round-trip, got {:?}", other),
    }
}

#[test]
fn encode_request_with_numeric_id() {
    let encoded = encode_request(json!(42), "session/load", json!({"sessionId":"s42","cwd":"/x","mcpServers":[]}));
    let v: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(v["id"], json!(42));
    assert_eq!(v["method"], json!("session/load"));
    // Must NOT carry a result or error field.
    assert!(v.get("result").is_none(), "request must not have result field");
    assert!(v.get("error").is_none(), "request must not have error field");
}

// ── encode_response ───────────────────────────────────────────────────────────

#[test]
fn encode_response_produces_valid_json_rpc_2_0() {
    let encoded = encode_response(json!(1), json!({"sessionId":"s1"}));
    let v: Value = serde_json::from_str(&encoded).expect("encoded response must be valid JSON");
    assert_eq!(v["jsonrpc"], json!("2.0"));
    assert_eq!(v["id"], json!(1));
    assert_eq!(v["result"]["sessionId"], json!("s1"));
}

#[test]
fn encode_response_is_newline_free() {
    let encoded = encode_response(json!(2), json!({"stopReason":"completed"}));
    assert!(!encoded.contains('\n'), "encoded response must not contain newline");
    assert!(!encoded.contains('\r'), "encoded response must not contain carriage return");
}

#[test]
fn encode_response_round_trip_via_parse() {
    let original_id = json!(7);
    let original_result = json!({"outcome":"selected","optionId":"allow_once"});
    let encoded = encode_response(original_id.clone(), original_result.clone());
    let msg = parse_acp_line(&encoded).expect("encoded response must be parseable");
    match msg {
        AcpMessage::Response { id, result } => {
            assert_eq!(id, original_id);
            let r = result.expect("expected Ok result");
            assert_eq!(r, original_result);
        }
        other => panic!("expected Response after round-trip, got {:?}", other),
    }
}

#[test]
fn encode_response_no_method_no_error_field() {
    let encoded = encode_response(json!(3), json!({"agentCapabilities":{}}));
    let v: Value = serde_json::from_str(&encoded).unwrap();
    assert!(v.get("method").is_none(), "response must not have method field");
    assert!(v.get("error").is_none(), "response must not have error field");
}

// ── encode_notification ───────────────────────────────────────────────────────

#[test]
fn encode_notification_produces_valid_json_rpc_2_0() {
    let encoded = encode_notification("session/cancel", json!({"sessionId":"s1"}));
    let v: Value = serde_json::from_str(&encoded).expect("encoded notification must be valid JSON");
    assert_eq!(v["jsonrpc"], json!("2.0"));
    assert_eq!(v["method"], json!("session/cancel"));
    assert_eq!(v["params"]["sessionId"], json!("s1"));
}

#[test]
fn encode_notification_has_no_id_field() {
    let encoded = encode_notification("session/update", json!({"sessionId":"s2","update":{}}));
    let v: Value = serde_json::from_str(&encoded).unwrap();
    assert!(v.get("id").is_none(), "notification must not have an id field");
}

#[test]
fn encode_notification_is_newline_free() {
    let encoded = encode_notification("session/cancel", json!({"sessionId":"abc"}));
    assert!(!encoded.contains('\n'), "encoded notification must not contain newline");
}

#[test]
fn encode_notification_round_trip_via_parse() {
    let original_method = "session/update";
    let original_params = json!({"sessionId":"s3","update":{"type":"agent_message_chunk","content":{"type":"text","text":"hello"}}});
    let encoded = encode_notification(original_method, original_params.clone());
    let msg = parse_acp_line(&encoded).expect("encoded notification must be parseable");
    match msg {
        AcpMessage::Notification { method, params } => {
            assert_eq!(method, original_method);
            assert_eq!(params, original_params);
        }
        other => panic!("expected Notification after round-trip, got {:?}", other),
    }
}

// ── initialize_params ─────────────────────────────────────────────────────────

#[test]
fn initialize_params_has_protocol_version() {
    let p = initialize_params("Marsad", "1.0.0");
    assert!(p.get("protocolVersion").is_some(), "initialize_params must include protocolVersion");
    // ACP protocolVersion is an INTEGER (1), not a string — opencode rejects a
    // string with -32602 "expected number, received string" (verified vs opencode 1.18).
    assert_eq!(p["protocolVersion"], json!(1));
}

#[test]
fn initialize_params_declines_fs_read() {
    let p = initialize_params("Marsad", "1.0.0");
    assert_eq!(
        p["clientCapabilities"]["fs"]["readTextFile"],
        json!(false),
        "initialize_params must decline fs.readTextFile"
    );
}

#[test]
fn initialize_params_declines_fs_write() {
    let p = initialize_params("Marsad", "1.0.0");
    assert_eq!(
        p["clientCapabilities"]["fs"]["writeTextFile"],
        json!(false),
        "initialize_params must decline fs.writeTextFile"
    );
}

#[test]
fn initialize_params_declines_terminal() {
    let p = initialize_params("Marsad", "1.0.0");
    assert_eq!(
        p["clientCapabilities"]["terminal"],
        json!(false),
        "initialize_params must decline terminal"
    );
}

#[test]
fn initialize_params_carries_client_info() {
    let p = initialize_params("Marsad", "2.3.1");
    let info = &p["clientInfo"];
    assert!(info.is_object(), "clientInfo must be an object");
    // Name and version should appear somewhere in clientInfo
    let info_str = info.to_string();
    assert!(info_str.contains("Marsad"), "clientInfo must carry the client name");
    assert!(info_str.contains("2.3.1"), "clientInfo must carry the client version");
}

// ── session_new_params ────────────────────────────────────────────────────────

#[test]
fn session_new_params_carries_cwd() {
    let p = session_new_params("/workspace/myproject", &json!([]), None);
    assert_eq!(p["cwd"], json!("/workspace/myproject"));
}

#[test]
fn session_new_params_carries_mcp_servers() {
    let servers = json!([{"name":"myserver","command":"npx","args":["--yes","@company/mcp"]}]);
    let p = session_new_params("/proj", &servers, None);
    assert_eq!(p["mcpServers"], servers);
}

#[test]
fn session_new_params_without_additional_dirs() {
    let p = session_new_params("/proj", &json!([]), None);
    // additionalDirectories either absent or null when not provided
    let additional = p.get("additionalDirectories");
    let is_absent_or_null = additional.is_none() || additional == Some(&Value::Null);
    assert!(is_absent_or_null, "additionalDirectories must be absent or null when not provided");
}

#[test]
fn session_new_params_with_additional_dirs() {
    let dirs = json!(["/extra/dir1", "/extra/dir2"]);
    let p = session_new_params("/proj", &json!([]), Some(&dirs));
    assert_eq!(p["additionalDirectories"], dirs);
}

// ── session_load_params ───────────────────────────────────────────────────────

#[test]
fn session_load_params_carries_session_id() {
    let p = session_load_params("sess-xyz", "/proj", &json!([]));
    assert_eq!(p["sessionId"], json!("sess-xyz"));
}

#[test]
fn session_load_params_carries_cwd() {
    let p = session_load_params("sess-xyz", "/my/project", &json!([]));
    assert_eq!(p["cwd"], json!("/my/project"));
}

#[test]
fn session_load_params_carries_mcp_servers() {
    let servers = json!([{"name":"s","command":"cmd","args":[]}]);
    let p = session_load_params("sess-xyz", "/proj", &servers);
    assert_eq!(p["mcpServers"], servers);
}

// ── session_prompt_params ─────────────────────────────────────────────────────

#[test]
fn session_prompt_params_carries_session_id() {
    let p = session_prompt_params("sess-abc", "What is 2+2?");
    assert_eq!(p["sessionId"], json!("sess-abc"));
}

#[test]
fn session_prompt_params_carries_text_content_block() {
    let p = session_prompt_params("sess-abc", "What is 2+2?");
    let prompt = p["prompt"].as_array().expect("prompt must be an array");
    assert!(!prompt.is_empty(), "prompt must have at least one content block");
    let block = &prompt[0];
    assert_eq!(block["type"], json!("text"), "content block type must be 'text'");
    assert_eq!(block["text"], json!("What is 2+2?"), "content block text must match the input");
}

#[test]
fn session_prompt_params_multiline_text_preserved() {
    let text = "Line one\nLine two\nLine three";
    let p = session_prompt_params("s1", text);
    let prompt = p["prompt"].as_array().unwrap();
    assert_eq!(prompt[0]["text"], json!(text));
}

// ── session_cancel_params ─────────────────────────────────────────────────────

#[test]
fn session_cancel_params_carries_session_id() {
    let p = session_cancel_params("sess-cancel-me");
    assert_eq!(p["sessionId"], json!("sess-cancel-me"));
}

// ── request_permission_response ───────────────────────────────────────────────

#[test]
fn request_permission_response_selected_when_option_provided() {
    let r = request_permission_response(Some("opt-allow-once"));
    assert_eq!(r["outcome"], json!("selected"));
    assert_eq!(r["optionId"], json!("opt-allow-once"));
}

#[test]
fn request_permission_response_cancelled_when_none() {
    let r = request_permission_response(None);
    assert_eq!(r["outcome"], json!("cancelled"));
    assert!(r.get("optionId").is_none() || r["optionId"].is_null(),
        "cancelled response must not have optionId (or it must be null)");
}

#[test]
fn request_permission_response_selected_preserves_option_id_exactly() {
    let r = request_permission_response(Some("allow_always"));
    assert_eq!(r["optionId"], json!("allow_always"));
}

#[test]
fn request_permission_response_selected_reject_option() {
    let r = request_permission_response(Some("reject_once"));
    assert_eq!(r["outcome"], json!("selected"));
    assert_eq!(r["optionId"], json!("reject_once"));
}

// ── is_permission_request ─────────────────────────────────────────────────────

#[test]
fn is_permission_request_true_for_correct_method() {
    assert!(is_permission_request("session/request_permission"));
}

#[test]
fn is_permission_request_false_for_initialize() {
    assert!(!is_permission_request("initialize"));
}

#[test]
fn is_permission_request_false_for_session_update() {
    assert!(!is_permission_request("session/update"));
}

#[test]
fn is_permission_request_false_for_session_prompt() {
    assert!(!is_permission_request("session/prompt"));
}

#[test]
fn is_permission_request_false_for_empty_string() {
    assert!(!is_permission_request(""));
}

#[test]
fn is_permission_request_false_for_partial_match() {
    // Must be an exact match, not a substring match.
    assert!(!is_permission_request("request_permission"));
    assert!(!is_permission_request("session/request_permissions")); // trailing 's'
}

// ── extract_session_id ────────────────────────────────────────────────────────

#[test]
fn extract_session_id_from_session_new_result() {
    let result = json!({"sessionId": "my-sess-001", "modes": null});
    let id = extract_session_id(&result);
    assert_eq!(id.as_deref(), Some("my-sess-001"));
}

#[test]
fn extract_session_id_returns_none_when_absent() {
    let result = json!({"stopReason": "completed"});
    let id = extract_session_id(&result);
    assert!(id.is_none(), "extract_session_id must return None when sessionId is absent");
}

#[test]
fn extract_session_id_returns_none_when_not_string() {
    let result = json!({"sessionId": 42});
    let id = extract_session_id(&result);
    assert!(id.is_none(), "extract_session_id must return None when sessionId is not a string");
}

#[test]
fn extract_session_id_returns_none_for_null() {
    let result = json!({"sessionId": null});
    let id = extract_session_id(&result);
    assert!(id.is_none(), "extract_session_id must return None when sessionId is null");
}

// ── extract_stop_reason ───────────────────────────────────────────────────────

#[test]
fn extract_stop_reason_completed() {
    let result = json!({"stopReason": "completed"});
    let reason = extract_stop_reason(&result);
    assert_eq!(reason.as_deref(), Some("completed"));
}

#[test]
fn extract_stop_reason_cancelled() {
    let result = json!({"stopReason": "cancelled"});
    let reason = extract_stop_reason(&result);
    assert_eq!(reason.as_deref(), Some("cancelled"));
}

#[test]
fn extract_stop_reason_auth_required() {
    let result = json!({"stopReason": "auth_required"});
    let reason = extract_stop_reason(&result);
    assert_eq!(reason.as_deref(), Some("auth_required"));
}

#[test]
fn extract_stop_reason_request_limit_exceeded() {
    let result = json!({"stopReason": "request_limit_exceeded"});
    let reason = extract_stop_reason(&result);
    assert_eq!(reason.as_deref(), Some("request_limit_exceeded"));
}

#[test]
fn extract_stop_reason_returns_none_when_absent() {
    let result = json!({"sessionId": "s1"});
    let reason = extract_stop_reason(&result);
    assert!(reason.is_none(), "extract_stop_reason must return None when stopReason is absent");
}

#[test]
fn extract_stop_reason_returns_none_when_not_string() {
    let result = json!({"stopReason": 0});
    let reason = extract_stop_reason(&result);
    assert!(reason.is_none(), "extract_stop_reason must return None when stopReason is not a string");
}

// ── extract_update ────────────────────────────────────────────────────────────

#[test]
fn extract_update_returns_update_object() {
    let params = json!({"sessionId":"s1","update":{"type":"agent_message_chunk","content":{"type":"text","text":"hi"}}});
    let update = extract_update(&params).expect("extract_update must return the update object");
    assert_eq!(update["type"], json!("agent_message_chunk"));
}

#[test]
fn extract_update_returns_none_when_absent() {
    let params = json!({"sessionId":"s1"});
    let update = extract_update(&params);
    assert!(update.is_none(), "extract_update must return None when update field is absent");
}

#[test]
fn extract_update_tool_call_type() {
    let params = json!({
        "sessionId": "s2",
        "update": {
            "type": "tool_call",
            "toolName": "bash",
            "toolUseId": "tu-001",
            "input": "{\"command\":\"ls\"}"
        }
    });
    let update = extract_update(&params).expect("update present");
    assert_eq!(update["type"], json!("tool_call"));
    assert_eq!(update["toolName"], json!("bash"));
}

#[test]
fn extract_update_thought_chunk_type() {
    let params = json!({"sessionId":"s3","update":{"type":"agent_thought_chunk","thinking":"reasoning here"}});
    let update = extract_update(&params).expect("update present");
    assert_eq!(update["type"], json!("agent_thought_chunk"));
    assert_eq!(update["thinking"], json!("reasoning here"));
}

// ── extract_update_type ───────────────────────────────────────────────────────

#[test]
fn extract_update_type_returns_type_string() {
    let update = json!({"type":"agent_message_chunk","content":{"type":"text","text":"hello"}});
    let t = extract_update_type(&update).expect("update type must be present");
    assert_eq!(t, "agent_message_chunk");
}

#[test]
fn extract_update_type_tool_call() {
    let update = json!({"type":"tool_call","toolName":"write_file","toolUseId":"tu-abc","input":"{}"});
    let t = extract_update_type(&update).expect("update type must be present");
    assert_eq!(t, "tool_call");
}

#[test]
fn extract_update_type_returns_none_when_absent() {
    let update = json!({"content":"something"});
    let t = extract_update_type(&update);
    assert!(t.is_none(), "extract_update_type must return None when type field is absent");
}

#[test]
fn extract_update_type_returns_none_when_not_string() {
    let update = json!({"type": 99});
    let t = extract_update_type(&update);
    assert!(t.is_none(), "extract_update_type must return None when type is not a string");
}

#[test]
fn extract_update_type_plan() {
    let update = json!({"type":"plan","plan":{"entries":[{"content":"step 1","priority":"high"}]}});
    let t = extract_update_type(&update).expect("type present");
    assert_eq!(t, "plan");
}

#[test]
fn extract_update_type_available_commands_update() {
    let update = json!({"type":"available_commands_update","availableCommands":[]});
    let t = extract_update_type(&update).expect("type present");
    assert_eq!(t, "available_commands_update");
}

// ── opencode_argv ─────────────────────────────────────────────────────────────

mod opencode_argv_tests {
    use super::*;
    use marsad_agent::opencode_argv;
    use marsad_proto::{LaunchMode, LaunchSpec, Posture};

    /// Build a LaunchSpec for the opencode driver (used by opencode_argv tests).
    fn opencode_spec(model: &str, bin: Option<&str>) -> LaunchSpec {
        LaunchSpec {
            project: "/proj".to_string(),
            host_id: "local".to_string(),
            model: model.to_string(),
            posture: Posture::Ask,
            first_prompt: None,
            mode: LaunchMode::Fresh,
            host_target: None,
            claude_bin: bin.map(|s| s.to_string()),
            setup: None,
            env: vec![],
            name: None,
            disallowed_tools: vec![],
            wasl: false,
            driver: "opencode".to_string(),
        }
    }

    #[test]
    fn opencode_argv_includes_acp_subcommand() {
        let spec = opencode_spec("claude-opus-4-5", None);
        let argv = opencode_argv(&spec);
        // The argv must include "acp" (first arg after the bin, or as the bin's subcommand).
        assert!(
            argv.contains(&"acp".to_string()),
            "opencode_argv must include 'acp' subcommand; got: {:?}",
            argv
        );
    }

    #[test]
    fn opencode_argv_default_bin_is_opencode() {
        let spec = opencode_spec("claude-opus-4-5", None);
        let argv = opencode_argv(&spec);
        assert!(!argv.is_empty(), "argv must not be empty");
        assert_eq!(
            argv[0], "opencode",
            "default binary must be 'opencode' when claude_bin is None; got: {:?}",
            argv
        );
    }

    #[test]
    fn opencode_argv_custom_bin_overrides_default() {
        let spec = opencode_spec("claude-opus-4-5", Some("/usr/local/bin/opencode"));
        let argv = opencode_argv(&spec);
        assert_eq!(argv[0], "/usr/local/bin/opencode");
    }

    #[test]
    fn opencode_argv_omits_model_flag() {
        // `opencode acp` REJECTS `--model` (verified against opencode 1.18: the
        // acp subcommand's yargs is strict and prints usage + exits on an unknown
        // flag, in either position). Passing it killed every model-selected launch.
        // So the model must NOT appear in the argv at all — opencode owns model
        // selection in ACP mode.
        let spec = opencode_spec("anthropic/claude-opus-4-5", None);
        let argv = opencode_argv(&spec);
        assert!(
            !argv.iter().any(|a| a == "--model" || a == "-m"),
            "opencode_argv must NOT pass a --model/-m flag (acp rejects it); got: {:?}",
            argv
        );
        assert!(
            !argv.contains(&"anthropic/claude-opus-4-5".to_string()),
            "opencode_argv must NOT thread the model id (acp has no model flag); got: {:?}",
            argv
        );
    }

    #[test]
    fn opencode_argv_is_exactly_bin_then_acp() {
        // The whole invocation is `<bin> acp` — nothing else. Any extra arg risks
        // the acp subcommand rejecting it (strict yargs).
        let spec = opencode_spec("some-model", None);
        let argv = opencode_argv(&spec);
        assert_eq!(
            argv,
            vec!["opencode".to_string(), "acp".to_string()],
            "opencode_argv must be exactly [bin, \"acp\"]; got: {:?}",
            argv
        );
    }

    #[test]
    fn opencode_argv_no_stream_json_flags() {
        // opencode ACP does NOT use claude's --input-format / --output-format stream-json.
        let spec = opencode_spec("mymodel", None);
        let argv = opencode_argv(&spec);
        assert!(
            !argv.contains(&"--input-format".to_string()),
            "opencode_argv must not include --input-format"
        );
        assert!(
            !argv.contains(&"--output-format".to_string()),
            "opencode_argv must not include --output-format"
        );
        assert!(
            !argv.contains(&"stream-json".to_string()),
            "opencode_argv must not include stream-json"
        );
    }
}

// ── ID correlation: encode then parse preserves IDs ───────────────────────────

#[test]
fn id_correlation_integer_survives_round_trip() {
    let id = json!(12345);
    let encoded = encode_request(id.clone(), "session/cancel", json!({"sessionId":"s1"}));
    let msg = parse_acp_line(&encoded).unwrap();
    match msg {
        AcpMessage::Request { id: parsed_id, .. } => assert_eq!(parsed_id, id),
        other => panic!("expected Request, got {:?}", other),
    }
}

#[test]
fn id_correlation_string_survives_round_trip() {
    let id = json!("uuid-aaaa-bbbb-cccc");
    let encoded = encode_request(id.clone(), "initialize", json!({"protocolVersion":"1"}));
    let msg = parse_acp_line(&encoded).unwrap();
    match msg {
        AcpMessage::Request { id: parsed_id, .. } => assert_eq!(parsed_id, id),
        other => panic!("expected Request, got {:?}", other),
    }
}

#[test]
fn id_correlation_response_id_survives_round_trip() {
    let id = json!(99);
    let encoded = encode_response(id.clone(), json!({"outcome":"cancelled"}));
    let msg = parse_acp_line(&encoded).unwrap();
    match msg {
        AcpMessage::Response { id: parsed_id, result: Ok(_) } => assert_eq!(parsed_id, id),
        other => panic!("expected Response Ok, got {:?}", other),
    }
}
