//! fixtures.rs — vendored-contract conformance (spec §7.1, first half).
//!
//! Every one of the 16 files in `fixtures/operator-v1/` (FROZEN, never
//! edited by this crate) parses into its typed struct with full field
//! equality — not just "is ok". Also: unknown-field forward-compat tolerance
//! (top-level and nested), the `DecisionRequest` golden encode check, and an
//! inventory pin so a partial/extended vendor fails loudly.
//!
//! This file WILL fail to compile until a later worker implements the
//! `marsad_wasl` types referenced below (§2.1) — that is the expected
//! tests-first signal for this crate-bucket block.
use marsad_wasl::{
    AuditDecision, AuditEntry, AuditFrame, DecisionRequest, ErrorBody, GateDecided, GateRequest,
    GateRequested, GatesSnapshot, Hello, OperatorInfo, PendingGate,
};

// ---------------------------------------------------------------------------
// Path / IO helpers — mirrors crates/marsad-proto/tests/fixtures.rs style.
// ---------------------------------------------------------------------------

fn fixture_path(filename: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/operator-v1")
        .join(filename)
}

fn read_fixture(filename: &str) -> String {
    let path = fixture_path(filename);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {path:?}: {e}"))
}

fn val(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("must be valid JSON")
}

/// Re-parse `raw` with `"x_future_field": 123` injected at the top level and
/// assert the typed result is unchanged — forward-compat tolerance (§2.1: "no
/// deny_unknown_fields anywhere").
fn assert_tolerant_top_level<T>(raw: &str)
where
    T: serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let original: T = serde_json::from_str(raw).expect("fixture must parse");
    let mut v = val(raw);
    v.as_object_mut()
        .expect("fixture must be a JSON object")
        .insert("x_future_field".to_string(), serde_json::json!(123));
    let mutated: T = serde_json::from_value(v).expect("must tolerate unknown top-level field");
    assert_eq!(
        original, mutated,
        "unknown top-level field changed the parsed value"
    );
}

/// Same as `assert_tolerant_top_level` but injects into a nested object field
/// (`request` or `entry`) rather than the top level.
fn assert_tolerant_nested<T>(raw: &str, nested_key: &str)
where
    T: serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let original: T = serde_json::from_str(raw).expect("fixture must parse");
    let mut v = val(raw);
    v.as_object_mut()
        .expect("fixture must be a JSON object")
        .get_mut(nested_key)
        .unwrap_or_else(|| panic!("fixture must have a `{nested_key}` field"))
        .as_object_mut()
        .unwrap_or_else(|| panic!("`{nested_key}` must be a JSON object"))
        .insert("x_future_field".to_string(), serde_json::json!(123));
    let mutated: T = serde_json::from_value(v).expect("must tolerate unknown nested field");
    assert_eq!(
        original, mutated,
        "unknown nested field changed the parsed value"
    );
}

// ---------------------------------------------------------------------------
// 1. info.json → OperatorInfo
// ---------------------------------------------------------------------------

#[test]
fn info_json_parses() {
    let raw = read_fixture("info.json");
    let parsed: OperatorInfo = serde_json::from_str(&raw).expect("info.json must parse");
    assert_eq!(parsed.agent_id, "wasl-a");
    assert_eq!(parsed.host_id, "host-1");
    assert_eq!(parsed.version, "0.1.0");
}

#[test]
fn info_json_tolerates_unknown_field() {
    assert_tolerant_top_level::<OperatorInfo>(&read_fixture("info.json"));
}

// ---------------------------------------------------------------------------
// 2. hello.json → Hello
// ---------------------------------------------------------------------------

#[test]
fn hello_json_parses() {
    let raw = read_fixture("hello.json");
    let parsed: Hello = serde_json::from_str(&raw).expect("hello.json must parse");
    assert_eq!(parsed.api_version, 1);
    assert_eq!(parsed.agent_id, "wasl-a");
    assert_eq!(parsed.host_id, "host-1");
    assert_eq!(parsed.version, "0.1.0");
    assert_eq!(parsed.last_seq, 42);
    assert_eq!(parsed.resume, "snapshot");
}

#[test]
fn hello_json_tolerates_unknown_field() {
    assert_tolerant_top_level::<Hello>(&read_fixture("hello.json"));
}

