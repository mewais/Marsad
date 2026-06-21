/// happy.rs — nominal parse + encode paths for every StreamMsg and IpcMsg variant.
///
/// These tests WILL fail to compile until the code-agent implements `marsad_proto`.
/// That is the expected tests-first signal.
use marsad_proto::{
    encode_control_response, encode_ipc, encode_user_input, parse_ipc, parse_stream_line,
    ContentBlock, ControlResponse, IpcMsg, ResultSubtype, StreamMsg, UserInput,
};

// ---------------------------------------------------------------------------
// StreamMsg happy paths
// ---------------------------------------------------------------------------

#[test]
fn parse_system_init() {
    let line = r#"{"type":"system","subtype":"init","cwd":"/work","session_id":"abc-123","model":"claude-opus-4.8","tools":["Read","Write"],"mcp_servers":[{"name":"hares","status":"connected"}],"permissionMode":"default"}"#;
    let msg = parse_stream_line(line).expect("should parse system/init");
    match msg {
        StreamMsg::SystemInit(init) => {
            assert_eq!(init.session_id, "abc-123");
            assert_eq!(init.model, "claude-opus-4.8");
            assert_eq!(init.cwd, "/work");
            assert_eq!(init.tools, vec!["Read", "Write"]);
            assert_eq!(init.permission_mode, "default");
        }
        other => panic!("expected SystemInit, got {other:?}"),
    }
}

#[test]
fn parse_assistant_text_content() {
    let line = r#"{"type":"assistant","message":{"id":"msg_001","model":"claude-opus-4.8","type":"message","role":"assistant","content":[{"type":"text","text":"Hello, world!"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"session_id":"abc-123","uuid":"uuid-001"}"#;
    let msg = parse_stream_line(line).expect("should parse assistant");
    match msg {
        StreamMsg::Assistant(a) => {
            assert_eq!(a.message.id, "msg_001");
            assert_eq!(a.message.content.len(), 1);
            match &a.message.content[0] {
                ContentBlock::Text { text } => assert_eq!(text, "Hello, world!"),
                other => panic!("expected Text block, got {other:?}"),
            }
            assert_eq!(a.message.usage.input_tokens, 10);
            assert_eq!(a.message.usage.output_tokens, 5);
        }
        other => panic!("expected Assistant, got {other:?}"),
    }
}

#[test]
fn parse_assistant_tool_use_content() {
    let line = r#"{"type":"assistant","message":{"id":"msg_002","model":"claude-opus-4.8","type":"message","role":"assistant","content":[{"type":"tool_use","id":"tu_001","name":"Read","input":{"file_path":"/foo.txt"}}],"usage":{"input_tokens":20,"output_tokens":8,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"session_id":"abc-123","uuid":"uuid-002"}"#;
    let msg = parse_stream_line(line).expect("should parse assistant with tool_use");
    match msg {
        StreamMsg::Assistant(a) => {
            assert_eq!(a.message.content.len(), 1);
            match &a.message.content[0] {
                ContentBlock::ToolUse { id, name, input } => {
                    assert_eq!(id, "tu_001");
                    assert_eq!(name, "Read");
                    assert!(input.is_object());
                }
                other => panic!("expected ToolUse block, got {other:?}"),
            }
        }
        other => panic!("expected Assistant, got {other:?}"),
    }
}

#[test]
fn parse_user_tool_result_content() {
    let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_001","content":"file contents here","is_error":false}]},"session_id":"abc-123"}"#;
    let msg = parse_stream_line(line).expect("should parse user/tool_result");
    match msg {
        StreamMsg::User(u) => {
            assert_eq!(u.message.content.len(), 1);
            match &u.message.content[0] {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content: _,
                    is_error,
                } => {
                    assert_eq!(tool_use_id, "tu_001");
                    assert!(!is_error);
                }
                other => panic!("expected ToolResult block, got {other:?}"),
            }
        }
        other => panic!("expected User, got {other:?}"),
    }
}

#[test]
fn parse_result_success() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"total_cost_usd":0.5,"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"permission_denials":[],"session_id":"abc-123"}"#;
    let msg = parse_stream_line(line).expect("should parse result/success");
    match msg {
        StreamMsg::Result(r) => {
            assert_eq!(r.subtype, ResultSubtype::Success);
            assert!(!r.is_error);
            assert_eq!(r.num_turns, 1);
            assert!((r.total_cost_usd - 0.5).abs() < 1e-9);
            assert!(r.permission_denials.is_empty());
        }
        other => panic!("expected Result, got {other:?}"),
    }
}

