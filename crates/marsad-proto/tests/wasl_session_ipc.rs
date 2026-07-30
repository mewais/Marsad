//! wasl_session_ipc.rs — golden-JSON regression suite for the NEW Wasl **backend**
//! (Wasl-managed session) IPC surface (`docs/design/wasl-backend-plan.md` §7,
//! v2 finalized).
//!
//! This is the additive companion to `tests/wasl_ipc.rs` (the X1 Wasl *operator*
//! connector variants — a DIFFERENT feature: send-side operator gates for a
//! session Marsad still owns). This file covers the 10 NEW `IpcMsg` variants for
//! the Wasl **backend** (a session whose process Wasl owns end-to-end; Marsad is
//! a thin client): the six core→agent commands
//! (`LaunchWaslSession`, `SendWaslPrompt`, `AttachWaslSession`,
//! `AnswerWaslNativeGate`, `InterruptWaslSession`, `StopWaslSession`) and the
//! four agent→core broadcasts/acks
//! (`WaslSessionStatus`, `WaslSessionEvent`, `WaslPromptReceipt`,
//! `WaslNativeGateAnswered`).
//!
//! It follows the exact per-variant contract convention of `tests/wasl_ipc.rs`
//! (golden parse / encode->parse identity / re-encode stability /
//! golden-encode-as-Value / unknown-field tolerance), plus the KNOWN-closure and
//! required-identity-field (`session_id`/`host_id`/`gate_id`) schema-error tests
//! §7 calls out, plus the §7 v2 "round-trip typed-conversion tests at the crate
//! boundary" requirement for the Value-carried payloads
//! (`wasl_target_key`/`status`/`event`/`receipt`).
//!
//! Payloads whose typed form lives in `marsad-wasl-session` are carried as
//! `serde_json::Value` at THIS crate's boundary (§7: `marsad-proto` must NOT
//! depend on `marsad-wasl-session`) — so every payload in this file is an OPAQUE
//! JSON literal, not a typed struct. The round-trip tests prove those nested
//! shapes survive across the wire without loss.
//!
//! This file WILL fail to compile until the code-agent lands the 10 new
//! `IpcMsg` variants and grows `parse_ipc`'s `KNOWN` list by exactly these 10
//! tags (§7). That is the expected tests-first compile-failure signal.

use marsad_proto::{encode_ipc, parse_ipc, IpcMsg, ProtoError};

// ---------------------------------------------------------------------------
// Shared helpers (mirrors tests/wasl_ipc.rs's `val`/`inject_future_field`/
// `assert_variant_contract` exactly — duplicated here because each file in
// tests/ is compiled as an independent crate).
// ---------------------------------------------------------------------------

fn val(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("golden literal must itself be valid JSON")
}

/// Inject an unknown field into a golden JSON object literal, mimicking a newer
/// sender adding an additive field this build doesn't know about yet (§7:
/// "`#[serde(default)]` tolerance on non-identity fields").
fn inject_future_field(golden: &str) -> String {
    let mut v = val(golden);
    v.as_object_mut()
        .expect("golden must be a JSON object")
        .insert("x_future_field".to_string(), serde_json::json!(123));
    v.to_string()
}

/// The full per-variant contract: golden parse, encode->parse identity +
/// re-encode stability, golden encode as `Value`, and unknown-field tolerance.
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

// ===========================================================================
// core -> agent commands
// ===========================================================================

// ---------------------------------------------------------------------------
// LaunchWaslSession — §7. Identity: session_id, host_id (required). Optional:
// wasl_target_key (default Value), name (default Option).
// ---------------------------------------------------------------------------

// A non-trivial NESTED wasl_target_key (protocol id + context) — proves the
// Value payload survives more than a flat object (§7 drift-guard requirement).
const LAUNCH_WASL_SESSION: &str = r#"{"type":"LaunchWaslSession","session_id":"s-1","host_id":"local","wasl_target_key":{"protocol":"claude-native","context":{"cwd":"/home/u/proj","env":[["FOO","bar"]],"nested":{"depth":2,"list":[1,2,3]}}},"name":"My Wasl run"}"#;

fn launch_wasl_session() -> IpcMsg {
    IpcMsg::LaunchWaslSession {
        session_id: "s-1".to_string(),
        host_id: "local".to_string(),
        wasl_target_key: serde_json::json!({
            "protocol": "claude-native",
            "context": {
                "cwd": "/home/u/proj",
                "env": [["FOO", "bar"]],
                "nested": {"depth": 2, "list": [1, 2, 3]},
            },
        }),
        name: Some("My Wasl run".to_string()),
    }
}