// ---------------------------------------------------------------------------
// 3-4. gate_requested_{external,elicitation}.json → GateRequested
// ---------------------------------------------------------------------------

#[test]
fn gate_requested_external_parses() {
    let raw = read_fixture("gate_requested_external.json");
    let parsed: GateRequested = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        GateRequested {
            gate_id: "gate-1".to_string(),
            mode: "external".to_string(),
            decidable: true,
            request: GateRequest {
                id: "call-9".to_string(),
                kind: "message".to_string(),
                from: "wasl-a".to_string(),
                summary: "message to 1 peer(s)".to_string(),
                preview: "hi bob".to_string(),
            },
            requested_at: "2026-07-02T12:00:00.000Z".to_string(),
            timeout_ms: 120_000,
            deadline: "2026-07-02T12:02:00.000Z".to_string(),
        }
    );
}

#[test]
fn gate_requested_external_tolerates_unknown_fields() {
    let raw = read_fixture("gate_requested_external.json");
    assert_tolerant_top_level::<GateRequested>(&raw);
    assert_tolerant_nested::<GateRequested>(&raw, "request");
}

#[test]
fn gate_requested_elicitation_parses() {
    let raw = read_fixture("gate_requested_elicitation.json");
    let parsed: GateRequested = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(parsed.gate_id, "gate-2");
    assert_eq!(parsed.mode, "elicitation");
    assert!(
        !parsed.decidable,
        "elicitation fixture must be non-decidable"
    );
    assert_eq!(parsed.request.id, "call-10");
    assert_eq!(parsed.request.kind, "consult");
    assert_eq!(parsed.request.from, "wasl-b");
    assert_eq!(parsed.request.summary, "consult 1 peer(s)");
    assert_eq!(parsed.request.preview, "what's the status?");
    assert_eq!(parsed.requested_at, "2026-07-02T12:00:05.000Z");
    assert_eq!(parsed.timeout_ms, 120_000);
    assert_eq!(parsed.deadline, "2026-07-02T12:02:05.000Z");
}

#[test]
fn gate_requested_elicitation_tolerates_unknown_fields() {
    let raw = read_fixture("gate_requested_elicitation.json");
    assert_tolerant_top_level::<GateRequested>(&raw);
    assert_tolerant_nested::<GateRequested>(&raw, "request");
}

// ---------------------------------------------------------------------------
// 5-8. gate_decided_*.json → GateDecided
// ---------------------------------------------------------------------------

#[test]
fn gate_decided_approved_operator_parses() {
    let raw = read_fixture("gate_decided_approved_operator.json");
    let parsed: GateDecided = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        GateDecided {
            gate_id: "gate-1".to_string(),
            outcome: "approved".to_string(),
            decided_by: "operator".to_string(),
            reason: None,
            decided_at: "2026-07-02T12:00:03.500Z".to_string(),
        }
    );
}

#[test]
fn gate_decided_denied_operator_parses() {
    let raw = read_fixture("gate_decided_denied_operator.json");
    let parsed: GateDecided = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        GateDecided {
            gate_id: "gate-1".to_string(),
            outcome: "denied".to_string(),
            decided_by: "operator".to_string(),
            reason: Some("not during business hours".to_string()),
            decided_at: "2026-07-02T12:00:03.500Z".to_string(),
        }
    );
}

#[test]
fn gate_decided_dismissed_timeout_parses() {
    let raw = read_fixture("gate_decided_dismissed_timeout.json");
    let parsed: GateDecided = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(parsed.gate_id, "gate-1");
    assert_eq!(parsed.outcome, "dismissed");
    assert_eq!(parsed.decided_by, "timeout");
    assert_eq!(parsed.reason, None);
    assert_eq!(parsed.decided_at, "2026-07-02T12:02:00.000Z");
}

#[test]
fn gate_decided_approved_elicitation_parses() {
    let raw = read_fixture("gate_decided_approved_elicitation.json");
    let parsed: GateDecided = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        GateDecided {
            gate_id: "gate-2".to_string(),
            outcome: "approved".to_string(),
            decided_by: "elicitation".to_string(),
            reason: None,
            decided_at: "2026-07-02T12:00:06.000Z".to_string(),
        }
    );
}