#[test]
fn parse_result_error_max_turns() {
    let line = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":10,"total_cost_usd":1.0,"usage":{"input_tokens":500,"output_tokens":200,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"permission_denials":[],"session_id":"abc-123"}"#;
    let msg = parse_stream_line(line).expect("should parse result/error_max_turns");
    match msg {
        StreamMsg::Result(r) => {
            assert_eq!(r.subtype, ResultSubtype::ErrorMaxTurns);
            assert!(r.is_error);
        }
        other => panic!("expected Result, got {other:?}"),
    }
}

#[test]
fn parse_result_error_during_execution() {
    let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":3,"total_cost_usd":0.2,"usage":{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"permission_denials":[],"session_id":"abc-123"}"#;
    let msg = parse_stream_line(line).expect("should parse result/error_during_execution");
    match msg {
        StreamMsg::Result(r) => {
            assert_eq!(r.subtype, ResultSubtype::ErrorDuringExecution);
        }
        other => panic!("expected Result, got {other:?}"),
    }
}

#[test]
fn parse_control_request() {
    // Real Agent-SDK control protocol: payload nested under `request`.
    let line = r#"{"type":"control_request","request_id":"req-42","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls /"}}}"#;
    let msg = parse_stream_line(line).expect("should parse control_request/can_use_tool");
    match msg {
        StreamMsg::ControlRequest(cr) => {
            assert_eq!(cr.tool_name, "Bash");
            assert_eq!(cr.request_id, "req-42");
            assert!(cr.input.is_object());
            assert_eq!(cr.input["command"].as_str(), Some("ls /"));
        }
        other => panic!("expected ControlRequest, got {other:?}"),
    }
}

#[test]
fn parse_control_request_missing_tool_name_and_input_defaults() {
    // can_use_tool with no tool_name / input → empty name + Null input (no panic).
    let line =
        r#"{"type":"control_request","request_id":"req-7","request":{"subtype":"can_use_tool"}}"#;
    let msg = parse_stream_line(line).expect("should parse");
    match msg {
        StreamMsg::ControlRequest(cr) => {
            assert_eq!(cr.tool_name, "");
            assert_eq!(cr.request_id, "req-7");
            assert!(cr.input.is_null());
        }
        other => panic!("expected ControlRequest, got {other:?}"),
    }
}

#[test]
fn parse_control_request_non_can_use_tool_subtype_is_unknown() {
    // A control_request with a different subtype is forward-compat Unknown.
    let line = r#"{"type":"control_request","request_id":"req-9","request":{"subtype":"mcp_message","payload":{}}}"#;
    let msg = parse_stream_line(line).expect("should parse");
    assert!(
        matches!(msg, StreamMsg::Unknown(_)),
        "non-can_use_tool control_request must be Unknown, got {msg:?}"
    );
}

#[test]
fn parse_control_request_missing_request_object_is_unknown() {
    // Missing `request` object → Unknown rather than an error.
    let line = r#"{"type":"control_request","request_id":"req-10"}"#;
    let msg = parse_stream_line(line).expect("should parse");
    assert!(
        matches!(msg, StreamMsg::Unknown(_)),
        "control_request without `request` must be Unknown, got {msg:?}"
    );
}

#[test]
fn parse_control_response_init_ack_is_noop_unknown() {
    // The CLI's `initialize` ack arrives as a `control_response`; the agent
    // does not act on it, so it parses as a forward-compat Unknown (no-op).
    let line = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_1_init","response":{"commands":[],"output_style":"default"}}}"#;
    let msg = parse_stream_line(line).expect("should parse control_response ack");
    assert!(
        matches!(msg, StreamMsg::Unknown(_)),
        "control_response init-ack must be Unknown (no-op), got {msg:?}"
    );
}

// ---------------------------------------------------------------------------
// Encode happy paths
// ---------------------------------------------------------------------------

#[test]
fn encode_user_input_produces_valid_json() {
    let input = UserInput {
        content: vec![ContentBlock::Text {
            text: "Hello from user".to_string(),
        }],
    };
    let encoded = encode_user_input(&input);
    // The raw JSON must contain "role":"user" so the claude binary accepts it.
    let raw: serde_json::Value =
        serde_json::from_str(&encoded).expect("encoded user input must be valid JSON");
    assert_eq!(
        raw["message"]["role"].as_str(),
        Some("user"),
        "message object must include \"role\":\"user\""
    );
    // Must also round-trip to a typed user message.
    let parsed = parse_stream_line(&encoded).expect("encoded user input must parse back");
    match parsed {
        StreamMsg::User(u) => {
            assert_eq!(u.message.content.len(), 1);
            match &u.message.content[0] {
                ContentBlock::Text { text } => assert_eq!(text, "Hello from user"),
                other => panic!("expected Text, got {other:?}"),
            }
        }
        other => panic!("expected User after encoding, got {other:?}"),
    }
}