#[test]
fn launch_wasl_session_golden_contract() {
    assert_variant_contract(LAUNCH_WASL_SESSION, &launch_wasl_session());
}

/// §7: `wasl_target_key` is `serde_json::Value` ON PURPOSE (typed form
/// `WaslTargetKey` lives in marsad-wasl-session; proto must not depend on it).
/// Assert it round-trips as an opaque, arbitrarily-nested object.
#[test]
fn launch_wasl_session_target_key_is_opaque_nested_value() {
    match launch_wasl_session() {
        IpcMsg::LaunchWaslSession {
            wasl_target_key, ..
        } => {
            assert!(
                wasl_target_key.is_object(),
                "wasl_target_key must be a JSON object Value"
            );
            assert_eq!(
                wasl_target_key["context"]["nested"]["depth"],
                serde_json::json!(2),
                "deeply-nested payload fields must survive without loss"
            );
            assert_eq!(
                wasl_target_key["context"]["nested"]["list"],
                serde_json::json!([1, 2, 3])
            );
        }
        other => panic!("expected LaunchWaslSession, got {other:?}"),
    }
}

/// Non-identity fields absent → documented defaults (empty/null Value; None name).
#[test]
fn launch_wasl_session_absent_optionals_parse_to_defaults() {
    let line = r#"{"type":"LaunchWaslSession","session_id":"s-1","host_id":"local"}"#;
    let parsed = parse_ipc(line).expect("non-identity fields must be parse-tolerant");
    match parsed {
        IpcMsg::LaunchWaslSession {
            session_id,
            host_id,
            wasl_target_key,
            name,
        } => {
            assert_eq!(session_id, "s-1");
            assert_eq!(host_id, "local");
            assert_eq!(wasl_target_key, serde_json::Value::Null);
            assert_eq!(name, None);
        }
        other => panic!("expected LaunchWaslSession, got {other:?}"),
    }
}