#[test]
fn gate_decided_fixtures_tolerate_unknown_top_level_field() {
    for name in [
        "gate_decided_approved_operator.json",
        "gate_decided_denied_operator.json",
        "gate_decided_dismissed_timeout.json",
        "gate_decided_approved_elicitation.json",
    ] {
        assert_tolerant_top_level::<GateDecided>(&read_fixture(name));
    }
}

// ---------------------------------------------------------------------------
// 9-10. audit_{decision,message}.json → AuditFrame
// ---------------------------------------------------------------------------

#[test]
fn audit_decision_parses() {
    let raw = read_fixture("audit_decision.json");
    let parsed: AuditFrame = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        AuditFrame {
            entry: AuditEntry {
                seq: 3,
                kind: "decision".to_string(),
                actor: "wasl-a".to_string(),
                targets: vec!["wasl-b".to_string()],
                summary: "gate Message".to_string(),
                decision: Some(AuditDecision {
                    decision: "approve".to_string(),
                    reason: None,
                }),
            }
        }
    );
}

#[test]
fn audit_message_parses() {
    let raw = read_fixture("audit_message.json");
    let parsed: AuditFrame = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(parsed.entry.seq, 1);
    assert_eq!(parsed.entry.kind, "message");
    assert_eq!(parsed.entry.actor, "wasl-a");
    assert_eq!(parsed.entry.targets, vec!["wasl-b".to_string()]);
    assert_eq!(parsed.entry.summary, "message to 1 peer(s)");
    assert_eq!(
        parsed.entry.decision, None,
        "auto-approved send must have no decision"
    );
}

#[test]
fn audit_fixtures_tolerate_unknown_fields() {
    for name in ["audit_decision.json", "audit_message.json"] {
        let raw = read_fixture(name);
        assert_tolerant_top_level::<AuditFrame>(&raw);
        assert_tolerant_nested::<AuditFrame>(&raw, "entry");
    }
}

// ---------------------------------------------------------------------------
// 11. gates_snapshot.json → GatesSnapshot
// ---------------------------------------------------------------------------

#[test]
fn gates_snapshot_parses() {
    let raw = read_fixture("gates_snapshot.json");
    let parsed: GatesSnapshot = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        GatesSnapshot {
            last_seq: 42,
            gates: vec![PendingGate {
                gate_id: "gate-1".to_string(),
                request: GateRequest {
                    id: "call-9".to_string(),
                    kind: "message".to_string(),
                    from: "wasl-a".to_string(),
                    summary: "message to 1 peer(s)".to_string(),
                    preview: "hi bob".to_string(),
                },
                requested_at: "2026-07-02T12:00:00.000Z".to_string(),
                age_ms: 1500,
                deadline: "2026-07-02T12:02:00.000Z".to_string(),
            }],
        }
    );
}

#[test]
fn gates_snapshot_tolerates_unknown_top_level_field() {
    assert_tolerant_top_level::<GatesSnapshot>(&read_fixture("gates_snapshot.json"));
}

#[test]
fn gates_snapshot_tolerates_unknown_field_inside_nested_request() {
    let raw = read_fixture("gates_snapshot.json");
    let original: GatesSnapshot = serde_json::from_str(&raw).expect("must parse");
    let mut v = val(&raw);
    v["gates"][0]["request"]
        .as_object_mut()
        .expect("gates[0].request must be an object")
        .insert("x_future_field".to_string(), serde_json::json!(123));
    let mutated: GatesSnapshot = serde_json::from_value(v).expect("must tolerate unknown field");
    assert_eq!(original, mutated);
}

// ---------------------------------------------------------------------------
// 12-13. decision_request_{approved,denied}.json → DecisionRequest
// ---------------------------------------------------------------------------

#[test]
fn decision_request_approved_parses() {
    let raw = read_fixture("decision_request_approved.json");
    let parsed: DecisionRequest = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        DecisionRequest {
            outcome: "approved".to_string(),
            reason: None
        }
    );
}

#[test]
fn decision_request_denied_parses() {
    let raw = read_fixture("decision_request_denied.json");
    let parsed: DecisionRequest = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        DecisionRequest {
            outcome: "denied".to_string(),
            reason: Some("not during business hours".to_string()),
        }
    );
}

