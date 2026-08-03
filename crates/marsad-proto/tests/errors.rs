/// errors.rs — every ProtoError variant for the IPC parser.
///
/// Wave 0 removed the Claude stream-json types (`StreamMsg`, `parse_stream_line`)
/// from `marsad-proto`; the stream-parser error cases that used to live here went
/// with them (stream parsing now happens in the wasit drivers). The IPC-parser
/// error cases below are unchanged.
use marsad_proto::{parse_ipc, ProtoError};

// ---------------------------------------------------------------------------
// MalformedJson — truncated / syntactically invalid JSON
// ---------------------------------------------------------------------------

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
fn ipc_missing_discriminant() {
    let line = r#"{"cwd":"/work","model":"claude-opus-4.8"}"#;
    let err = parse_ipc(line).expect_err("missing type field must be an IPC error");
    assert!(
        matches!(err, ProtoError::MissingDiscriminant),
        "expected MissingDiscriminant, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// UnknownDiscriminant — valid JSON, "type" present but not a recognized variant.
//
// IPC protocol: unrecognized types are UnknownDiscriminant (an error). The IPC
// protocol is a closed GUI<->agent surface; an unknown type is a bug, not
// forward-compat. There is no Unknown variant in IpcMsg — this is final per spec.
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
            assert_eq!(
                s, "FutureMsg",
                "discriminant string must match the type field"
            );
        }
        other => panic!("expected UnknownDiscriminant(\"FutureMsg\"), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// SchemaMismatch — correct "type" but wrong field type
// ---------------------------------------------------------------------------

#[test]
fn ipc_schema_mismatch_wrong_field_type() {
    // "allow" should be a boolean, supplying a number should fail
    let line = r#"{"type":"AnswerGate","session_id":"s-1","request_id":"req-1","allow":42,"deny_reason":null}"#;
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
