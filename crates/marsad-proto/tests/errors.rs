/// errors.rs — every ProtoError variant for both stream and IPC parsers.
///
/// These tests WILL fail to compile until the code-agent implements `marsad_proto`.
/// That is the expected tests-first signal.
use marsad_proto::{parse_ipc, parse_stream_line, ProtoError};

// ---------------------------------------------------------------------------
// MalformedJson — truncated / syntactically invalid JSON
// ---------------------------------------------------------------------------

#[test]
fn stream_malformed_truncated_json() {
    let line = r#"{"type":"system","subtype":"init","cwd":"/work""#; // unclosed brace
    let err = parse_stream_line(line).expect_err("truncated JSON must be an error");
    assert!(
        matches!(err, ProtoError::MalformedJson(_)),
        "expected MalformedJson, got {err:?}"
    );
}

#[test]
fn stream_malformed_empty_string() {
    let err = parse_stream_line("").expect_err("empty string must be an error");
    assert!(
        matches!(err, ProtoError::MalformedJson(_)),
        "expected MalformedJson, got {err:?}"
    );
}

#[test]
fn stream_malformed_not_json_at_all() {
    let err = parse_stream_line("this is not json").expect_err("non-JSON must be an error");
    assert!(
        matches!(err, ProtoError::MalformedJson(_)),
        "expected MalformedJson, got {err:?}"
    );
}

#[test]
fn stream_malformed_json_array_not_object() {
    // A JSON array is syntactically valid JSON but not an object — must be MalformedJson
    // (the protocol requires a JSON object per line).
    let err = parse_stream_line("[1,2,3]").expect_err("JSON array must be an error");
    assert!(
        matches!(err, ProtoError::MalformedJson(_)),
        "expected MalformedJson, got {err:?}"
    );
}

#[test]
fn ipc_malformed_truncated_json() {
    let line = r#"{"type":"LaunchSession""#;
    let err = parse_ipc(line).expect_err("truncated JSON must be an error");
    assert!(
        matches!(err, ProtoError::MalformedJson(_)),
        "expected MalformedJson, got {err:?}"
    );
}

#[test]
fn ipc_malformed_empty_string() {
    let err = parse_ipc("").expect_err("empty string must be IPC error");
    assert!(
        matches!(err, ProtoError::MalformedJson(_)),
        "expected MalformedJson, got {err:?}"
    );
}

#[test]
fn ipc_malformed_not_json() {
    let err = parse_ipc("not json at all!").expect_err("non-JSON IPC must be an error");
    assert!(
        matches!(err, ProtoError::MalformedJson(_)),
        "expected MalformedJson, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// MissingDiscriminant — valid JSON object but no "type" field
// ---------------------------------------------------------------------------

#[test]
fn stream_missing_discriminant() {
    // Valid JSON object, no "type" key
    let line = r#"{"subtype":"init","cwd":"/work","session_id":"abc"}"#;
    let err = parse_stream_line(line).expect_err("missing type field must be an error");
    assert!(
        matches!(err, ProtoError::MissingDiscriminant),
        "expected MissingDiscriminant, got {err:?}"
    );
}

#[test]
fn ipc_missing_discriminant() {
    let line = r#"{"cwd":"/work","model":"claude-opus-4.8"}"#;
    let err = parse_ipc(line).expect_err("missing type field must be an IPC error");
    assert!(
        matches!(err, ProtoError::MissingDiscriminant),
        "expected MissingDiscriminant, got {err:?}"
    );
}

#[test]
fn stream_type_field_is_null() {
    // "type" key present but null — should be MissingDiscriminant (no usable discriminant value)
    let line = r#"{"type":null,"subtype":"init"}"#;
    let err = parse_stream_line(line).expect_err("null type must be an error");
    assert!(
        matches!(
            err,
            ProtoError::MissingDiscriminant | ProtoError::UnknownDiscriminant(_)
        ),
        "expected MissingDiscriminant or UnknownDiscriminant for null type, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// UnknownDiscriminant — valid JSON, "type" present but not a recognized variant
// (NOTE: for stream, unrecognized types become Unknown, not an error — see branches.rs.
//  For IPC, unrecognized types are UnknownDiscriminant because IPC has no Unknown escape hatch
//  per spec.  If the spec later adds Unknown to IPC, update accordingly.)
// ---------------------------------------------------------------------------

#[test]
fn ipc_unknown_discriminant() {
    let line = r#"{"type":"NonExistentIpcVariant","data":"foo"}"#;
    let err = parse_ipc(line).expect_err("unknown IPC type must be an error");
    assert!(
        matches!(err, ProtoError::UnknownDiscriminant(_)),
        "expected UnknownDiscriminant, got {err:?}"
    );
}

#[test]
fn ipc_unknown_discriminant_carries_the_type_string() {
    let line = r#"{"type":"FutureMsg","payload":42}"#;
    let err = parse_ipc(line).expect_err("unknown IPC type must be an error");
    match err {
        ProtoError::UnknownDiscriminant(s) => {
            assert_eq!(s, "FutureMsg", "discriminant string must match the type field");
        }
        other => panic!("expected UnknownDiscriminant(\"FutureMsg\"), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// SchemaMismatch — correct "type" but wrong field type
// ---------------------------------------------------------------------------

#[test]
fn stream_schema_mismatch_wrong_field_type() {
    // "num_turns" should be a number, supplying a string should fail schema validation.
    let line = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":"not-a-number","total_cost_usd":0.0,"usage":{},"permission_denials":[]}"#;
    let err = parse_stream_line(line).expect_err("wrong field type must be an error");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn stream_schema_mismatch_carries_field_name() {
    // "is_error" should be bool, not string
    let line = r#"{"type":"result","subtype":"success","is_error":"yes","num_turns":1,"total_cost_usd":0.0,"usage":{},"permission_denials":[]}"#;
    let err = parse_stream_line(line).expect_err("wrong bool field must be an error");
    match err {
        ProtoError::SchemaMismatch { field, detail } => {
            // field name should point at the offending field
            assert!(
                !field.is_empty(),
                "SchemaMismatch must carry a non-empty field name"
            );
            assert!(
                !detail.is_empty(),
                "SchemaMismatch must carry a non-empty detail"
            );
        }
        other => panic!("expected SchemaMismatch, got {other:?}"),
    }
}

#[test]
fn ipc_schema_mismatch_wrong_field_type() {
    // "allow" should be a boolean, supplying a number should fail
    let line = r#"{"type":"AnswerGate","request_id":"req-1","allow":42,"deny_reason":null}"#;
    let err = parse_ipc(line).expect_err("wrong field type in IPC must be an error");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// ProtoError derives Debug — confirm we can format it
// ---------------------------------------------------------------------------

#[test]
fn proto_error_debug_format() {
    let errs: Vec<ProtoError> = vec![
        ProtoError::MalformedJson("bad json".to_string()),
        ProtoError::MissingDiscriminant,
        ProtoError::UnknownDiscriminant("FutureType".to_string()),
        ProtoError::SchemaMismatch {
            field: "num_turns",
            detail: "expected u64".to_string(),
        },
    ];
    for e in &errs {
        // Just assert it doesn't panic — Debug must be implemented.
        let s = format!("{e:?}");
        assert!(!s.is_empty(), "Debug output must not be empty");
    }
}
