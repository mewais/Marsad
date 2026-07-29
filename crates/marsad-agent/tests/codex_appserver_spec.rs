//! Unit tests for the codex app-server codec (`codex_appserver`).
//!
//! Uses REAL bytes captured live from codex-cli 0.142.5 (2026-07-21) so a wire
//! drift fails here. Codex omits the `jsonrpc` field — the key reason we can't
//! reuse `acp::parse_acp_line` (which requires it).

use marsad_agent::acp::AcpMessage;
use marsad_agent::codex_appserver as cx;

#[test]
fn parses_response_without_jsonrpc_field() {
    // Real `initialize` result line (no "jsonrpc").
    let line = r#"{"id":1,"result":{"userAgent":"m/0.142.5","codexHome":"/tmp/cxhome","platformOs":"linux"}}"#;
    match cx::parse_line(line).expect("codex response parses") {
        AcpMessage::Response { id, result } => {
            assert_eq!(id, serde_json::json!(1));
            assert!(result.is_ok());
        }
        other => panic!("expected Response, got {other:?}"),
    }
}

#[test]
fn parses_notification_without_jsonrpc_field() {
    // Real `thread/started` notification line.
    let line = r#"{"method":"thread/started","params":{"thread":{"id":"019fab44-dff8-77a3-a70e-ddcbee5cd044","sessionId":"019fab44-dff8-77a3-a70e-ddcbee5cd044"}}}"#;
    match cx::parse_line(line).expect("codex notification parses") {
        AcpMessage::Notification { method, .. } => assert_eq!(method, "thread/started"),
        other => panic!("expected Notification, got {other:?}"),
    }
}

#[test]
fn parses_server_approval_request() {
    let line = r#"{"id":7,"method":"item/commandExecution/requestApproval","params":{"itemId":"i1","threadId":"t","turnId":"u","command":"rm -rf x","cwd":"/tmp"}}"#;
    match cx::parse_line(line).expect("approval request parses") {
        AcpMessage::Request { id, method, .. } => {
            assert_eq!(id, serde_json::json!(7));
            assert!(cx::is_command_approval(&method), "recognised as an approval gate");
        }
        other => panic!("expected Request, got {other:?}"),
    }
}

#[test]
fn extract_thread_id_from_real_result() {
    // Real thread/start result payload (the `result` object).
    let result: serde_json::Value = serde_json::from_str(
        r#"{"thread":{"id":"019fab44-dff8-77a3-a70e-ddcbee5cd044","sessionId":"019fab44-dff8-77a3-a70e-ddcbee5cd044","forkedFromId":null,"parentThreadId":null}}"#,
    )
    .unwrap();
    assert_eq!(
        cx::extract_thread_id(&result).as_deref(),
        Some("019fab44-dff8-77a3-a70e-ddcbee5cd044")
    );
}

#[test]
fn extract_turn_id_and_status_from_real_turn_completed() {
    // Real turn/completed params (a failed turn from the live probe).
    let params: serde_json::Value = serde_json::from_str(
        r#"{"threadId":"019fab20-a8a7-7ed3-bb20-473790000177","turn":{"id":"019fab20-b87a-74a1-a8b9-53ed4c097388","items":[],"status":"failed","error":{"message":"x"}}}"#,
    )
    .unwrap();
    assert_eq!(
        cx::extract_turn_id(&params).as_deref(),
        Some("019fab20-b87a-74a1-a8b9-53ed4c097388")
    );
    assert_eq!(cx::extract_turn_status(&params).as_deref(), Some("failed"));
}

#[test]
fn param_constructors_have_the_verified_shapes() {
    let init = cx::initialize_params("marsad", "0.1");
    assert_eq!(init["clientInfo"]["name"], "marsad");

    // turn/start: input is a single text item (verified accepted live).
    let t = cx::turn_start_params("thread-1", "hello");
    assert_eq!(t["threadId"], "thread-1");
    assert_eq!(t["input"][0]["type"], "text");
    assert_eq!(t["input"][0]["text"], "hello");

    let ti = cx::turn_interrupt_params("thread-1", "turn-9");
    assert_eq!(ti["threadId"], "thread-1");
    assert_eq!(ti["turnId"], "turn-9");

    // thread/start omits empty optionals but includes set ones.
    let ts = cx::thread_start_params("/proj", Some("gpt-5-codex"), Some("on-request"), Some("read-only"));
    assert_eq!(ts["cwd"], "/proj");
    assert_eq!(ts["model"], "gpt-5-codex");
    assert_eq!(ts["approvalPolicy"], "on-request");
    assert_eq!(ts["sandbox"], "read-only");
    let ts_min = cx::thread_start_params("/proj", None, None, None);
    assert!(ts_min.get("model").is_none() && ts_min.get("sandbox").is_none());

    // live settings update
    let su = cx::settings_update_params("thread-1", "model", "gpt-5-codex");
    assert_eq!(su["threadId"], "thread-1");
    assert_eq!(su["model"], "gpt-5-codex");
}

#[test]
fn decision_builders_match_the_schema_enums() {
    assert_eq!(cx::approval_decision(true, false)["decision"], "accept");
    assert_eq!(cx::approval_decision(true, true)["decision"], "acceptForSession");
    assert_eq!(cx::approval_decision(false, false)["decision"], "decline");
    assert_eq!(cx::legacy_review_decision(true)["decision"], "approved");
    assert_eq!(cx::legacy_review_decision(false)["decision"], "denied");
}