#[test]
fn encode_control_response_allow_with_updated_input() {
    let resp = ControlResponse {
        request_id: "req-1".to_string(),
        allow: true,
        deny_reason: None,
        updated_input: Some(serde_json::json!({"command": "ls /"})),
    };
    let encoded = encode_control_response(&resp);
    let raw: serde_json::Value =
        serde_json::from_str(&encoded).expect("encoded allow must be valid JSON");
    assert_eq!(raw["type"], "control_response");
    assert_eq!(raw["response"]["subtype"], "success");
    assert_eq!(raw["response"]["request_id"], "req-1");
    assert_eq!(raw["response"]["response"]["behavior"], "allow");
    // updatedInput must echo the original tool input so the CLI runs the right command.
    assert_eq!(
        raw["response"]["response"]["updatedInput"]["command"].as_str(),
        Some("ls /")
    );
}

#[test]
fn encode_control_response_allow_without_updated_input_defaults_to_empty_object() {
    let resp = ControlResponse {
        request_id: "req-1b".to_string(),
        allow: true,
        deny_reason: None,
        updated_input: None,
    };
    let encoded = encode_control_response(&resp);
    let raw: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
    assert_eq!(raw["response"]["response"]["behavior"], "allow");
    assert!(
        raw["response"]["response"]["updatedInput"].is_object(),
        "updatedInput defaults to an (empty) object"
    );
}

#[test]
fn encode_control_response_deny_with_reason() {
    let resp = ControlResponse {
        request_id: "req-2".to_string(),
        allow: false,
        deny_reason: Some("blocked".to_string()),
        updated_input: None,
    };
    let encoded = encode_control_response(&resp);
    let raw: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
    assert_eq!(raw["response"]["response"]["behavior"], "deny");
    assert_eq!(raw["response"]["response"]["message"], "blocked");
    assert_eq!(raw["response"]["request_id"], "req-2");
}

#[test]
fn encode_control_response_deny_without_reason_defaults_to_empty_message() {
    let resp = ControlResponse {
        request_id: "req-2b".to_string(),
        allow: false,
        deny_reason: None,
        updated_input: None,
    };
    let encoded = encode_control_response(&resp);
    let raw: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
    assert_eq!(raw["response"]["response"]["behavior"], "deny");
    assert_eq!(raw["response"]["response"]["message"], "");
}

#[test]
fn encode_initialize_handshake_shape() {
    let encoded = marsad_proto::encode_control_request_initialize("req_1_init");
    let raw: serde_json::Value =
        serde_json::from_str(&encoded).expect("encoded initialize must be valid JSON");
    assert_eq!(raw["type"], "control_request");
    assert_eq!(raw["request_id"], "req_1_init");
    assert_eq!(raw["request"]["subtype"], "initialize");
    assert!(
        raw["request"].get("hooks").is_some(),
        "initialize request carries a (null) hooks field"
    );
}

// ---------------------------------------------------------------------------
// IpcMsg happy paths — parse + encode round-trips
// ---------------------------------------------------------------------------

#[test]
fn parse_ipc_launch_session() {
    let line = r#"{"type":"LaunchSession","cwd":"/work/proj","model":"claude-opus-4.8","permission_mode":"default"}"#;
    let msg = parse_ipc(line).expect("should parse LaunchSession");
    match msg {
        IpcMsg::LaunchSession { .. } => {}
        other => panic!("expected LaunchSession, got {other:?}"),
    }
    // round-trip
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("round-trip LaunchSession");
    match reparsed {
        IpcMsg::LaunchSession { .. } => {}
        other => panic!("expected LaunchSession after round-trip, got {other:?}"),
    }
}

#[test]
fn parse_ipc_resume_session() {
    let line = r#"{"type":"ResumeSession","session_id":"abc-123"}"#;
    let msg = parse_ipc(line).expect("should parse ResumeSession");
    match msg {
        IpcMsg::ResumeSession { session_id } => assert_eq!(session_id, "abc-123"),
        other => panic!("expected ResumeSession, got {other:?}"),
    }
}

