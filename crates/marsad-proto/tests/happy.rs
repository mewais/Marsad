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
    let line = r#"{"type":"control","subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls /"},"request_id":"req-42"}"#;
    let msg = parse_stream_line(line).expect("should parse control/can_use_tool");
    match msg {
        StreamMsg::ControlRequest(cr) => {
            assert_eq!(cr.tool_name, "Bash");
            assert_eq!(cr.request_id, "req-42");
            assert!(cr.input.is_object());
        }
        other => panic!("expected ControlRequest, got {other:?}"),
    }
}

#[test]
fn parse_control_response_allow() {
    let line =
        r#"{"type":"control_response","request_id":"req-42","allow":true,"deny_reason":null}"#;
    let msg = parse_stream_line(line).expect("should parse control_response allow");
    match msg {
        StreamMsg::ControlResponse(cr) => {
            assert_eq!(cr.request_id, "req-42");
            assert!(cr.allow);
        }
        other => panic!("expected ControlResponse, got {other:?}"),
    }
}

#[test]
fn parse_control_response_deny_with_reason() {
    let line = r#"{"type":"control_response","request_id":"req-99","allow":false,"deny_reason":"not permitted"}"#;
    let msg = parse_stream_line(line).expect("should parse control_response deny");
    match msg {
        StreamMsg::ControlResponse(cr) => {
            assert!(!cr.allow);
            assert_eq!(cr.deny_reason.as_deref(), Some("not permitted"));
        }
        other => panic!("expected ControlResponse, got {other:?}"),
    }
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
    // Must be valid JSON and must round-trip to a user message
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
fn encode_control_response_allow_produces_valid_json() {
    let resp = ControlResponse {
        request_id: "req-1".to_string(),
        allow: true,
        deny_reason: None,
    };
    let encoded = encode_control_response(&resp);
    let parsed = parse_stream_line(&encoded).expect("encoded control response must parse back");
    match parsed {
        StreamMsg::ControlResponse(cr) => {
            assert_eq!(cr.request_id, "req-1");
            assert!(cr.allow);
            assert!(cr.deny_reason.is_none());
        }
        other => panic!("expected ControlResponse after encoding, got {other:?}"),
    }
}

#[test]
fn encode_control_response_deny_produces_valid_json() {
    let resp = ControlResponse {
        request_id: "req-2".to_string(),
        allow: false,
        deny_reason: Some("blocked".to_string()),
    };
    let encoded = encode_control_response(&resp);
    let parsed =
        parse_stream_line(&encoded).expect("encoded deny control response must parse back");
    match parsed {
        StreamMsg::ControlResponse(cr) => {
            assert!(!cr.allow);
            assert_eq!(cr.deny_reason.as_deref(), Some("blocked"));
        }
        other => panic!("expected ControlResponse, got {other:?}"),
    }
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
