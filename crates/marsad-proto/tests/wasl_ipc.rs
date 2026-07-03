//! wasl_ipc.rs — golden-JSON regression suite for the X1 Wasl-connector IPC
//! surface (`docs/specs/wasl-connector.md` §3, "B01 marsad-proto — additive
//! `IpcMsg` variants").
//!
//! Authoritative spec: §3 (the 5 new variants + their normative golden wire
//! forms, values lifted verbatim from the vendored operator-v1 fixtures) and
//! §7.2 ("marsad-proto — NEW tests/wasl_ipc.rs"). This file is the additive
//! companion to `tests/ipc_typed.rs` (M2, FROZEN) and
//! `tests/capabilities_ipc.rs` (U1, FROZEN) — it does not touch or duplicate
//! either file's golden tables; it follows the exact same per-variant
//! contract convention (golden parse / encode-parse identity / re-encode
//! stability / golden-encode-as-Value / unknown-field tolerance) for the 5
//! NEW Wasl tags, plus the KNOWN-closure and required-identity-field
//! (`host_id`/`gate_id`) schema-error tests §7.2 calls out explicitly.
//!
//! This file WILL fail to compile until the code-agent lands the 5 new
//! `IpcMsg` variants (`AnswerWaslGate`, `WaslGateRequested`, `WaslGateDecided`,
//! `WaslObservation`, `WaslGateAnswered`) and grows `parse_ipc`'s `KNOWN` list
//! from 29 to 34 tags (§3). That is the expected tests-first compile-failure
//! signal.

use marsad_proto::{encode_ipc, parse_ipc, IpcMsg, ProtoError};

// ---------------------------------------------------------------------------
// Shared helpers (mirrors tests/ipc_typed.rs's and tests/capabilities_ipc.rs's
// `val`/`inject_future_field`/`assert_variant_contract` exactly — duplicated
// here because each file in tests/ is compiled as an independent crate).
// ---------------------------------------------------------------------------

fn val(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("golden literal must itself be valid JSON")
}

/// Inject an unknown field into a golden JSON object literal, mimicking a
/// newer sender adding an additive field this build doesn't know about yet
/// (§3: "`#[serde(default)]` parse tolerance on every non-identity field").
fn inject_future_field(golden: &str) -> String {
    let mut v = val(golden);
    v.as_object_mut()
        .expect("golden must be a JSON object")
        .insert("x_future_field".to_string(), serde_json::json!(123));
    v.to_string()
}

/// The full per-variant contract (typed-ipc.md §6 items 1-4, applied to the 5
/// new Wasl tags): golden parse, encode->parse identity + re-encode
/// stability, golden encode as `Value`, and unknown-field tolerance.
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
// AnswerWaslGate — §3 golden forms (approve / deny)
// ---------------------------------------------------------------------------

const ANSWER_WASL_GATE_APPROVE: &str = r#"{"type":"AnswerWaslGate","host_id":"local","gate_id":"gate-1","outcome":"approved","reason":null}"#;

const ANSWER_WASL_GATE_DENY: &str = r#"{"type":"AnswerWaslGate","host_id":"local","gate_id":"gate-1","outcome":"denied","reason":"not during business hours"}"#;

fn answer_wasl_gate_approve() -> IpcMsg {
    IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "gate-1".to_string(),
        outcome: "approved".to_string(),
        reason: None,
    }
}

fn answer_wasl_gate_deny() -> IpcMsg {
    IpcMsg::AnswerWaslGate {
        host_id: "local".to_string(),
        gate_id: "gate-1".to_string(),
        outcome: "denied".to_string(),
        reason: Some("not during business hours".to_string()),
    }
}

#[test]
fn answer_wasl_gate_approve_golden_contract() {
    assert_variant_contract(ANSWER_WASL_GATE_APPROVE, &answer_wasl_gate_approve());
}

#[test]
fn answer_wasl_gate_deny_golden_contract() {
    assert_variant_contract(ANSWER_WASL_GATE_DENY, &answer_wasl_gate_deny());
}