#[test]
fn parse_ipc_attach_session() {
    let line = r#"{"type":"AttachSession","session_id":"abc-456"}"#;
    let msg = parse_ipc(line).expect("should parse AttachSession");
    match msg {
        IpcMsg::AttachSession { session_id } => assert_eq!(session_id, "abc-456"),
        other => panic!("expected AttachSession, got {other:?}"),
    }
}

#[test]
fn parse_ipc_stop_session() {
    let line = r#"{"type":"StopSession","session_id":"abc-789"}"#;
    let msg = parse_ipc(line).expect("should parse StopSession");
    match msg {
        IpcMsg::StopSession { session_id } => assert_eq!(session_id, "abc-789"),
        other => panic!("expected StopSession, got {other:?}"),
    }
}

#[test]
fn parse_ipc_send_prompt() {
    let line = r#"{"type":"SendPrompt","session_id":"abc-123","text":"What is 2+2?"}"#;
    let msg = parse_ipc(line).expect("should parse SendPrompt");
    match msg {
        IpcMsg::SendPrompt { session_id, text } => {
            assert_eq!(session_id, "abc-123");
            assert_eq!(text, "What is 2+2?");
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
}

#[test]
fn parse_ipc_answer_gate() {
    let line = r#"{"type":"AnswerGate","request_id":"req-42","allow":true,"deny_reason":null}"#;
    let msg = parse_ipc(line).expect("should parse AnswerGate");
    match msg {
        IpcMsg::AnswerGate {
            request_id, allow, ..
        } => {
            assert_eq!(request_id, "req-42");
            assert!(allow);
        }
        other => panic!("expected AnswerGate, got {other:?}"),
    }
}

#[test]
fn parse_ipc_subscribe() {
    let line = r#"{"type":"Subscribe","session_id":"abc-123"}"#;
    let msg = parse_ipc(line).expect("should parse Subscribe");
    match msg {
        IpcMsg::Subscribe { session_id } => assert_eq!(session_id, "abc-123"),
        other => panic!("expected Subscribe, got {other:?}"),
    }
}

#[test]
fn parse_ipc_vm_update() {
    let line = r#"{"type":"VmUpdate","session_id":"abc-123","update":{"status":"working"}}"#;
    let msg = parse_ipc(line).expect("should parse VmUpdate");
    match msg {
        IpcMsg::VmUpdate { session_id, .. } => assert_eq!(session_id, "abc-123"),
        other => panic!("expected VmUpdate, got {other:?}"),
    }
}

#[test]
fn parse_ipc_agent_event() {
    let line = r#"{"type":"AgentEvent","session_id":"abc-123","event":{"kind":"stream","data":"some output"}}"#;
    let msg = parse_ipc(line).expect("should parse AgentEvent");
    match msg {
        IpcMsg::AgentEvent { session_id, .. } => assert_eq!(session_id, "abc-123"),
        other => panic!("expected AgentEvent, got {other:?}"),
    }
}

#[test]
fn encode_ipc_all_variants_produce_valid_json() {
    let variants: Vec<IpcMsg> = vec![
        IpcMsg::LaunchSession {
            cwd: "/work".to_string(),
            model: "claude-opus-4.8".to_string(),
            permission_mode: "default".to_string(),
        },
        IpcMsg::ResumeSession {
            session_id: "sid-1".to_string(),
        },
        IpcMsg::AttachSession {
            session_id: "sid-2".to_string(),
        },
        IpcMsg::StopSession {
            session_id: "sid-3".to_string(),
        },
        IpcMsg::SendPrompt {
            session_id: "sid-4".to_string(),
            text: "prompt text".to_string(),
        },
        IpcMsg::AnswerGate {
            request_id: "req-1".to_string(),
            allow: true,
            deny_reason: None,
        },
        IpcMsg::Subscribe {
            session_id: "sid-5".to_string(),
        },
    ];
    for variant in &variants {
        let encoded = encode_ipc(variant);
        let v: serde_json::Value =
            serde_json::from_str(&encoded).expect("encode_ipc must produce valid JSON");
        assert!(v.is_object(), "encoded IPC message must be a JSON object");
        // round-trip
        let reparsed = parse_ipc(&encoded)
            .unwrap_or_else(|e| panic!("IPC round-trip failed for {variant:?}: {e:?}"));
        // just assert they re-encode to the same string (stable serialization)
        assert_eq!(encode_ipc(&reparsed), encoded);
    }
}