#[test]
fn launch_wasl_session_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"LaunchWaslSession","host_id":"local"}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn launch_wasl_session_missing_host_id_is_schema_mismatch() {
    let line = r#"{"type":"LaunchWaslSession","session_id":"s-1"}"#;
    let err = parse_ipc(line).expect_err("missing host_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// SendWaslPrompt — §7. Identity: session_id (required). `text` required (no
// #[serde(default)] in §7). Optional: idempotency_key, busy_policy, max_wait_ms.
// ---------------------------------------------------------------------------

const SEND_WASL_PROMPT: &str = r#"{"type":"SendWaslPrompt","session_id":"s-1","text":"summarize the repo","idempotency_key":"018f3c2a-7b1e-7c9d-9f01-abc123456789","busy_policy":"queue","max_wait_ms":30000}"#;

fn send_wasl_prompt() -> IpcMsg {
    IpcMsg::SendWaslPrompt {
        session_id: "s-1".to_string(),
        text: "summarize the repo".to_string(),
        idempotency_key: "018f3c2a-7b1e-7c9d-9f01-abc123456789".to_string(),
        busy_policy: "queue".to_string(),
        max_wait_ms: 30000,
    }
}

#[test]
fn send_wasl_prompt_golden_contract() {
    assert_variant_contract(SEND_WASL_PROMPT, &send_wasl_prompt());
}

/// Non-identity fields absent → documented defaults (empty idempotency_key,
/// empty busy_policy, 0 max_wait_ms). `text` is present because §7 declares no
/// default for it (see the missing-`text` test below).
#[test]
fn send_wasl_prompt_absent_optionals_parse_to_defaults() {
    let line = r#"{"type":"SendWaslPrompt","session_id":"s-1","text":"hi"}"#;
    let parsed = parse_ipc(line).expect("optional fields must be parse-tolerant");
    match parsed {
        IpcMsg::SendWaslPrompt {
            session_id,
            text,
            idempotency_key,
            busy_policy,
            max_wait_ms,
        } => {
            assert_eq!(session_id, "s-1");
            assert_eq!(text, "hi");
            assert_eq!(idempotency_key, "");
            assert_eq!(busy_policy, "");
            assert_eq!(max_wait_ms, 0);
        }
        other => panic!("expected SendWaslPrompt, got {other:?}"),
    }
}

#[test]
fn send_wasl_prompt_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"SendWaslPrompt","text":"hi"}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

/// ASSUMPTION (flagged in report): §7's `SendWaslPrompt` declares `text`
/// WITHOUT `#[serde(default)]` — i.e. required, like `LaunchSession.model`. A
/// prompt with no text is not a meaningful send, so a missing `text` is a
/// SchemaMismatch, mirroring how `SendPrompt.text` (the MarsadManaged analogue)
/// is a required field.
#[test]
fn send_wasl_prompt_missing_text_is_schema_mismatch() {
    let line = r#"{"type":"SendWaslPrompt","session_id":"s-1"}"#;
    let err = parse_ipc(line).expect_err("missing text must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// AttachWaslSession — §7. Identity: session_id (required). Optional: after_seq.
// ---------------------------------------------------------------------------

const ATTACH_WASL_SESSION: &str =
    r#"{"type":"AttachWaslSession","session_id":"s-1","after_seq":42}"#;

fn attach_wasl_session() -> IpcMsg {
    IpcMsg::AttachWaslSession {
        session_id: "s-1".to_string(),
        after_seq: 42,
    }
}

#[test]
fn attach_wasl_session_golden_contract() {
    assert_variant_contract(ATTACH_WASL_SESSION, &attach_wasl_session());
}

/// §6.1 reconnect: a fresh attach (after a "core restarted, session was gone"
/// path) attaches from seq 0. Absent `after_seq` must default to 0 so an old /
/// minimal sender is byte-equivalent to "from the beginning".
#[test]
fn attach_wasl_session_absent_after_seq_defaults_to_zero() {
    let line = r#"{"type":"AttachWaslSession","session_id":"s-1"}"#;
    let parsed = parse_ipc(line).expect("after_seq must be parse-tolerant (default 0)");
    match parsed {
        IpcMsg::AttachWaslSession {
            session_id,
            after_seq,
        } => {
            assert_eq!(session_id, "s-1");
            assert_eq!(after_seq, 0);
        }
        other => panic!("expected AttachWaslSession, got {other:?}"),
    }
}

#[test]
fn attach_wasl_session_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"AttachWaslSession","after_seq":5}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// AnswerWaslNativeGate — §7. Identity: session_id, gate_id (required). `outcome`
// required (no #[serde(default)] in §7). Optional: reason (default Option).
// ---------------------------------------------------------------------------

const ANSWER_WASL_NATIVE_GATE_APPROVED: &str = r#"{"type":"AnswerWaslNativeGate","session_id":"s-1","gate_id":"ng-1","outcome":"approved","reason":null}"#;

const ANSWER_WASL_NATIVE_GATE_DENIED: &str = r#"{"type":"AnswerWaslNativeGate","session_id":"s-1","gate_id":"ng-1","outcome":"denied","reason":"not allowed to run rm -rf"}"#;

fn answer_wasl_native_gate_approved() -> IpcMsg {
    IpcMsg::AnswerWaslNativeGate {
        session_id: "s-1".to_string(),
        gate_id: "ng-1".to_string(),
        outcome: "approved".to_string(),
        reason: None,
    }
}

fn answer_wasl_native_gate_denied() -> IpcMsg {
    IpcMsg::AnswerWaslNativeGate {
        session_id: "s-1".to_string(),
        gate_id: "ng-1".to_string(),
        outcome: "denied".to_string(),
        reason: Some("not allowed to run rm -rf".to_string()),
    }
}

#[test]
fn answer_wasl_native_gate_approved_golden_contract() {
    assert_variant_contract(
        ANSWER_WASL_NATIVE_GATE_APPROVED,
        &answer_wasl_native_gate_approved(),
    );
}

#[test]
fn answer_wasl_native_gate_denied_golden_contract() {
    assert_variant_contract(
        ANSWER_WASL_NATIVE_GATE_DENIED,
        &answer_wasl_native_gate_denied(),
    );
}

/// §7 mirrors `AnswerWaslGate.reason` semantics ("ALWAYS serialized, null when
/// absent", like `LaunchSession.first_prompt`). Pin it explicitly.
#[test]
fn answer_wasl_native_gate_reason_none_reencodes_as_literal_null_key_present() {
    let encoded = encode_ipc(&answer_wasl_native_gate_approved());
    let obj = val(&encoded);
    let obj = obj.as_object().expect("must be a JSON object");
    assert_eq!(
        obj.get("reason"),
        Some(&serde_json::Value::Null),
        "reason must ALWAYS be serialized (null when absent), got: {encoded}"
    );
}

#[test]
fn answer_wasl_native_gate_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"AnswerWaslNativeGate","gate_id":"ng-1","outcome":"approved"}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn answer_wasl_native_gate_missing_gate_id_is_schema_mismatch() {
    let line = r#"{"type":"AnswerWaslNativeGate","session_id":"s-1","outcome":"approved"}"#;
    let err = parse_ipc(line).expect_err("missing gate_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// InterruptWaslSession — §7. Identity: session_id (required). No other fields.
// ---------------------------------------------------------------------------

const INTERRUPT_WASL_SESSION: &str = r#"{"type":"InterruptWaslSession","session_id":"s-1"}"#;

fn interrupt_wasl_session() -> IpcMsg {
    IpcMsg::InterruptWaslSession {
        session_id: "s-1".to_string(),
    }
}

#[test]
fn interrupt_wasl_session_golden_contract() {
    assert_variant_contract(INTERRUPT_WASL_SESSION, &interrupt_wasl_session());
}

#[test]
fn interrupt_wasl_session_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"InterruptWaslSession"}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// StopWaslSession — §7. Identity: session_id (required). Optional: drain.
// ---------------------------------------------------------------------------

const STOP_WASL_SESSION: &str = r#"{"type":"StopWaslSession","session_id":"s-1","drain":true}"#;

fn stop_wasl_session() -> IpcMsg {
    IpcMsg::StopWaslSession {
        session_id: "s-1".to_string(),
        drain: true,
    }
}

#[test]
fn stop_wasl_session_golden_contract() {
    assert_variant_contract(STOP_WASL_SESSION, &stop_wasl_session());
}

/// §7: `drain` = Drain else Immediate. Absent → default false (Immediate), so a
/// minimal sender defaults to the non-draining shutdown.
#[test]
fn stop_wasl_session_absent_drain_defaults_to_false() {
    let line = r#"{"type":"StopWaslSession","session_id":"s-1"}"#;
    let parsed = parse_ipc(line).expect("drain must be parse-tolerant (default false)");
    match parsed {
        IpcMsg::StopWaslSession { session_id, drain } => {
            assert_eq!(session_id, "s-1");
            assert!(!drain);
        }
        other => panic!("expected StopWaslSession, got {other:?}"),
    }
}

#[test]
fn stop_wasl_session_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"StopWaslSession","drain":true}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ===========================================================================
// agent -> core broadcasts / acks
// ===========================================================================

// ---------------------------------------------------------------------------
// WaslSessionStatus — §7. Identity: session_id (required). Optional: status
// (default Value; carries a verbatim WaslTargetStatus).
// ---------------------------------------------------------------------------

// A non-trivial nested status: a serialized AwaitingGate (the externally-tagged
// serde shape a WaslTargetStatus produces) — proves the Value survives a nested
// object, not just a flat one. Proto treats it opaquely.
const WASL_SESSION_STATUS: &str = r#"{"type":"WaslSessionStatus","session_id":"s-1","status":{"AwaitingGate":{"active_turn":"t-9","detail":{"gate_id":"ng-1","summary":"run rm -rf /tmp/x","preview":"$ rm -rf /tmp/x","tool":"Bash"},"since":"2026-07-02T12:00:00.000Z","queued":0}}}"#;

fn wasl_session_status() -> IpcMsg {
    IpcMsg::WaslSessionStatus {
        session_id: "s-1".to_string(),
        status: serde_json::json!({
            "AwaitingGate": {
                "active_turn": "t-9",
                "detail": {
                    "gate_id": "ng-1",
                    "summary": "run rm -rf /tmp/x",
                    "preview": "$ rm -rf /tmp/x",
                    "tool": "Bash",
                },
                "since": "2026-07-02T12:00:00.000Z",
                "queued": 0,
            },
        }),
    }
}

#[test]
fn wasl_session_status_golden_contract() {
    assert_variant_contract(WASL_SESSION_STATUS, &wasl_session_status());
}

/// §7: `status` is `serde_json::Value` ON PURPOSE (typed form
/// `WaslTargetStatus` lives in marsad-wasl-session). Round-trip a nested shape.
#[test]
fn wasl_session_status_status_field_is_opaque_nested_value() {
    match wasl_session_status() {
        IpcMsg::WaslSessionStatus { status, .. } => {
            assert!(status.is_object(), "status must be a JSON object Value");
            assert_eq!(
                status["AwaitingGate"]["detail"]["gate_id"],
                serde_json::json!("ng-1"),
                "deeply-nested status fields must survive without loss"
            );
        }
        other => panic!("expected WaslSessionStatus, got {other:?}"),
    }
}

#[test]
fn wasl_session_status_absent_status_parses_to_null_value() {
    let line = r#"{"type":"WaslSessionStatus","session_id":"s-1"}"#;
    let parsed = parse_ipc(line).expect("status must be parse-tolerant (default Value::Null)");
    match parsed {
        IpcMsg::WaslSessionStatus { status, .. } => {
            assert_eq!(status, serde_json::Value::Null);
        }
        other => panic!("expected WaslSessionStatus, got {other:?}"),
    }
}

#[test]
fn wasl_session_status_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslSessionStatus","status":{}}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// WaslSessionEvent — §7. Identity: session_id (required). `seq` required (no
// #[serde(default)] in §7). Optional: event (default Value; verbatim
// WaslSessionEvent kind).
// ---------------------------------------------------------------------------

// A non-trivial nested event: a serialized NativeGateRequested kind (the
// externally-tagged serde shape a WaslSessionEventKind produces). Proto opaque.
const WASL_SESSION_EVENT: &str = r#"{"type":"WaslSessionEvent","session_id":"s-1","seq":7,"event":{"NativeGateRequested":{"detail":{"gate_id":"ng-1","summary":"run rm -rf /tmp/x","preview":"$ rm -rf /tmp/x","tool":"Bash"}}}}"#;

fn wasl_session_event() -> IpcMsg {
    IpcMsg::WaslSessionEvent {
        session_id: "s-1".to_string(),
        seq: 7,
        event: serde_json::json!({
            "NativeGateRequested": {
                "detail": {
                    "gate_id": "ng-1",
                    "summary": "run rm -rf /tmp/x",
                    "preview": "$ rm -rf /tmp/x",
                    "tool": "Bash",
                },
            },
        }),
    }
}

#[test]
fn wasl_session_event_golden_contract() {
    assert_variant_contract(WASL_SESSION_EVENT, &wasl_session_event());
}

/// §7: `event` is `serde_json::Value` ON PURPOSE. Round-trip a nested shape.
#[test]
fn wasl_session_event_event_field_is_opaque_nested_value() {
    match wasl_session_event() {
        IpcMsg::WaslSessionEvent { event, seq, .. } => {
            assert_eq!(seq, 7);
            assert!(event.is_object(), "event must be a JSON object Value");
            assert_eq!(
                event["NativeGateRequested"]["detail"]["gate_id"],
                serde_json::json!("ng-1"),
                "deeply-nested event fields must survive without loss"
            );
        }
        other => panic!("expected WaslSessionEvent, got {other:?}"),
    }
}

/// `event` (the only §7-`#[serde(default)]` non-identity field) absent →
/// Value::Null. `seq` is present because §7 declares no default for it (see the
/// missing-`seq` test below).
#[test]
fn wasl_session_event_absent_event_parses_to_null_value() {
    let line = r#"{"type":"WaslSessionEvent","session_id":"s-1","seq":3}"#;
    let parsed = parse_ipc(line).expect("event must be parse-tolerant (default Value::Null)");
    match parsed {
        IpcMsg::WaslSessionEvent { event, seq, .. } => {
            assert_eq!(seq, 3);
            assert_eq!(event, serde_json::Value::Null);
        }
        other => panic!("expected WaslSessionEvent, got {other:?}"),
    }
}

#[test]
fn wasl_session_event_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslSessionEvent","seq":1,"event":{}}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

/// ASSUMPTION (flagged in report): §7's `WaslSessionEvent` declares `seq`
/// WITHOUT `#[serde(default)]` — i.e. required. `seq` is load-bearing (it is the
/// replay/dedup key, §6.1/§10.4); an event with no sequence cannot be folded
/// gap/dup-free, so a missing `seq` is a SchemaMismatch, not a silent 0.
#[test]
fn wasl_session_event_missing_seq_is_schema_mismatch() {
    let line = r#"{"type":"WaslSessionEvent","session_id":"s-1","event":{}}"#;
    let err = parse_ipc(line).expect_err("missing seq must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// WaslPromptReceipt — §7. Identity: session_id (required). Optional: receipt
// (default Value; verbatim WaslTurnReceipt).
// ---------------------------------------------------------------------------

// A non-trivial nested receipt: a serialized RejectedUnavailable{status} — the
// receipt variant that itself carries a nested WaslTargetStatus, so the payload
// is deeply nested. Proto opaque.
const WASL_PROMPT_RECEIPT: &str = r#"{"type":"WaslPromptReceipt","session_id":"s-1","receipt":{"RejectedUnavailable":{"status":{"Faulted":{"error":"process exited","recoverable":false}}}}}"#;

fn wasl_prompt_receipt() -> IpcMsg {
    IpcMsg::WaslPromptReceipt {
        session_id: "s-1".to_string(),
        receipt: serde_json::json!({
            "RejectedUnavailable": {
                "status": {
                    "Faulted": {"error": "process exited", "recoverable": false},
                },
            },
        }),
    }
}

#[test]
fn wasl_prompt_receipt_golden_contract() {
    assert_variant_contract(WASL_PROMPT_RECEIPT, &wasl_prompt_receipt());
}

/// §7: `receipt` is `serde_json::Value` ON PURPOSE. Round-trip a nested shape.
#[test]
fn wasl_prompt_receipt_receipt_field_is_opaque_nested_value() {
    match wasl_prompt_receipt() {
        IpcMsg::WaslPromptReceipt { receipt, .. } => {
            assert!(receipt.is_object(), "receipt must be a JSON object Value");
            assert_eq!(
                receipt["RejectedUnavailable"]["status"]["Faulted"]["recoverable"],
                serde_json::json!(false),
                "deeply-nested receipt fields must survive without loss"
            );
        }
        other => panic!("expected WaslPromptReceipt, got {other:?}"),
    }
}

#[test]
fn wasl_prompt_receipt_absent_receipt_parses_to_null_value() {
    let line = r#"{"type":"WaslPromptReceipt","session_id":"s-1"}"#;
    let parsed = parse_ipc(line).expect("receipt must be parse-tolerant (default Value::Null)");
    match parsed {
        IpcMsg::WaslPromptReceipt { receipt, .. } => {
            assert_eq!(receipt, serde_json::Value::Null);
        }
        other => panic!("expected WaslPromptReceipt, got {other:?}"),
    }
}

#[test]
fn wasl_prompt_receipt_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslPromptReceipt","receipt":{}}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// WaslNativeGateAnswered — §7. Identity: session_id, gate_id (required).
// ---------------------------------------------------------------------------

const WASL_NATIVE_GATE_ANSWERED: &str =
    r#"{"type":"WaslNativeGateAnswered","session_id":"s-1","gate_id":"ng-1"}"#;

fn wasl_native_gate_answered() -> IpcMsg {
    IpcMsg::WaslNativeGateAnswered {
        session_id: "s-1".to_string(),
        gate_id: "ng-1".to_string(),
    }
}

#[test]
fn wasl_native_gate_answered_golden_contract() {
    assert_variant_contract(WASL_NATIVE_GATE_ANSWERED, &wasl_native_gate_answered());
}

#[test]
fn wasl_native_gate_answered_missing_session_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslNativeGateAnswered","gate_id":"ng-1"}"#;
    let err = parse_ipc(line).expect_err("missing session_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn wasl_native_gate_answered_missing_gate_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslNativeGateAnswered","session_id":"s-1"}"#;
    let err = parse_ipc(line).expect_err("missing gate_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ===========================================================================
// Table-driven sweep over all 10 new §7 variants (13 golden lines: two of the
// variants — AnswerWaslNativeGate — contribute an approve + a deny form).
// ===========================================================================

fn all_new_goldens() -> Vec<(&'static str, &'static str, IpcMsg)> {
    vec![
        (
            "LaunchWaslSession",
            LAUNCH_WASL_SESSION,
            launch_wasl_session(),
        ),
        ("SendWaslPrompt", SEND_WASL_PROMPT, send_wasl_prompt()),
        (
            "AttachWaslSession",
            ATTACH_WASL_SESSION,
            attach_wasl_session(),
        ),
        (
            "AnswerWaslNativeGate (approved)",
            ANSWER_WASL_NATIVE_GATE_APPROVED,
            answer_wasl_native_gate_approved(),
        ),
        (
            "AnswerWaslNativeGate (denied)",
            ANSWER_WASL_NATIVE_GATE_DENIED,
            answer_wasl_native_gate_denied(),
        ),
        (
            "InterruptWaslSession",
            INTERRUPT_WASL_SESSION,
            interrupt_wasl_session(),
        ),
        ("StopWaslSession", STOP_WASL_SESSION, stop_wasl_session()),
        (
            "WaslSessionStatus",
            WASL_SESSION_STATUS,
            wasl_session_status(),
        ),
        ("WaslSessionEvent", WASL_SESSION_EVENT, wasl_session_event()),
        (
            "WaslPromptReceipt",
            WASL_PROMPT_RECEIPT,
            wasl_prompt_receipt(),
        ),
        (
            "WaslNativeGateAnswered",
            WASL_NATIVE_GATE_ANSWERED,
            wasl_native_gate_answered(),
        ),
    ]
}

#[test]
fn all_new_goldens_table_covers_all_expected_lines() {
    assert_eq!(
        all_new_goldens().len(),
        11,
        "§7 lists 10 new tags; AnswerWaslNativeGate contributes 2 golden forms \
         (approve + deny), so the table has 11 golden lines total"
    );
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
fn every_new_golden_round_trips_encode_parse_identity() {
    for (name, _golden, expected) in &all_new_goldens() {
        let encoded = encode_ipc(expected);
        let reparsed = parse_ipc(&encoded)
            .unwrap_or_else(|e| panic!("{name}: self-encoded must reparse: {e:?}"));
        assert_eq!(&reparsed, expected, "{name}: encode->parse identity failed");
        assert_eq!(
            encode_ipc(&reparsed),
            encoded,
            "{name}: re-encode must be stable"
        );
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

// ===========================================================================
// KNOWN closure (§7: "parse_ipc's KNOWN list grows by exactly these tags") —
// every one of the 10 new tags must be recognized (no longer
// UnknownDiscriminant), and a genuinely fake future tag must STILL cleanly hit
// the UnknownDiscriminant path (the core tolerant-skip contract; §7's own
// backward-compat note that an old build gracefully no-ops an unknown *Wasl*
// command it never provoked).
// ===========================================================================

/// The 10 new backend tags, exactly as §7 names them.
const NEW_WASL_SESSION_TAGS: &[&str] = &[
    "LaunchWaslSession",
    "SendWaslPrompt",
    "AttachWaslSession",
    "AnswerWaslNativeGate",
    "InterruptWaslSession",
    "StopWaslSession",
    "WaslSessionStatus",
    "WaslSessionEvent",
    "WaslPromptReceipt",
    "WaslNativeGateAnswered",
];

#[test]
fn new_wasl_session_tags_are_no_longer_unknown_discriminants() {
    for tag in NEW_WASL_SESSION_TAGS {
        let line = format!(r#"{{"type":"{tag}"}}"#);
        let err = parse_ipc(&line);
        // Each of these tags has at least one required identity field
        // (session_id, and host_id/gate_id for some), so a bare
        // `{"type":"<tag>"}` is a SchemaMismatch, NOT an UnknownDiscriminant —
        // either way the discriminant itself MUST be recognized.
        if let Err(ProtoError::UnknownDiscriminant(t)) = err {
            panic!("tag {tag:?} must be known to parse_ipc, got UnknownDiscriminant({t:?})");
        }
    }
}

#[test]
fn new_wasl_session_tags_list_covers_exactly_ten() {
    assert_eq!(
        NEW_WASL_SESSION_TAGS.len(),
        10,
        "§7 adds exactly 10 new IpcMsg variants to the Wasl-backend family"
    );
}

/// A deliberately fake future Wasl-backend tag must STILL be
/// `UnknownDiscriminant` (proving backward-compat / the tolerant-skip path is
/// preserved — a future variant this build doesn't know is not a hard error at
/// the discriminant level, exactly as §7 requires).
#[test]
fn fake_future_wasl_session_tag_is_still_unknown_discriminant() {
    let line = r#"{"type":"FutureWaslSessionMsg","session_id":"s-1"}"#;
    let err = parse_ipc(line).expect_err("unrecognized IPC type must error");
    match err {
        ProtoError::UnknownDiscriminant(s) => assert_eq!(s, "FutureWaslSessionMsg"),
        other => panic!("expected UnknownDiscriminant, got {other:?}"),
    }
}

// ===========================================================================
// §7 v2 drift guard: "round-trip typed-conversion tests at the crate boundary"
// for the Value-carried payloads (wasl_target_key / status / event / receipt).
//
// marsad-proto MUST NOT depend on marsad-wasl-session, so these tests cannot
// import the typed forms. Instead they assert the crossing-the-wire invariant
// proto is responsible for: an arbitrary NESTED JSON payload placed into a
// Value-carried field survives `construct -> encode_ipc -> parse_ipc` byte-for-
// byte (the "value -> Value -> value == value" identity, the marsad-proto half
// of the typed<->Value round-trip the design's §7/§10.2 pins). The typed<->Value
// half (WaslTargetStatus -> Value -> WaslTargetStatus) lives in the
// marsad-wasl-session tests, which CAN name the typed forms.
// ===========================================================================

/// A single deeply-nested payload exercised through every Value-carried field:
/// mixed objects, arrays, numbers, bools, nulls, and empty containers — the
/// "non-trivial nested object, not just a flat one" the brief requires.
fn gnarly_nested_payload() -> serde_json::Value {
    serde_json::json!({
        "tag": "SomeVariant",
        "fields": {
            "n": 42,
            "f": 3.5,
            "flag": true,
            "none": serde_json::Value::Null,
            "list": [1, "two", {"three": 3}, [4, 5]],
            "empty_obj": {},
            "empty_arr": [],
            "deep": {"a": {"b": {"c": {"d": "leaf"}}}},
        },
    })
}

#[test]
fn wasl_target_key_value_payload_round_trips_without_loss() {
    let payload = gnarly_nested_payload();
    let msg = IpcMsg::LaunchWaslSession {
        session_id: "s-1".to_string(),
        host_id: "local".to_string(),
        wasl_target_key: payload.clone(),
        name: None,
    };
    let reparsed = parse_ipc(&encode_ipc(&msg)).expect("must reparse");
    match reparsed {
        IpcMsg::LaunchWaslSession {
            wasl_target_key, ..
        } => assert_eq!(
            wasl_target_key, payload,
            "wasl_target_key nested payload must survive value->Value->value with no loss"
        ),
        other => panic!("expected LaunchWaslSession, got {other:?}"),
    }
}

#[test]
fn status_value_payload_round_trips_without_loss() {
    let payload = gnarly_nested_payload();
    let msg = IpcMsg::WaslSessionStatus {
        session_id: "s-1".to_string(),
        status: payload.clone(),
    };
    let reparsed = parse_ipc(&encode_ipc(&msg)).expect("must reparse");
    match reparsed {
        IpcMsg::WaslSessionStatus { status, .. } => assert_eq!(
            status, payload,
            "status nested payload must survive value->Value->value with no loss"
        ),
        other => panic!("expected WaslSessionStatus, got {other:?}"),
    }
}

#[test]
fn event_value_payload_round_trips_without_loss() {
    let payload = gnarly_nested_payload();
    let msg = IpcMsg::WaslSessionEvent {
        session_id: "s-1".to_string(),
        seq: 99,
        event: payload.clone(),
    };
    let reparsed = parse_ipc(&encode_ipc(&msg)).expect("must reparse");
    match reparsed {
        IpcMsg::WaslSessionEvent { event, seq, .. } => {
            assert_eq!(seq, 99, "seq must survive alongside the Value payload");
            assert_eq!(
                event, payload,
                "event nested payload must survive value->Value->value with no loss"
            );
        }
        other => panic!("expected WaslSessionEvent, got {other:?}"),
    }
}

#[test]
fn receipt_value_payload_round_trips_without_loss() {
    let payload = gnarly_nested_payload();
    let msg = IpcMsg::WaslPromptReceipt {
        session_id: "s-1".to_string(),
        receipt: payload.clone(),
    };
    let reparsed = parse_ipc(&encode_ipc(&msg)).expect("must reparse");
    match reparsed {
        IpcMsg::WaslPromptReceipt { receipt, .. } => assert_eq!(
            receipt, payload,
            "receipt nested payload must survive value->Value->value with no loss"
        ),
        other => panic!("expected WaslPromptReceipt, got {other:?}"),
    }
}
