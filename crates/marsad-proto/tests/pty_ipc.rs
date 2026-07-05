//! pty_ipc.rs — golden-JSON regression suite for the WAVE 3b PTY escape-hatch
//! IPC surface (interactive raw-shell terminal, `docs/design/DESIGN_HANDOFF.md`
//! "Terminal tab").
//!
//! This is the additive companion to `tests/ipc_typed.rs` (M2, FROZEN),
//! `tests/capabilities_ipc.rs` (U1, FROZEN), and `tests/wasl_ipc.rs` (X1,
//! FROZEN) — it does not touch or duplicate any of their golden tables; it
//! follows the exact same per-variant contract convention (golden parse /
//! encode-parse identity / re-encode stability / golden-encode-as-Value /
//! unknown-field tolerance) for the 5 NEW Pty tags, plus the KNOWN-closure
//! and required-field schema-error tests.
//!
//! Wire shapes:
//! - `PtyStart{session_id,cols,rows}` / `PtyResize{session_id,cols,rows}` —
//!   GUI/core → agent, no ack (fire-and-forget, mirrors `SetWorkspace`).
//! - `PtyInput{session_id,data}` — GUI/core → agent; `data` is PLAIN UTF-8
//!   text (terminal keystrokes are always a valid JS string from xterm.js's
//!   `onData`, even control sequences — no encoding loss in a JSON string).
//! - `PtyOutput{session_id,data}` — agent → GUI/core; `data` is BASE64 (raw
//!   pty bytes are arbitrary — control sequences, partial multi-byte UTF-8
//!   split across reads — and must round-trip byte-for-byte).
//! - `PtyExit{session_id,code}` — agent → GUI/core; the child process's exit
//!   code (`-1` when the pty could not even be spawned).

use marsad_proto::{encode_ipc, parse_ipc, IpcMsg, ProtoError};

// ---------------------------------------------------------------------------
// Shared helpers (duplicated per-file convention — see wasl_ipc.rs's header)
// ---------------------------------------------------------------------------

fn val(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("golden literal must itself be valid JSON")
}

fn inject_future_field(golden: &str) -> String {
    let mut v = val(golden);
    v.as_object_mut()
        .expect("golden must be a JSON object")
        .insert("x_future_field".to_string(), serde_json::json!(123));
    v.to_string()
}

fn assert_variant_contract(golden: &str, expected: &IpcMsg) {
    let parsed = parse_ipc(golden).unwrap_or_else(|e| panic!("golden must parse: {e:?}\n{golden}"));
    assert_eq!(&parsed, expected, "golden parse mismatch for {golden}");

    let encoded = encode_ipc(expected);
    let reparsed = parse_ipc(&encoded).expect("self-encoded IpcMsg must reparse");
    assert_eq!(
        &reparsed, expected,
        "encode->parse identity failed for {golden}"
    );
    assert_eq!(
        encode_ipc(&reparsed),
        encoded,
        "re-encode must be stable for {golden}"
    );

    assert_eq!(
        val(&encoded),
        val(golden),
        "golden encode Value mismatch for {golden}"
    );

    let tolerant = inject_future_field(golden);
    let parsed_tolerant = parse_ipc(&tolerant)
        .unwrap_or_else(|e| panic!("unknown-field-tolerant parse failed: {e:?}"));
    assert_eq!(
        &parsed_tolerant, expected,
        "unknown-field injection must not change the parsed value for {golden}"
    );
}

// ---------------------------------------------------------------------------
// PtyStart
// ---------------------------------------------------------------------------

const PTY_START: &str = r#"{"type":"PtyStart","session_id":"s-1","cols":80,"rows":24}"#;

fn pty_start() -> IpcMsg {
    IpcMsg::PtyStart {
        session_id: "s-1".to_string(),
        cols: 80,
        rows: 24,
    }
}

#[test]
fn pty_start_golden_contract() {
    assert_variant_contract(PTY_START, &pty_start());
}

