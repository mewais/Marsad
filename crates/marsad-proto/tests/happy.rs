/// happy.rs — nominal parse + encode paths for every StreamMsg and IpcMsg variant.
///
/// These tests WILL fail to compile until the code-agent implements `marsad_proto`.
/// That is the expected tests-first signal.
use marsad_proto::{
    encode_control_response, encode_control_response_elicitation, encode_control_response_question,
    encode_ipc, encode_user_input, parse_ipc, parse_stream_line, ContentBlock, ControlResponse,
    IpcMsg, ResultSubtype, StreamMsg, UserInput,
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
fn parse_control_request_elicitation_carries_message_and_server() {
    // An MCP elicitation (e.g. a sandboxed-bash server asking to approve a push)
    // must parse to a ControlRequest with the message captured and the server as
    // tool_name — NOT fall through to Unknown (which left sessions stuck at Working).
    let line = r#"{"type":"control_request","request_id":"eli-1","request":{"subtype":"elicitation","mcp_server_name":"hares","message":"Allow `git push`?","mode":"form","requested_schema":{"type":"object","properties":{}}}}"#;
    let msg = parse_stream_line(line).expect("should parse elicitation control_request");
    match msg {
        StreamMsg::ControlRequest(cr) => {
            assert_eq!(cr.request_id, "eli-1");
            assert_eq!(cr.tool_name, "hares");
            assert_eq!(cr.elicitation_message.as_deref(), Some("Allow `git push`?"));
        }
        other => panic!("expected ControlRequest, got {other:?}"),
    }
}

#[test]
fn parse_control_request_elicitation_missing_fields_default_empty() {
    let line =
        r#"{"type":"control_request","request_id":"eli-2","request":{"subtype":"elicitation"}}"#;
    let msg = parse_stream_line(line).expect("should parse");
    match msg {
        StreamMsg::ControlRequest(cr) => {
            assert_eq!(cr.tool_name, "");
            assert_eq!(cr.elicitation_message.as_deref(), Some(""));
        }
        other => panic!("expected ControlRequest, got {other:?}"),
    }
}

#[test]
fn parse_control_request_can_use_tool_has_no_elicitation_message() {
    let line = r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#;
    match parse_stream_line(line).expect("parse") {
        StreamMsg::ControlRequest(cr) => assert!(cr.elicitation_message.is_none()),
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
        updated_permissions: None,
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
        updated_permissions: None,
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
        updated_permissions: None,
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
        updated_permissions: None,
    };
    let encoded = encode_control_response(&resp);
    let raw: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
    assert_eq!(raw["response"]["response"]["behavior"], "deny");
    assert_eq!(raw["response"]["response"]["message"], "");
}

#[test]
fn encode_control_response_elicitation_accept() {
    let encoded = encode_control_response_elicitation("eli-1", true, None);
    let raw: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
    assert_eq!(raw["type"], "control_response");
    assert_eq!(raw["response"]["subtype"], "success");
    assert_eq!(raw["response"]["request_id"], "eli-1");
    assert_eq!(raw["response"]["response"]["action"], "accept");
    assert!(raw["response"]["response"]["content"].is_object());
}

#[test]
fn encode_control_response_elicitation_decline() {
    let encoded = encode_control_response_elicitation("eli-2", false, None);
    let raw: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
    assert_eq!(raw["response"]["request_id"], "eli-2");
    assert_eq!(raw["response"]["response"]["action"], "decline");
    assert!(raw["response"]["response"].get("content").is_none());
}

#[test]
fn encode_control_response_elicitation_accept_with_content() {
    // A scope elicitation accept carries the chosen content verbatim.
    let content = serde_json::json!({ "grant": "session" });
    let encoded = encode_control_response_elicitation("eli-3", true, Some(content));
    let raw: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
    assert_eq!(raw["response"]["response"]["action"], "accept");
    assert_eq!(raw["response"]["response"]["content"]["grant"], "session");
}

#[test]
fn parse_elicitation_choices_reads_hares_grant_schema() {
    use marsad_proto::parse_elicitation_choices;
    // The exact schema hares' request_path_access elicitation sends.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "grant": {
                "type": "string",
                "title": "Grant scope",
                "enum": ["once", "session", "deny"],
                "enumNames": [
                    "Allow once (this command only)",
                    "Allow for rest of session",
                    "Deny",
                ],
            },
        },
        "required": ["grant"],
    });
    let choices = parse_elicitation_choices(Some(&schema));
    assert_eq!(choices.len(), 3);
    assert!(choices.iter().all(|c| c.key == "grant"));
    assert_eq!(choices[0].value, "once");
    assert_eq!(choices[0].label, "Allow once (this command only)");
    assert_eq!(choices[1].value, "session");
    assert_eq!(choices[2].value, "deny");
    assert_eq!(choices[2].label, "Deny");
}