/// Golden **encode**: constructed values must reproduce the two POST-body
/// fixtures exactly as `serde_json::Value` (§2.1) — approve emits NO
/// `reason` key (`skip_serializing_if`), deny carries it.
#[test]
fn decision_request_golden_encode_approved() {
    let constructed = DecisionRequest {
        outcome: "approved".to_string(),
        reason: None,
    };
    let encoded = serde_json::to_value(&constructed).expect("serialize");
    let golden = val(&read_fixture("decision_request_approved.json"));
    assert_eq!(encoded, golden);
    assert!(
        !encoded.as_object().unwrap().contains_key("reason"),
        "approve must omit the `reason` key entirely, not serialize null"
    );
}

#[test]
fn decision_request_golden_encode_denied() {
    let constructed = DecisionRequest {
        outcome: "denied".to_string(),
        reason: Some("not during business hours".to_string()),
    };
    let encoded = serde_json::to_value(&constructed).expect("serialize");
    let golden = val(&read_fixture("decision_request_denied.json"));
    assert_eq!(encoded, golden);
}

#[test]
fn decision_request_fixtures_tolerate_unknown_field() {
    assert_tolerant_top_level::<DecisionRequest>(&read_fixture("decision_request_approved.json"));
    assert_tolerant_top_level::<DecisionRequest>(&read_fixture("decision_request_denied.json"));
}

// ---------------------------------------------------------------------------
// 14-16. error_*.json → ErrorBody
// ---------------------------------------------------------------------------

#[test]
fn error_unknown_gate_parses() {
    let raw = read_fixture("error_unknown_gate.json");
    let parsed: ErrorBody = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        ErrorBody {
            error: "unknown_gate".to_string(),
            detail: "gate-9".to_string()
        }
    );
}

#[test]
fn error_already_decided_parses() {
    let raw = read_fixture("error_already_decided.json");
    let parsed: ErrorBody = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(
        parsed,
        ErrorBody {
            error: "already_decided".to_string(),
            detail: "gate-1".to_string()
        }
    );
}

#[test]
fn error_malformed_parses() {
    let raw = read_fixture("error_malformed.json");
    let parsed: ErrorBody = serde_json::from_str(&raw).expect("must parse");
    assert_eq!(parsed.error, "malformed");
    assert_eq!(parsed.detail, "missing field `outcome` at line 1 column 2");
}

#[test]
fn error_fixtures_tolerate_unknown_field() {
    for name in [
        "error_unknown_gate.json",
        "error_already_decided.json",
        "error_malformed.json",
    ] {
        assert_tolerant_top_level::<ErrorBody>(&read_fixture(name));
    }
}

// ---------------------------------------------------------------------------
// Inventory pin — a partial or extended vendor must fail loudly.
// ---------------------------------------------------------------------------

const EXPECTED_FIXTURES: [&str; 16] = [
    "info.json",
    "hello.json",
    "gate_requested_external.json",
    "gate_requested_elicitation.json",
    "gate_decided_approved_operator.json",
    "gate_decided_denied_operator.json",
    "gate_decided_dismissed_timeout.json",
    "gate_decided_approved_elicitation.json",
    "audit_decision.json",
    "audit_message.json",
    "gates_snapshot.json",
    "decision_request_approved.json",
    "decision_request_denied.json",
    "error_unknown_gate.json",
    "error_already_decided.json",
    "error_malformed.json",
];

#[test]
fn vendored_directory_has_exactly_the_pinned_16_files() {
    let dir = fixture_path(".");
    let mut found: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("could not read {dir:?}: {e}"))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    found.sort();

    let mut expected: Vec<String> = EXPECTED_FIXTURES.iter().map(|s| s.to_string()).collect();
    expected.sort();

    assert_eq!(
        found, expected,
        "fixtures/operator-v1/ must contain exactly the 16 pinned files (partial/extended vendor detected)"
    );
    assert_eq!(found.len(), 16);
}

#[test]
fn every_pinned_fixture_file_exists_and_is_nonempty() {
    for name in EXPECTED_FIXTURES {
        let path = fixture_path(name);
        let meta = std::fs::metadata(&path).unwrap_or_else(|e| panic!("{path:?} missing: {e}"));
        assert!(meta.len() > 0, "{name} must be non-empty");
    }
}