/// §3: "reason: ALWAYS serialized (null when absent)" — like
/// `LaunchSession.first_prompt`. Pin the re-encode explicitly, not just via
/// the golden-Value-equality check above.
#[test]
fn answer_wasl_gate_reason_none_reencodes_as_literal_null_key_present() {
    let encoded = encode_ipc(&answer_wasl_gate_approve());
    let obj = val(&encoded);
    let obj = obj.as_object().expect("must be a JSON object");
    assert_eq!(
        obj.get("reason"),
        Some(&serde_json::Value::Null),
        "reason must ALWAYS be serialized (null when absent), got: {encoded}"
    );
}

// ---------------------------------------------------------------------------
// WaslGateRequested — §3 golden form
// ---------------------------------------------------------------------------

const WASL_GATE_REQUESTED: &str = r#"{"type":"WaslGateRequested","host_id":"local","gate_id":"gate-1","mode":"external","decidable":true,"request":{"id":"call-9","kind":"message","from":"wasl-a","summary":"message to 1 peer(s)","preview":"hi bob"},"requested_at":"2026-07-02T12:00:00.000Z","timeout_ms":120000,"deadline":"2026-07-02T12:02:00.000Z"}"#;

fn wasl_gate_requested() -> IpcMsg {
    IpcMsg::WaslGateRequested {
        host_id: "local".to_string(),
        gate_id: "gate-1".to_string(),
        mode: "external".to_string(),
        decidable: true,
        request: serde_json::json!({
            "id": "call-9",
            "kind": "message",
            "from": "wasl-a",
            "summary": "message to 1 peer(s)",
            "preview": "hi bob",
        }),
        requested_at: "2026-07-02T12:00:00.000Z".to_string(),
        timeout_ms: 120000,
        deadline: "2026-07-02T12:02:00.000Z".to_string(),
    }
}

#[test]
fn wasl_gate_requested_golden_contract() {
    assert_variant_contract(WASL_GATE_REQUESTED, &wasl_gate_requested());
}