#[test]
fn parse_elicitation_choices_empty_schema_yields_none() {
    use marsad_proto::parse_elicitation_choices;
    // The high-memory / exec elicitations use an empty-property schema → no
    // choices → the UI falls back to binary Approve/Deny.
    let schema = serde_json::json!({ "type": "object", "properties": {} });
    assert!(parse_elicitation_choices(Some(&schema)).is_empty());
    assert!(parse_elicitation_choices(None).is_empty());
}

#[test]
fn launch_mode_default_serde_and_spec_default() {
    use marsad_proto::{LaunchMode, LaunchSpec};
    // Default is Fresh.
    assert_eq!(LaunchMode::default(), LaunchMode::Fresh);
    // Each variant round-trips, and Clone/Debug are exercised.
    for m in [
        LaunchMode::Fresh,
        LaunchMode::Continue,
        LaunchMode::Resume("sess-1".to_string()),
    ] {
        let cloned = m.clone();
        let _ = format!("{cloned:?}");
        let s = serde_json::to_string(&m).expect("serialize LaunchMode");
        let back: LaunchMode = serde_json::from_str(&s).expect("deserialize LaunchMode");
        assert_eq!(m, back);
    }
    // A LaunchSpec serialized WITHOUT a `mode` field deserializes to Fresh
    // (the #[serde(default)] keeps older payloads working).
    let spec: LaunchSpec = serde_json::from_str(
        r#"{"project":"/p","host_id":"local","model":"m","posture":"Ask","first_prompt":null}"#,
    )
    .expect("LaunchSpec without mode must parse");
    assert_eq!(spec.mode, LaunchMode::Fresh);
    // And with an explicit resume mode.
    let resumed: LaunchSpec = serde_json::from_str(
        r#"{"project":"/p","host_id":"local","model":"m","posture":"Ask","first_prompt":null,"mode":{"Resume":"abc"}}"#,
    )
    .expect("LaunchSpec with resume mode must parse");
    assert_eq!(resumed.mode, LaunchMode::Resume("abc".to_string()));
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

#[test]
fn encode_interrupt_request_shape() {
    let encoded = marsad_proto::encode_control_request_interrupt("req_int");
    let raw: serde_json::Value =
        serde_json::from_str(&encoded).expect("encoded interrupt must be valid JSON");
    assert_eq!(raw["type"], "control_request");
    assert_eq!(raw["request_id"], "req_int");
    assert_eq!(raw["request"]["subtype"], "interrupt");
}

// ---------------------------------------------------------------------------
// IpcMsg happy paths — parse + encode round-trips
// ---------------------------------------------------------------------------

#[test]
fn parse_ipc_launch_session() {
    // SPEC CHANGE (docs/specs/typed-ipc.md §7, "Tests that MUST change"): the
    // original golden here (`cwd`/`model`/`permission_mode` only) never
    // matched the real wire — the agent has always required `project`+
    // `posture`. Replaced with the §2 minimal + maximal LaunchSession
    // goldens (full per-field pinned coverage now lives in
    // crates/marsad-proto/tests/ipc_typed.rs).
    let minimal = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5","posture":"Ask"}"#;
    let msg = parse_ipc(minimal).expect("should parse minimal LaunchSession");
    match msg {
        IpcMsg::LaunchSession {
            ref project,
            ref model,
            ..
        } => {
            assert_eq!(project, "/home/user/proj");
            assert_eq!(model, "claude-opus-4-5");
        }
        other => panic!("expected LaunchSession, got {other:?}"),
    }
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("round-trip minimal LaunchSession");
    match reparsed {
        IpcMsg::LaunchSession { .. } => {}
        other => panic!("expected LaunchSession after round-trip, got {other:?}"),
    }

    let maximal = r#"{"type":"LaunchSession","session_id":"s-1","project":"/home/u/proj","cwd":"/home/u/proj","host_id":"hpc","host_target":"user@hpc.example.com","claude_bin":"genie","setup":"module load cuda","env":[["FOO","bar"]],"model":"claude-opus-4-8","posture":"Ask","permission_mode":"default","first_prompt":null,"resume_session_id":"uuid-xyz","name":"My run","disallowed_tools":["Bash"]}"#;
    let msg = parse_ipc(maximal).expect("should parse maximal LaunchSession");
    match msg {
        IpcMsg::LaunchSession {
            ref project,
            ref model,
            ref posture,
            ..
        } => {
            assert_eq!(project, "/home/u/proj");
            assert_eq!(model, "claude-opus-4-8");
            assert_eq!(*posture, marsad_proto::Posture::Ask);
        }
        other => panic!("expected LaunchSession, got {other:?}"),
    }
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("round-trip maximal LaunchSession");
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
        // MECHANICAL FIX (not a semantic edit): `..` added because
        // `ResumeSession` gained the additive `first_prompt` field
        // (typed-ipc.md §2); Rust's exhaustive struct-pattern rule (E0027)
        // requires it. Assertion behavior is unchanged.
        IpcMsg::ResumeSession { session_id, .. } => assert_eq!(session_id, "abc-123"),
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
    let line = r#"{"type":"AnswerGate","session_id":"sess-9","request_id":"req-42","allow":true,"deny_reason":null}"#;
    let msg = parse_ipc(line).expect("should parse AnswerGate");
    match msg {
        IpcMsg::AnswerGate {
            session_id,
            request_id,
            allow,
            ..
        } => {
            assert_eq!(session_id, "sess-9");
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
    // SPEC CHANGE (docs/specs/typed-ipc.md §7, "Tests that MUST change"):
    // `LaunchSession`'s constructor gains the full §2 field set (this is now
    // the retyped variant, not the old 3-field stub), and the loop is
    // extended to cover every one of the 29 `IpcMsg` tags from §2 (10
    // pre-M2 + 19 new). Full per-variant golden coverage lives in
    // crates/marsad-proto/tests/ipc_typed.rs; this loop keeps its original
    // "every variant round-trips + re-encodes stably" smoke-test role.
    let variants: Vec<IpcMsg> = vec![
        IpcMsg::LaunchSession {
            session_id: Some("s-1".to_string()),
            project: "/home/u/proj".to_string(),
            cwd: "/home/u/proj".to_string(),
            host_id: Some("hpc".to_string()),
            host_target: Some("user@hpc.example.com".to_string()),
            claude_bin: Some("genie".to_string()),
            setup: Some("module load cuda".to_string()),
            env: vec![("FOO".to_string(), "bar".to_string())],
            model: "claude-opus-4-8".to_string(),
            posture: marsad_proto::Posture::Ask,
            permission_mode: "default".to_string(),
            first_prompt: None,
            continue_: None,
            resume_session_id: Some("uuid-xyz".to_string()),
            name: Some("My run".to_string()),
            disallowed_tools: vec!["Bash".to_string()],
            wasl: false,
            driver: "claude".to_string(),
        },
        IpcMsg::ResumeSession {
            session_id: "sid-1".to_string(),
            first_prompt: None,
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
        IpcMsg::Interrupt {
            session_id: "sid-4b".to_string(),
        },
        IpcMsg::AnswerGate {
            session_id: "sid-5".to_string(),
            request_id: "req-1".to_string(),
            allow: true,
            deny_reason: None,
            elicitation_content: None,
            permission_updates: None,
        },
        IpcMsg::AnswerQuestion {
            session_id: "sid-6".to_string(),
            request_id: "req-q1".to_string(),
            answers: serde_json::json!({"Which approach?": "Option A"}),
        },
        IpcMsg::Subscribe {
            session_id: "sid-5".to_string(),
        },
        IpcMsg::Unsubscribe {
            subscriber_id: "sub-1".to_string(),
        },
        IpcMsg::SlashCmd {
            session_id: "sid-7".to_string(),
            cmd: "compact".to_string(),
        },
        IpcMsg::SetWorkspace {
            workspace_id: "ws-1".to_string(),
        },
        IpcMsg::VmUpdate {
            session_id: "sid-8".to_string(),
            update: serde_json::json!({"status":"working"}),
        },
        IpcMsg::AgentEvent {
            session_id: "sid-9".to_string(),
            event: serde_json::json!({"type":"result"}),
        },
        IpcMsg::TranscriptBackfill {
            session_id: "sid-10".to_string(),
            jsonl: "{\"type\":\"user\"}".to_string(),
        },
        IpcMsg::ChangesUpdate {
            session_id: "sid-11".to_string(),
            changes: serde_json::json!([{"path":"a.rs","status":"M"}]),
        },
        IpcMsg::UsageUpdate {
            session_id: "sid-12".to_string(),
            usage: serde_json::json!({"m":{"input":1}}),
        },
        IpcMsg::SessionLaunched {
            session_id: "sid-13".to_string(),
        },
        IpcMsg::SessionResumed {
            session_id: "sid-14".to_string(),
        },
        IpcMsg::SessionAttached {
            session_id: "sid-15".to_string(),
        },
        IpcMsg::SessionStopped {
            session_id: "sid-16".to_string(),
        },
        IpcMsg::PromptSent {
            session_id: "sid-17".to_string(),
        },
        IpcMsg::Interrupted {
            session_id: "sid-18".to_string(),
        },
        IpcMsg::GateAnswered {
            session_id: "sid-19".to_string(),
        },
        IpcMsg::QuestionAnswered {
            session_id: "sid-20".to_string(),
        },
        IpcMsg::Subscribed {
            subscriber_id: "sub-2".to_string(),
        },
        IpcMsg::Error {
            message: "boom".to_string(),
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

// ---------------------------------------------------------------------------
// encode_control_response_question — new HITL question answer encoder
// ---------------------------------------------------------------------------

#[test]
fn encode_control_response_question_with_answers() {
    let original_input = serde_json::json!({
        "questions": [{"question": "Which approach?", "header": "Strategy", "multiSelect": false, "options": []}]
    });
    let answers = serde_json::json!({"Which approach?": "Option A"});
    let encoded = encode_control_response_question("req-q1", &original_input, &answers);
    let val: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
    assert_eq!(val["type"].as_str(), Some("control_response"));
    let response = &val["response"];
    assert_eq!(response["subtype"].as_str(), Some("success"));
    assert_eq!(response["request_id"].as_str(), Some("req-q1"));
    let inner = &response["response"];
    assert_eq!(inner["behavior"].as_str(), Some("allow"));
    let updated = &inner["updatedInput"];
    assert!(updated["questions"].is_array());
    assert_eq!(
        updated["answers"]["Which approach?"].as_str(),
        Some("Option A")
    );
}

#[test]
fn encode_control_response_question_no_questions_field_uses_empty_array() {
    // When the original input has no "questions" field, it should default to [].
    let original_input = serde_json::json!({"plan": "some plan"});
    let answers = serde_json::json!({});
    let encoded = encode_control_response_question("req-q2", &original_input, &answers);
    let val: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
    let inner = &val["response"]["response"];
    assert_eq!(inner["updatedInput"]["questions"], serde_json::json!([]));
}

// ---------------------------------------------------------------------------
// parse_ipc / encode_ipc — AnswerQuestion round-trip
// ---------------------------------------------------------------------------

#[test]
fn parse_ipc_answer_question() {
    let line = r#"{"type":"AnswerQuestion","session_id":"s1","request_id":"req-q1","answers":{"Which approach?":"Option A"}}"#;
    let msg = parse_ipc(line).expect("should parse AnswerQuestion");
    match msg {
        IpcMsg::AnswerQuestion {
            session_id,
            request_id,
            answers,
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(request_id, "req-q1");
            assert_eq!(answers["Which approach?"].as_str(), Some("Option A"));
        }
        other => panic!("expected AnswerQuestion, got {other:?}"),
    }
}

#[test]
fn encode_decode_answer_question_roundtrip() {
    let msg = IpcMsg::AnswerQuestion {
        session_id: "s1".to_string(),
        request_id: "req-q1".to_string(),
        answers: serde_json::json!({"q": "a"}),
    };
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("roundtrip should succeed");
    assert_eq!(encode_ipc(&reparsed), encoded);
}

// ---------------------------------------------------------------------------
// ModelUsage + ResultMsg.model_usage
// ---------------------------------------------------------------------------

/// A `result` event with a `modelUsage` map parses the per-model breakdown.
#[test]
fn parse_result_with_model_usage_populates_map() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"total_cost_usd":0.268,"usage":{"input_tokens":11,"output_tokens":155,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"permission_denials":[],"modelUsage":{"claude-sonnet-4.6[1m]":{"inputTokens":11,"outputTokens":155,"costUSD":0.268,"cacheReadInputTokens":0,"cacheCreationInputTokens":0}}}"#;
    let msg = parse_stream_line(line).expect("should parse result with modelUsage");
    match msg {
        StreamMsg::Result(r) => {
            assert!(!r.model_usage.is_empty(), "model_usage must be populated");
            let entry = r
                .model_usage
                .get("claude-sonnet-4.6[1m]")
                .expect("must have the model key");
            assert_eq!(entry.input_tokens, 11);
            assert_eq!(entry.output_tokens, 155);
            assert!((entry.cost_usd - 0.268).abs() < 1e-9);
        }
        other => panic!("expected Result, got {other:?}"),
    }
}

/// A `result` event WITHOUT `modelUsage` defaults to an empty map.
#[test]
fn parse_result_without_model_usage_defaults_empty() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"total_cost_usd":0.5,"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"permission_denials":[]}"#;
    let msg = parse_stream_line(line).expect("should parse result without modelUsage");
    match msg {
        StreamMsg::Result(r) => {
            assert!(
                r.model_usage.is_empty(),
                "absent modelUsage must default to empty map"
            );
        }
        other => panic!("expected Result, got {other:?}"),
    }
}

/// `ModelUsage` derives Default, Debug, Clone, PartialEq.
#[test]
fn model_usage_derives() {
    use marsad_proto::ModelUsage;
    let m = ModelUsage {
        input_tokens: 10,
        output_tokens: 20,
        cost_usd: 0.5,
        // Mechanical compile-fix: `estimated` is U4 cost-truthfulness turf
        // (concurrent worker); this literal just needs a value to compile.
        estimated: false,
    };
    let m2 = m.clone();
    assert_eq!(m, m2);
    let _ = format!("{m:?}");
    let def = ModelUsage::default();
    assert_eq!(def.input_tokens, 0);
    assert_eq!(def.output_tokens, 0);
    assert!((def.cost_usd).abs() < 1e-15);
}

// ---------------------------------------------------------------------------
// McpServerInfo tests
// ---------------------------------------------------------------------------

/// `McpServerInfo` derives Default, Debug, Clone, PartialEq, Serialize, Deserialize.
#[test]
fn mcp_server_info_derives() {
    use marsad_proto::McpServerInfo;
    let s = McpServerInfo {
        name: "hares".to_string(),
        status: "connected".to_string(),
    };
    let s2 = s.clone();
    assert_eq!(s, s2);
    let _ = format!("{s:?}");
    let def = McpServerInfo::default();
    assert_eq!(def.name, "");
    assert_eq!(def.status, "");
}

/// `McpServerInfo` round-trips through serde.
#[test]
fn mcp_server_info_serde_roundtrip() {
    use marsad_proto::McpServerInfo;
    let s = McpServerInfo {
        name: "agent-hub".to_string(),
        status: "failed".to_string(),
    };
    let json = serde_json::to_string(&s).unwrap();
    let back: McpServerInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(s, back);
}

/// `McpServerInfo` status defaults to empty string when the field is absent.
#[test]
fn mcp_server_info_status_defaults() {
    use marsad_proto::McpServerInfo;
    let s: McpServerInfo = serde_json::from_str(r#"{"name":"foo"}"#).unwrap();
    assert_eq!(s.name, "foo");
    assert_eq!(s.status, "");
}

// ---------------------------------------------------------------------------
// SlashCommandInfo tests
// ---------------------------------------------------------------------------

/// `SlashCommandInfo` derives Default, Debug, Clone, PartialEq, Serialize, Deserialize.
#[test]
fn slash_command_info_derives() {
    use marsad_proto::SlashCommandInfo;
    let s = SlashCommandInfo {
        name: "/clear".to_string(),
        description: "Clear conversation".to_string(),
        argument_hint: "".to_string(),
    };
    let s2 = s.clone();
    assert_eq!(s, s2);
    let _ = format!("{s:?}");
    let def = SlashCommandInfo::default();
    assert_eq!(def.name, "");
    assert_eq!(def.description, "");
    assert_eq!(def.argument_hint, "");
}

/// `SlashCommandInfo` round-trips through serde with camelCase argumentHint.
#[test]
fn slash_command_info_serde_roundtrip() {
    use marsad_proto::SlashCommandInfo;
    let s = SlashCommandInfo {
        name: "/model".to_string(),
        description: "Switch model".to_string(),
        argument_hint: "model-name".to_string(),
    };
    let json = serde_json::to_string(&s).unwrap();
    let back: SlashCommandInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(s, back);
    // The wire key is "argumentHint" (camelCase rename).
    assert!(
        json.contains("argumentHint"),
        "wire key must be argumentHint; json: {json}"
    );
}

/// `SlashCommandInfo` defaults: description and argumentHint absent → empty.
#[test]
fn slash_command_info_defaults() {
    use marsad_proto::SlashCommandInfo;
    let s: SlashCommandInfo = serde_json::from_str(r#"{"name":"/clear"}"#).unwrap();
    assert_eq!(s.name, "/clear");
    assert_eq!(s.description, "");
    assert_eq!(s.argument_hint, "");
}

// ---------------------------------------------------------------------------
// SystemInit with new fields
// ---------------------------------------------------------------------------

/// `system/init` with typed `mcp_servers` (name+status objects) parses correctly.
#[test]
fn parse_system_init_with_mcp_servers() {
    use marsad_proto::{McpServerInfo, StreamMsg};
    let line = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m","cwd":"/","tools":["Read"],"mcp_servers":[{"name":"hares","status":"failed"},{"name":"agent-hub","status":"connected"}],"permissionMode":"default"}"#;
    let msg = parse_stream_line(line).expect("parse");
    match msg {
        StreamMsg::SystemInit(init) => {
            assert_eq!(init.mcp_servers.len(), 2);
            assert_eq!(
                init.mcp_servers[0],
                McpServerInfo {
                    name: "hares".to_string(),
                    status: "failed".to_string()
                }
            );
            assert_eq!(
                init.mcp_servers[1],
                McpServerInfo {
                    name: "agent-hub".to_string(),
                    status: "connected".to_string()
                }
            );
        }
        other => panic!("expected SystemInit, got {other:?}"),
    }
}

/// `system/init` with `slash_commands` as an OBJECT array.
#[test]
fn parse_system_init_slash_commands_object_array() {
    use marsad_proto::StreamMsg;
    let line = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m","cwd":"/","tools":[],"mcp_servers":[],"permissionMode":"default","slash_commands":[{"name":"/clear","description":"Clear conversation history","argumentHint":""},{"name":"/model","description":"Switch the active model","argumentHint":"model-name"}]}"#;
    let msg = parse_stream_line(line).expect("parse");
    match msg {
        StreamMsg::SystemInit(init) => {
            assert_eq!(init.slash_commands.len(), 2);
            assert_eq!(init.slash_commands[0].name, "/clear");
            assert_eq!(
                init.slash_commands[0].description,
                "Clear conversation history"
            );
            assert_eq!(init.slash_commands[1].name, "/model");
            assert_eq!(init.slash_commands[1].argument_hint, "model-name");
        }
        other => panic!("expected SystemInit, got {other:?}"),
    }
}

/// `system/init` with `slash_commands` as a STRING array (bare command names).
#[test]
fn parse_system_init_slash_commands_string_array() {
    use marsad_proto::StreamMsg;
    // Bare strings: some with leading slash, some without.
    let line = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m","cwd":"/","tools":[],"mcp_servers":[],"permissionMode":"default","slash_commands":["clear","/compact","help"]}"#;
    let msg = parse_stream_line(line).expect("parse");
    match msg {
        StreamMsg::SystemInit(init) => {
            assert_eq!(init.slash_commands.len(), 3);
            // "clear" without slash → normalised to "/clear"
            assert_eq!(init.slash_commands[0].name, "/clear");
            assert_eq!(init.slash_commands[0].description, "");
            // "/compact" already has slash
            assert_eq!(init.slash_commands[1].name, "/compact");
            // "help" without slash → "/help"
            assert_eq!(init.slash_commands[2].name, "/help");
        }
        other => panic!("expected SystemInit, got {other:?}"),
    }
}

/// `system/init` with absent `slash_commands` field → empty vec (default).
#[test]
fn parse_system_init_no_slash_commands_defaults_empty() {
    use marsad_proto::StreamMsg;
    let line = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m","cwd":"/","tools":[],"mcp_servers":[],"permissionMode":"default"}"#;
    let msg = parse_stream_line(line).expect("parse");
    match msg {
        StreamMsg::SystemInit(init) => {
            assert!(init.slash_commands.is_empty());
        }
        other => panic!("expected SystemInit, got {other:?}"),
    }
}

/// `system/init` with tools populated — the `tools` vec is a `Vec<String>`.
#[test]
fn parse_system_init_tools_populated() {
    use marsad_proto::StreamMsg;
    let line = r#"{"type":"system","subtype":"init","session_id":"s1","model":"m","cwd":"/","tools":["Read","Bash","Edit"],"mcp_servers":[],"permissionMode":"default"}"#;
    let msg = parse_stream_line(line).expect("parse");
    match msg {
        StreamMsg::SystemInit(init) => {
            assert_eq!(init.tools, vec!["Read", "Bash", "Edit"]);
        }
        other => panic!("expected SystemInit, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// LaunchSpec.disallowed_tools
// ---------------------------------------------------------------------------

/// `LaunchSpec.disallowed_tools` defaults to empty when the field is absent.
#[test]
fn launch_spec_disallowed_tools_defaults_empty() {
    use marsad_proto::{LaunchMode, LaunchSpec, Posture};
    let spec = LaunchSpec {
        project: "/p".to_string(),
        host_id: "local".to_string(),
        model: "m".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: LaunchMode::Fresh,
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![],
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    };
    assert!(spec.disallowed_tools.is_empty());
}

/// `LaunchSpec.disallowed_tools` with values round-trips through serde.
#[test]
fn launch_spec_disallowed_tools_roundtrip() {
    use marsad_proto::{LaunchMode, LaunchSpec, Posture};
    let spec = LaunchSpec {
        project: "/p".to_string(),
        host_id: "local".to_string(),
        model: "m".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: LaunchMode::Fresh,
        host_target: None,
        claude_bin: None,
        setup: None,
        env: vec![],
        name: None,
        disallowed_tools: vec!["Bash".to_string(), "Edit".to_string()],
        wasl: false,
        driver: "claude".to_string(),
    };
    let json = serde_json::to_string(&spec).unwrap();
    let back: LaunchSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back.disallowed_tools, vec!["Bash", "Edit"]);
}

/// Old `LaunchSpec` JSON without `disallowed_tools` deserialises to empty vec.
#[test]
fn launch_spec_disallowed_tools_absent_field_defaults() {
    use marsad_proto::LaunchSpec;
    let json = r#"{"project":"/p","host_id":"local","model":"m","posture":"Ask","permissionMode":"default","mode":"Fresh"}"#;
    let spec: LaunchSpec = serde_json::from_str(json).unwrap();
    assert!(spec.disallowed_tools.is_empty());
}