#[test]
fn pty_start_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"PtyStart","cols":80,"rows":24}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "got {err:?}"
    );
}

#[test]
fn pty_start_missing_cols_is_schema_mismatch() {
    let line = r#"{"type":"PtyStart","session_id":"s-1","rows":24}"#;
    let err = parse_ipc(line).expect_err("missing cols must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "got {err:?}"
    );
}

#[test]
fn pty_start_missing_rows_is_schema_mismatch() {
    let line = r#"{"type":"PtyStart","session_id":"s-1","cols":80}"#;
    let err = parse_ipc(line).expect_err("missing rows must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// PtyInput — plain UTF-8 text, incl. control characters
// ---------------------------------------------------------------------------

const PTY_INPUT: &str = r#"{"type":"PtyInput","session_id":"s-1","data":"ls -la\n"}"#;

fn pty_input() -> IpcMsg {
    IpcMsg::PtyInput {
        session_id: "s-1".to_string(),
        data: "ls -la\n".to_string(),
    }
}

#[test]
fn pty_input_golden_contract() {
    assert_variant_contract(PTY_INPUT, &pty_input());
}

/// Control characters (Ctrl-C == ETX, U+0003) are valid Unicode scalar
/// values and must round-trip through the plain-text `data` field with no
/// encoding scheme — this is exactly why `PtyInput` does NOT need base64
/// (unlike `PtyOutput`).
#[test]
fn pty_input_control_character_roundtrips_as_plain_text() {
    let msg = IpcMsg::PtyInput {
        session_id: "s-1".to_string(),
        data: "\u{3}".to_string(), // Ctrl-C
    };
    let encoded = encode_ipc(&msg);
    let reparsed = parse_ipc(&encoded).expect("control character must roundtrip");
    assert_eq!(reparsed, msg);
    match reparsed {
        IpcMsg::PtyInput { data, .. } => assert_eq!(data, "\u{3}"),
        other => panic!("expected PtyInput, got {other:?}"),
    }
}

#[test]
fn pty_input_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"PtyInput","data":"x"}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "got {err:?}"
    );
}