/// §3: "request" is `serde_json::Value` ON PURPOSE (typed form is
/// `marsad_wasl::GateRequest`; proto must not depend on wasl, DAG) — assert
/// it round-trips as an opaque object, not a typed struct.
#[test]
fn wasl_gate_requested_request_field_is_opaque_value() {
    match wasl_gate_requested() {
        IpcMsg::WaslGateRequested { request, .. } => {
            assert!(request.is_object(), "request must be a JSON object Value");
            assert_eq!(request["id"], serde_json::json!("call-9"));
        }
        other => panic!("expected WaslGateRequested, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WaslGateDecided — §3 golden forms (approved / denied / synthesized unknown)
// ---------------------------------------------------------------------------

const WASL_GATE_DECIDED_APPROVED: &str = r#"{"type":"WaslGateDecided","host_id":"local","gate_id":"gate-1","outcome":"approved","decided_by":"operator","reason":null,"decided_at":"2026-07-02T12:00:03.500Z"}"#;

const WASL_GATE_DECIDED_DENIED: &str = r#"{"type":"WaslGateDecided","host_id":"local","gate_id":"gate-1","outcome":"denied","decided_by":"operator","reason":"not during business hours","decided_at":"2026-07-02T12:00:03.500Z"}"#;

const WASL_GATE_DECIDED_UNKNOWN_RESYNC: &str = r#"{"type":"WaslGateDecided","host_id":"local","gate_id":"gate-7","outcome":"unknown","decided_by":"resync","reason":null,"decided_at":""}"#;

fn wasl_gate_decided_approved() -> IpcMsg {
    IpcMsg::WaslGateDecided {
        host_id: "local".to_string(),
        gate_id: "gate-1".to_string(),
        outcome: "approved".to_string(),
        decided_by: "operator".to_string(),
        reason: None,
        decided_at: "2026-07-02T12:00:03.500Z".to_string(),
    }
}

fn wasl_gate_decided_denied() -> IpcMsg {
    IpcMsg::WaslGateDecided {
        host_id: "local".to_string(),
        gate_id: "gate-1".to_string(),
        outcome: "denied".to_string(),
        decided_by: "operator".to_string(),
        reason: Some("not during business hours".to_string()),
        decided_at: "2026-07-02T12:00:03.500Z".to_string(),
    }
}

/// §3: `outcome:"unknown"` / `decided_by:"resync"` / `decided_at:""` —
/// connector-synthesized `GateVanished` resolution (Marsad-wire-only value;
/// never sent to Wasl itself).
fn wasl_gate_decided_unknown_resync() -> IpcMsg {
    IpcMsg::WaslGateDecided {
        host_id: "local".to_string(),
        gate_id: "gate-7".to_string(),
        outcome: "unknown".to_string(),
        decided_by: "resync".to_string(),
        reason: None,
        decided_at: String::new(),
    }
}

#[test]
fn wasl_gate_decided_approved_golden_contract() {
    assert_variant_contract(WASL_GATE_DECIDED_APPROVED, &wasl_gate_decided_approved());
}

#[test]
fn wasl_gate_decided_denied_golden_contract() {
    assert_variant_contract(WASL_GATE_DECIDED_DENIED, &wasl_gate_decided_denied());
}

/// §3/§7.2: "the outcome:"unknown"/decided_by:"resync"/decided_at:"" synthesized
/// golden round-trips."
#[test]
fn wasl_gate_decided_unknown_resync_synthesized_golden_contract() {
    assert_variant_contract(
        WASL_GATE_DECIDED_UNKNOWN_RESYNC,
        &wasl_gate_decided_unknown_resync(),
    );
}

// ---------------------------------------------------------------------------
// WaslObservation — §3 golden form
// ---------------------------------------------------------------------------

const WASL_OBSERVATION: &str = r#"{"type":"WaslObservation","host_id":"local","entry":{"seq":3,"kind":"decision","actor":"wasl-a","targets":["wasl-b"],"summary":"gate Message","decision":{"decision":"approve"}}}"#;

fn wasl_observation() -> IpcMsg {
    IpcMsg::WaslObservation {
        host_id: "local".to_string(),
        entry: serde_json::json!({
            "seq": 3,
            "kind": "decision",
            "actor": "wasl-a",
            "targets": ["wasl-b"],
            "summary": "gate Message",
            "decision": {"decision": "approve"},
        }),
    }
}

#[test]
fn wasl_observation_golden_contract() {
    assert_variant_contract(WASL_OBSERVATION, &wasl_observation());
}

/// §3: "entry" is `serde_json::Value` ON PURPOSE (same rationale as `request`
/// — typed form is `marsad_wasl::AuditEntry`, proto must not depend on wasl).
#[test]
fn wasl_observation_entry_field_is_opaque_value() {
    match wasl_observation() {
        IpcMsg::WaslObservation { entry, .. } => {
            assert!(entry.is_object(), "entry must be a JSON object Value");
            assert_eq!(entry["kind"], serde_json::json!("decision"));
        }
        other => panic!("expected WaslObservation, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WaslGateAnswered — §3 golden form
// ---------------------------------------------------------------------------

const WASL_GATE_ANSWERED: &str =
    r#"{"type":"WaslGateAnswered","host_id":"local","gate_id":"gate-1"}"#;

fn wasl_gate_answered() -> IpcMsg {
    IpcMsg::WaslGateAnswered {
        host_id: "local".to_string(),
        gate_id: "gate-1".to_string(),
    }
}

#[test]
fn wasl_gate_answered_golden_contract() {
    assert_variant_contract(WASL_GATE_ANSWERED, &wasl_gate_answered());
}

// ---------------------------------------------------------------------------
// Table-driven sweep over the 8 new §3 golden lines (5 tags; AnswerWaslGate
// and WaslGateDecided each contribute more than one golden form).
// ---------------------------------------------------------------------------

fn all_new_goldens() -> Vec<(&'static str, &'static str, IpcMsg)> {
    vec![
        (
            "AnswerWaslGate (approve)",
            ANSWER_WASL_GATE_APPROVE,
            answer_wasl_gate_approve(),
        ),
        (
            "AnswerWaslGate (deny)",
            ANSWER_WASL_GATE_DENY,
            answer_wasl_gate_deny(),
        ),
        (
            "WaslGateRequested",
            WASL_GATE_REQUESTED,
            wasl_gate_requested(),
        ),
        (
            "WaslGateDecided (approved)",
            WASL_GATE_DECIDED_APPROVED,
            wasl_gate_decided_approved(),
        ),
        (
            "WaslGateDecided (denied)",
            WASL_GATE_DECIDED_DENIED,
            wasl_gate_decided_denied(),
        ),
        (
            "WaslGateDecided (unknown/resync synthesized)",
            WASL_GATE_DECIDED_UNKNOWN_RESYNC,
            wasl_gate_decided_unknown_resync(),
        ),
        ("WaslObservation", WASL_OBSERVATION, wasl_observation()),
        ("WaslGateAnswered", WASL_GATE_ANSWERED, wasl_gate_answered()),
    ]
}

#[test]
fn all_new_goldens_table_covers_all_8_lines() {
    assert_eq!(
        all_new_goldens().len(),
        8,
        "§3 lists exactly 8 golden wire lines across the 5 new tags \
         (2 AnswerWaslGate forms + 1 WaslGateRequested + 3 WaslGateDecided \
         forms + 1 WaslObservation + 1 WaslGateAnswered)"
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

// ---------------------------------------------------------------------------
// KNOWN closure (§3: "parse_ipc's KNOWN list grows by exactly these 5 tags
// (29 -> 34)") — the 5 new tags must no longer be UnknownDiscriminant, and a
// genuinely unrecognized tag must still be.
// ---------------------------------------------------------------------------

#[test]
fn new_wasl_tags_are_no_longer_unknown_discriminants() {
    for tag in [
        "AnswerWaslGate",
        "WaslGateRequested",
        "WaslGateDecided",
        "WaslObservation",
        "WaslGateAnswered",
    ] {
        let line = format!(r#"{{"type":"{tag}"}}"#);
        let err = parse_ipc(&line);
        // Every one of these tags has required identity fields (host_id, and
        // gate_id for 4 of the 5), so a bare `{"type":"<tag>"}` is a
        // SchemaMismatch, NOT an UnknownDiscriminant — either way, the
        // discriminant itself must be recognized (closing the compat gap an
        // old build would hit per §3's backward-compat note).
        if let Err(ProtoError::UnknownDiscriminant(t)) = err {
            panic!("tag {tag:?} must be known to parse_ipc, got UnknownDiscriminant({t:?})");
        }
    }
}

/// A type string that is neither one of the original 29 tags nor one of
/// these 5 new ones must still cleanly be `UnknownDiscriminant`, carrying the
/// exact type string (the core tolerant-skip contract this proves stays
/// intact, typed-ipc.md §4 / wasl-connector.md §3's own backward-compat note).
#[test]
fn genuinely_unknown_type_is_still_unknown_discriminant() {
    let line = r#"{"type":"WaslIpcProbeNotARealVariant","host_id":"local"}"#;
    let err = parse_ipc(line).expect_err("unrecognized IPC type must error");
    match err {
        ProtoError::UnknownDiscriminant(s) => assert_eq!(s, "WaslIpcProbeNotARealVariant"),
        other => panic!("expected UnknownDiscriminant, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Required identity fields (§3: "host_id and gate_id are identity fields:
// required on parse (missing -> SchemaMismatch -> Protocol error, same class
// as session_id on existing variants)")
// ---------------------------------------------------------------------------

#[test]
fn answer_wasl_gate_missing_host_id_is_schema_mismatch() {
    let line = r#"{"type":"AnswerWaslGate","gate_id":"gate-1","outcome":"approved"}"#;
    let err = parse_ipc(line).expect_err("missing host_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn answer_wasl_gate_missing_gate_id_is_schema_mismatch() {
    let line = r#"{"type":"AnswerWaslGate","host_id":"local","outcome":"approved"}"#;
    let err = parse_ipc(line).expect_err("missing gate_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn wasl_gate_requested_missing_host_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslGateRequested","gate_id":"gate-1"}"#;
    let err = parse_ipc(line).expect_err("missing host_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn wasl_gate_requested_missing_gate_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslGateRequested","host_id":"local"}"#;
    let err = parse_ipc(line).expect_err("missing gate_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn wasl_gate_decided_missing_host_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslGateDecided","gate_id":"gate-1","outcome":"approved"}"#;
    let err = parse_ipc(line).expect_err("missing host_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn wasl_gate_decided_missing_gate_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslGateDecided","host_id":"local","outcome":"approved"}"#;
    let err = parse_ipc(line).expect_err("missing gate_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn wasl_observation_missing_host_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslObservation","entry":{}}"#;
    let err = parse_ipc(line).expect_err("missing host_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn wasl_gate_answered_missing_host_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslGateAnswered","gate_id":"gate-1"}"#;
    let err = parse_ipc(line).expect_err("missing host_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

#[test]
fn wasl_gate_answered_missing_gate_id_is_schema_mismatch() {
    let line = r#"{"type":"WaslGateAnswered","host_id":"local"}"#;
    let err = parse_ipc(line).expect_err("missing gate_id must fail schema validation");
    assert!(
        matches!(err, ProtoError::SchemaMismatch { .. }),
        "expected SchemaMismatch, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Defaults for non-identity fields (§3: "#[serde(default)] parse tolerance
// on every non-identity field")
// ---------------------------------------------------------------------------

#[test]
fn wasl_gate_requested_absent_optionals_parse_to_documented_defaults() {
    let line = r#"{"type":"WaslGateRequested","host_id":"local","gate_id":"gate-1"}"#;
    let parsed = parse_ipc(line).expect("non-identity fields must all be parse-tolerant");
    match parsed {
        IpcMsg::WaslGateRequested {
            mode,
            decidable,
            request,
            requested_at,
            timeout_ms,
            deadline,
            ..
        } => {
            assert_eq!(mode, "");
            assert!(!decidable);
            assert_eq!(request, serde_json::Value::Null);
            assert_eq!(requested_at, "");
            assert_eq!(timeout_ms, 0);
            assert_eq!(deadline, "");
        }
        other => panic!("expected WaslGateRequested, got {other:?}"),
    }
}

#[test]
fn wasl_gate_decided_absent_optionals_parse_to_documented_defaults() {
    let line = r#"{"type":"WaslGateDecided","host_id":"local","gate_id":"gate-1"}"#;
    let parsed = parse_ipc(line).expect("non-identity fields must all be parse-tolerant");
    match parsed {
        IpcMsg::WaslGateDecided {
            outcome,
            decided_by,
            reason,
            decided_at,
            ..
        } => {
            assert_eq!(outcome, "");
            assert_eq!(decided_by, "");
            assert_eq!(reason, None);
            assert_eq!(decided_at, "");
        }
        other => panic!("expected WaslGateDecided, got {other:?}"),
    }
}

#[test]
fn wasl_observation_absent_entry_parses_to_null_value() {
    let line = r#"{"type":"WaslObservation","host_id":"local"}"#;
    let parsed = parse_ipc(line).expect("entry must be parse-tolerant (default Value::Null)");
    match parsed {
        IpcMsg::WaslObservation { entry, .. } => {
            assert_eq!(entry, serde_json::Value::Null);
        }
        other => panic!("expected WaslObservation, got {other:?}"),
    }
}
