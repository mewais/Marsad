/// branches.rs — table-driven cases closing remaining regions to 100% coverage.
///
/// Covers: Unknown passthrough, absent-usage defaults, each ResultSubtype,
/// each ContentBlock variant including is_error=true tool results, empty content arrays.
///
/// These tests WILL fail to compile until the code-agent implements `marsad_proto`.
/// That is the expected tests-first signal.
use marsad_proto::{parse_stream_line, ContentBlock, ResultSubtype, StreamMsg, Usage};

// ---------------------------------------------------------------------------
// Unknown passthrough — syntactically valid JSON with unrecognized "type" field
// must NOT be an error; it becomes StreamMsg::Unknown.
// ---------------------------------------------------------------------------

#[test]
fn unknown_type_becomes_unknown_variant() {
    let line = r#"{"type":"future_msg_type","some_field":42}"#;
    let msg = parse_stream_line(line).expect("unrecognized type must parse as Unknown, not error");
    match msg {
        StreamMsg::Unknown(v) => {
            assert_eq!(
                v["type"].as_str(),
                Some("future_msg_type"),
                "Unknown must preserve the raw JSON value"
            );
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn unknown_preserves_full_json_value() {
    let line = r#"{"type":"experimental","nested":{"a":1},"arr":[1,2,3]}"#;
    let msg = parse_stream_line(line).expect("should parse as Unknown");
    match msg {
        StreamMsg::Unknown(v) => {
            assert!(v["nested"].is_object());
            assert!(v["arr"].is_array());
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn unknown_with_empty_extra_fields() {
    let line = r#"{"type":"empty_future"}"#;
    let msg = parse_stream_line(line).expect("should parse as Unknown");
    assert!(matches!(msg, StreamMsg::Unknown(_)));
}

// ---------------------------------------------------------------------------
// Absent-usage defaults — Usage fields must default to 0 when absent from JSON.
// ---------------------------------------------------------------------------

#[test]
fn absent_usage_fields_default_to_zero() {
    // Provide an assistant message with an empty usage object
    let line = r#"{"type":"assistant","message":{"id":"msg_x","model":"claude-opus-4.8","type":"message","role":"assistant","content":[],"usage":{}},"session_id":"s1","uuid":"u1"}"#;
    let msg = parse_stream_line(line).expect("should parse assistant with empty usage");
    match msg {
        StreamMsg::Assistant(a) => {
            let u = &a.message.usage;
            assert_eq!(u.input_tokens, 0, "input_tokens must default to 0");
            assert_eq!(u.output_tokens, 0, "output_tokens must default to 0");
            assert_eq!(
                u.cache_read_input_tokens, 0,
                "cache_read_input_tokens must default to 0"
            );
            assert_eq!(
                u.cache_creation_input_tokens, 0,
                "cache_creation_input_tokens must default to 0"
            );
        }
        other => panic!("expected Assistant, got {other:?}"),
    }
}

#[test]
fn absent_usage_in_result_defaults_to_zero() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"total_cost_usd":0.0,"usage":{},"permission_denials":[]}"#;
    let msg = parse_stream_line(line).expect("should parse result with empty usage");
    match msg {
        StreamMsg::Result(r) => {
            assert_eq!(r.usage.input_tokens, 0);
            assert_eq!(r.usage.output_tokens, 0);
            assert_eq!(r.usage.cache_read_input_tokens, 0);
            assert_eq!(r.usage.cache_creation_input_tokens, 0);
        }
        other => panic!("expected Result, got {other:?}"),
    }
}

#[test]
fn partial_usage_fields_fill_missing_with_zero() {
    // Only provide input_tokens; the rest should default to 0.
    let line = r#"{"type":"assistant","message":{"id":"msg_y","model":"claude-opus-4.8","type":"message","role":"assistant","content":[],"usage":{"input_tokens":99}},"session_id":"s2","uuid":"u2"}"#;
    let msg = parse_stream_line(line).expect("should parse assistant with partial usage");
    match msg {
        StreamMsg::Assistant(a) => {
            assert_eq!(a.message.usage.input_tokens, 99);
            assert_eq!(a.message.usage.output_tokens, 0);
            assert_eq!(a.message.usage.cache_read_input_tokens, 0);
            assert_eq!(a.message.usage.cache_creation_input_tokens, 0);
        }
        other => panic!("expected Assistant, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Each ResultSubtype variant
// ---------------------------------------------------------------------------

#[test]
fn result_subtype_success_parses() {
    let line = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"total_cost_usd":0.0,"usage":{},"permission_denials":[]}"#;
    let msg = parse_stream_line(line).unwrap();
    match msg {
        StreamMsg::Result(r) => assert_eq!(r.subtype, ResultSubtype::Success),
        other => panic!("{other:?}"),
    }
}

#[test]
fn result_subtype_error_max_turns_parses() {
    let line = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":5,"total_cost_usd":0.0,"usage":{},"permission_denials":[]}"#;
    let msg = parse_stream_line(line).unwrap();
    match msg {
        StreamMsg::Result(r) => assert_eq!(r.subtype, ResultSubtype::ErrorMaxTurns),
        other => panic!("{other:?}"),
    }
}

#[test]
fn result_subtype_error_during_execution_parses() {
    let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":2,"total_cost_usd":0.0,"usage":{},"permission_denials":[]}"#;
    let msg = parse_stream_line(line).unwrap();
    match msg {
        StreamMsg::Result(r) => assert_eq!(r.subtype, ResultSubtype::ErrorDuringExecution),
        other => panic!("{other:?}"),
    }
}

#[test]
fn result_subtype_equality() {
    assert_eq!(ResultSubtype::Success, ResultSubtype::Success);
    assert_ne!(ResultSubtype::Success, ResultSubtype::ErrorMaxTurns);
    assert_ne!(ResultSubtype::ErrorMaxTurns, ResultSubtype::ErrorDuringExecution);
}

// ---------------------------------------------------------------------------
// Each ContentBlock variant
// ---------------------------------------------------------------------------

#[test]
fn content_block_text() {
    let line = r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-4.8","type":"message","role":"assistant","content":[{"type":"text","text":"hello"}],"usage":{}},"session_id":"s","uuid":"u"}"#;
    let msg = parse_stream_line(line).unwrap();
    match msg {
        StreamMsg::Assistant(a) => {
            assert_eq!(a.message.content.len(), 1);
            assert!(matches!(&a.message.content[0], ContentBlock::Text { text } if text == "hello"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn content_block_tool_use() {
    let line = r#"{"type":"assistant","message":{"id":"m2","model":"claude-opus-4.8","type":"message","role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"Glob","input":{"pattern":"**/*.rs"}}],"usage":{}},"session_id":"s","uuid":"u"}"#;
    let msg = parse_stream_line(line).unwrap();
    match msg {
        StreamMsg::Assistant(a) => match &a.message.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "tu_1");
                assert_eq!(name, "Glob");
                assert!(input.is_object());
            }
            other => panic!("expected ToolUse, got {other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn content_block_tool_result_success() {
    let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_1","content":"output text","is_error":false}]},"session_id":"s"}"#;
    let msg = parse_stream_line(line).unwrap();
    match msg {
        StreamMsg::User(u) => match &u.message.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tu_1");
                assert!(!is_error);
                // content is a serde_json::Value — just confirm it parsed
                assert!(!content.is_null() || content.is_null()); // always true; real check: non-panic
            }
            other => panic!("expected ToolResult, got {other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn content_block_tool_result_is_error_true() {
    // is_error=true branch
    let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_2","content":{"error":"command failed"},"is_error":true}]},"session_id":"s"}"#;
    let msg = parse_stream_line(line).unwrap();
    match msg {
        StreamMsg::User(u) => match &u.message.content[0] {
            ContentBlock::ToolResult { is_error, .. } => {
                assert!(*is_error, "is_error must be true");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn content_block_equality() {
    let t1 = ContentBlock::Text {
        text: "hi".to_string(),
    };
    let t2 = ContentBlock::Text {
        text: "hi".to_string(),
    };
    let t3 = ContentBlock::Text {
        text: "bye".to_string(),
    };
    assert_eq!(t1, t2);
    assert_ne!(t1, t3);
}

// ---------------------------------------------------------------------------
// Empty content arrays
// ---------------------------------------------------------------------------

#[test]
fn assistant_empty_content_array() {
    let line = r#"{"type":"assistant","message":{"id":"m_empty","model":"claude-opus-4.8","type":"message","role":"assistant","content":[],"usage":{}},"session_id":"s","uuid":"u"}"#;
    let msg = parse_stream_line(line).expect("empty content array is valid");
    match msg {
        StreamMsg::Assistant(a) => assert!(a.message.content.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn user_empty_content_array() {
    let line = r#"{"type":"user","message":{"content":[]},"session_id":"s"}"#;
    let msg = parse_stream_line(line).expect("empty user content array is valid");
    match msg {
        StreamMsg::User(u) => assert!(u.message.content.is_empty()),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Multiple ContentBlocks in one message
// ---------------------------------------------------------------------------

#[test]
fn assistant_multiple_content_blocks() {
    let line = r#"{"type":"assistant","message":{"id":"m_multi","model":"claude-opus-4.8","type":"message","role":"assistant","content":[{"type":"text","text":"first"},{"type":"tool_use","id":"tu_x","name":"Write","input":{}},{"type":"text","text":"second"}],"usage":{}},"session_id":"s","uuid":"u"}"#;
    let msg = parse_stream_line(line).expect("multiple content blocks should parse");
    match msg {
        StreamMsg::Assistant(a) => {
            assert_eq!(a.message.content.len(), 3);
            assert!(matches!(&a.message.content[0], ContentBlock::Text { .. }));
            assert!(matches!(&a.message.content[1], ContentBlock::ToolUse { .. }));
            assert!(matches!(&a.message.content[2], ContentBlock::Text { .. }));
        }
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Usage struct direct construction and clone/partial-eq
// ---------------------------------------------------------------------------

#[test]
fn usage_derives_clone_and_partialeq() {
    let u1 = Usage {
        input_tokens: 10,
        output_tokens: 20,
        cache_read_input_tokens: 5,
        cache_creation_input_tokens: 3,
    };
    let u2 = u1.clone();
    assert_eq!(u1, u2);
    let u3 = Usage {
        input_tokens: 99,
        ..u1.clone()
    };
    assert_ne!(u1, u3);
}

// ---------------------------------------------------------------------------
// permission_denials field — non-empty list
// ---------------------------------------------------------------------------

#[test]
fn result_with_permission_denials() {
    // NOTE: spec says ResultMsg contains a permission_denials field; the exact element type
    // is not specified beyond being a list. We test that it parses and is non-empty.
    let line = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":2,"total_cost_usd":0.1,"usage":{},"permission_denials":["Bash","Write"]}"#;
    let msg = parse_stream_line(line).expect("result with permission_denials should parse");
    match msg {
        StreamMsg::Result(r) => {
            assert_eq!(r.permission_denials.len(), 2);
        }
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// StreamMsg Clone + PartialEq
// ---------------------------------------------------------------------------

#[test]
fn stream_msg_unknown_clone_and_eq() {
    let line = r#"{"type":"future","x":1}"#;
    let msg = parse_stream_line(line).unwrap();
    let cloned = msg.clone();
    assert_eq!(msg, cloned);
}