#[test]
fn pty_input_missing_data_is_schema_mismatch() {
    let line = r#"{"type":"PtyInput","session_id":"s-1"}"#;
    let err = parse_ipc(line).expect_err("missing data must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// PtyResize
// ---------------------------------------------------------------------------

const PTY_RESIZE: &str = r#"{"type":"PtyResize","session_id":"s-1","cols":120,"rows":40}"#;

fn pty_resize() -> IpcMsg {
    IpcMsg::PtyResize {
        session_id: "s-1".to_string(),
        cols: 120,
        rows: 40,
    }
}

#[test]
fn pty_resize_golden_contract() {
    assert_variant_contract(PTY_RESIZE, &pty_resize());
}

#[test]
fn pty_resize_missing_cols_is_schema_mismatch() {
    let line = r#"{"type":"PtyResize","session_id":"s-1","rows":40}"#;
    let err = parse_ipc(line).expect_err("missing cols must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// PtyOutput — base64 raw bytes
// ---------------------------------------------------------------------------

const PTY_OUTPUT: &str = r#"{"type":"PtyOutput","session_id":"s-1","data":"bHMgLWxhCg=="}"#;

fn pty_output() -> IpcMsg {
    IpcMsg::PtyOutput {
        session_id: "s-1".to_string(),
        data: "bHMgLWxhCg==".to_string(), // base64("ls -la\n")
    }
}

#[test]
fn pty_output_golden_contract() {
    assert_variant_contract(PTY_OUTPUT, &pty_output());
}

#[test]
fn pty_output_missing_data_is_schema_mismatch() {
    let line = r#"{"type":"PtyOutput","session_id":"s-1"}"#;
    let err = parse_ipc(line).expect_err("missing data must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// PtyExit
// ---------------------------------------------------------------------------

const PTY_EXIT: &str = r#"{"type":"PtyExit","session_id":"s-1","code":0}"#;
const PTY_EXIT_SPAWN_FAILURE: &str = r#"{"type":"PtyExit","session_id":"s-1","code":-1}"#;

fn pty_exit() -> IpcMsg {
    IpcMsg::PtyExit {
        session_id: "s-1".to_string(),
        code: 0,
    }
}

fn pty_exit_spawn_failure() -> IpcMsg {
    IpcMsg::PtyExit {
        session_id: "s-1".to_string(),
        code: -1,
    }
}

#[test]
fn pty_exit_golden_contract() {
    assert_variant_contract(PTY_EXIT, &pty_exit());
}

/// `-1` is the documented sentinel for "the pty could not even be spawned"
/// (no real process ever ran, so there is no real exit code).
#[test]
fn pty_exit_spawn_failure_sentinel_golden_contract() {
    assert_variant_contract(PTY_EXIT_SPAWN_FAILURE, &pty_exit_spawn_failure());
}

#[test]
fn pty_exit_missing_code_is_schema_mismatch() {
    let line = r#"{"type":"PtyExit","session_id":"s-1"}"#;
    let err = parse_ipc(line).expect_err("missing code must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// KNOWN closure (parse_ipc's KNOWN list grows by exactly these 5 tags, 35->40)
// ---------------------------------------------------------------------------

#[test]
fn new_pty_tags_are_no_longer_unknown_discriminants() {
    for (line, _name) in [
        (PTY_START, "PtyStart"),
        (PTY_INPUT, "PtyInput"),
        (PTY_RESIZE, "PtyResize"),
        (PTY_OUTPUT, "PtyOutput"),
        (PTY_EXIT, "PtyExit"),
    ] {
        parse_ipc(line).unwrap_or_else(|e| panic!("{_name} must no longer be unknown: {e:?}"));
    }
}

/// A type string that is neither one of the pre-existing 35 tags nor one of
/// these 5 new ones must still cleanly be `UnknownDiscriminant`.
#[test]
fn genuinely_unknown_type_is_still_unknown_discriminant() {
    let line = r#"{"type":"PtyIpcProbeNotARealVariant","session_id":"s-1"}"#;
    let err = parse_ipc(line).expect_err("unrecognized IPC type must error");
    match err {
        ProtoError::UnknownDiscriminant(s) => assert_eq!(s, "PtyIpcProbeNotARealVariant"),
        other => panic!("expected UnknownDiscriminant, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Table-driven sweep over all 5 new goldens
// ---------------------------------------------------------------------------

fn all_new_goldens() -> Vec<(&'static str, &'static str, IpcMsg)> {
    vec![
        ("PtyStart", PTY_START, pty_start()),
        ("PtyInput", PTY_INPUT, pty_input()),
        ("PtyResize", PTY_RESIZE, pty_resize()),
        ("PtyOutput", PTY_OUTPUT, pty_output()),
        ("PtyExit", PTY_EXIT, pty_exit()),
    ]
}

#[test]
fn every_new_golden_parses_to_its_expected_typed_value() {
    for (name, golden, expected) in &all_new_goldens() {
        let parsed =
            parse_ipc(golden).unwrap_or_else(|e| panic!("{name}: golden must parse: {e:?}"));
        assert_eq!(&parsed, expected, "{name}: golden parse mismatch");
    }
}

#[test]
fn every_new_golden_tolerates_an_injected_unknown_field() {
    for (name, golden, expected) in &all_new_goldens() {
        let tolerant = inject_future_field(golden);
        let parsed = parse_ipc(&tolerant)
            .unwrap_or_else(|e| panic!("{name}: unknown-field-tolerant parse failed: {e:?}"));
        assert_eq!(
            &parsed, expected,
            "{name}: injected unknown field must not change the parsed value"
        );
    }
}
