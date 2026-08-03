/// happy.rs — nominal parse + encode round-trips for the IPC surface and the
/// kept protocol data types.
///
/// Wave 0 removed the Claude stream-json types (`StreamMsg`, `parse_stream_line`,
/// the `encode_*` control-protocol encoders, and their supporting structs) from
/// `marsad-proto`. Those tests were deleted; the IPC + `LaunchSpec` tests below
/// are unchanged, and `IpcMsg::AgentEvent` now carries a typed
/// `wasit_core::AgentEvent` instead of a raw JSON `Value` — the round-trip tests
/// for it were updated accordingly and new ones added.
use marsad_proto::{encode_ipc, parse_ipc, AgentEvent, IpcMsg};

// ---------------------------------------------------------------------------
// IpcMsg happy paths — parse + encode round-trips
// ---------------------------------------------------------------------------

#[test]
fn parse_ipc_launch_session() {
    let minimal = r#"{"type":"LaunchSession","project":"/home/user/proj","host_id":"local","model":"claude-opus-4-5"}"#;
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

    let maximal = r#"{"type":"LaunchSession","session_id":"s-1","project":"/home/u/proj","cwd":"/home/u/proj","host_id":"hpc","host_target":"user@hpc.example.com","claude_bin":"genie","setup":"module load cuda","env":[["FOO","bar"]],"model":"claude-opus-4-8","session_mode":"default","first_prompt":null,"resume_session_id":"uuid-xyz","name":"My run","disallowed_tools":["Bash"]}"#;
    let msg = parse_ipc(maximal).expect("should parse maximal LaunchSession");
    match msg {
        IpcMsg::LaunchSession {
            ref project,
            ref model,
            ref session_mode,
            ..
        } => {
            assert_eq!(project, "/home/u/proj");
            assert_eq!(model, "claude-opus-4-8");
            assert_eq!(session_mode, "default");
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

// ---------------------------------------------------------------------------
// IpcMsg::AgentEvent — now carries a typed wasit_core::AgentEvent
// ---------------------------------------------------------------------------

#[test]
fn parse_ipc_agent_event_session_init() {
    // The wire event is an internally-tagged AgentEvent object (`type` = the
    // snake_case variant tag), wrapped in an AgentEvent IPC message.
    let line = r#"{"type":"AgentEvent","session_id":"abc-123","event":{"type":"session_init","model":"claude-opus-4.8","cwd":"/work"}}"#;
    let msg = parse_ipc(line).expect("should parse AgentEvent{SessionInit}");
    match msg {
        IpcMsg::AgentEvent { session_id, event } => {
            assert_eq!(session_id, "abc-123");
            match event {
                AgentEvent::SessionInit { model, cwd, .. } => {
                    assert_eq!(model.as_deref(), Some("claude-opus-4.8"));
                    assert_eq!(cwd.as_deref(), Some("/work"));
                }
                other => panic!("expected SessionInit, got {other:?}"),
            }
        }
        other => panic!("expected AgentEvent, got {other:?}"),
    }
}

/// `IpcMsg::AgentEvent { SessionInit }` round-trips through encode_ipc → parse_ipc.
#[test]
fn agent_event_session_init_roundtrips() {
    let msg = IpcMsg::AgentEvent {
        session_id: "s-1".to_string(),
        event: AgentEvent::SessionInit {
            native_session_id: Some("uuid-1".to_string()),
            model: Some("claude-opus-4.8".to_string()),
            cwd: Some("/work".to_string()),
            tools: vec!["Read".to_string(), "Bash".to_string()],
            mcp_servers: vec![],
            slash_commands: vec![],
            permission_mode: "default".to_string(),
            claude_code_version: String::new(),
            available_modes: vec![],
            current_mode: "default".to_string(),
        },
    };
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("AgentEvent{SessionInit} must reparse");
    assert_eq!(reparsed, msg);
    assert_eq!(encode_ipc(&reparsed), encoded, "re-encode must be stable");
}

/// `IpcMsg::AgentEvent { AssistantDelta }` round-trips.
#[test]
fn agent_event_assistant_delta_roundtrips() {
    let msg = IpcMsg::AgentEvent {
        session_id: "s-2".to_string(),
        event: AgentEvent::AssistantDelta {
            text: "Hello, world!".to_string(),
            message_id: Some("msg_001".to_string()),
        },
    };
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("AgentEvent{AssistantDelta} must reparse");
    assert_eq!(reparsed, msg);
    match &reparsed {
        IpcMsg::AgentEvent {
            event: AgentEvent::AssistantDelta { text, message_id },
            ..
        } => {
            assert_eq!(text, "Hello, world!");
            assert_eq!(message_id.as_deref(), Some("msg_001"));
        }
        other => panic!("expected AgentEvent{{AssistantDelta}}, got {other:?}"),
    }
    assert_eq!(encode_ipc(&reparsed), encoded, "re-encode must be stable");
}

/// `IpcMsg::AgentEvent { TurnCompleted }` round-trips, carrying the rich usage.
#[test]
fn agent_event_turn_completed_roundtrips() {
    let usage = marsad_proto::WasitUsage {
        input_tokens: 100,
        output_tokens: 20,
        cache_read_tokens: 5,
        cache_creation_tokens: 3,
        cost_usd: 0.5,
    };
    let msg = IpcMsg::AgentEvent {
        session_id: "s-3".to_string(),
        event: AgentEvent::TurnCompleted {
            end_reason: None,
            was_interrupted: false,
            num_turns: 1,
            usage: usage.clone(),
            per_model: std::collections::HashMap::new(),
            reply: Some("done".to_string()),
        },
    };
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("AgentEvent{TurnCompleted} must reparse");
    assert_eq!(reparsed, msg);
    match &reparsed {
        IpcMsg::AgentEvent {
            event:
                AgentEvent::TurnCompleted {
                    usage: u,
                    num_turns,
                    reply,
                    ..
                },
            ..
        } => {
            assert_eq!(*u, usage);
            assert_eq!(*num_turns, 1);
            assert_eq!(reply.as_deref(), Some("done"));
        }
        other => panic!("expected AgentEvent{{TurnCompleted}}, got {other:?}"),
    }
    assert_eq!(encode_ipc(&reparsed), encoded, "re-encode must be stable");
}

#[test]
fn encode_ipc_all_variants_produce_valid_json() {
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
            session_mode: "default".to_string(),
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
            event: AgentEvent::TurnCompleted {
                end_reason: None,
                was_interrupted: false,
                num_turns: 0,
                usage: marsad_proto::WasitUsage::default(),
                per_model: std::collections::HashMap::new(),
                reply: None,
            },
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
// LaunchMode / LaunchSpec — kept protocol data types
// ---------------------------------------------------------------------------

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
        r#"{"project":"/p","host_id":"local","model":"m","first_prompt":null}"#,
    )
    .expect("LaunchSpec without mode must parse");
    assert_eq!(spec.mode, LaunchMode::Fresh);
    // And with an explicit resume mode.
    let resumed: LaunchSpec = serde_json::from_str(
        r#"{"project":"/p","host_id":"local","model":"m","first_prompt":null,"mode":{"Resume":"abc"}}"#,
    )
    .expect("LaunchSpec with resume mode must parse");
    assert_eq!(resumed.mode, LaunchMode::Resume("abc".to_string()));
}

/// `LaunchSpec.disallowed_tools` defaults to empty when the field is absent.
#[test]
fn launch_spec_disallowed_tools_defaults_empty() {
    use marsad_proto::{LaunchMode, LaunchSpec};
    let spec = LaunchSpec {
        project: "/p".to_string(),
        host_id: "local".to_string(),
        model: "m".to_string(),
        session_mode: "default".to_string(),
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
    use marsad_proto::{LaunchMode, LaunchSpec};
    let spec = LaunchSpec {
        project: "/p".to_string(),
        host_id: "local".to_string(),
        model: "m".to_string(),
        session_mode: "default".to_string(),
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
    let json = r#"{"project":"/p","host_id":"local","model":"m","mode":"Fresh"}"#;
    let spec: LaunchSpec = serde_json::from_str(json).unwrap();
    assert!(spec.disallowed_tools.is_empty());
}
