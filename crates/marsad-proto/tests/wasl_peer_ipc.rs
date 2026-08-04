//! wasl_peer_ipc.rs — round-trip tests for the two NEW Wasl peer-registration
//! IPC messages (`RegisterWaslPeer` / `UnregisterWaslPeer`), added as part of
//! the dynamic node-to-node peer registration feature.
//!
//! Follows the exact per-variant contract convention of `tests/wasl_ipc.rs`
//! (golden parse / encode->parse identity / re-encode stability /
//! golden-encode-as-Value / unknown-field tolerance).

use marsad_proto::{encode_ipc, parse_ipc, IpcMsg, ProtoError};

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
    let parsed = parse_ipc(golden)
        .unwrap_or_else(|e| panic!("golden must parse: {e:?}\n{golden}"));
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
// RegisterWaslPeer — with endpoint
// ---------------------------------------------------------------------------

#[test]
fn register_wasl_peer_with_endpoint_round_trips() {
    let golden = r#"{"type":"RegisterWaslPeer","host_id":"host-a","peer_id":"marsad-host-b","peer_host":"host-b","peer_endpoint":"http://127.0.0.1:8787"}"#;
    let expected = IpcMsg::RegisterWaslPeer {
        host_id: "host-a".to_string(),
        peer_id: "marsad-host-b".to_string(),
        peer_host: "host-b".to_string(),
        peer_endpoint: Some("http://127.0.0.1:8787".to_string()),
    };
    assert_variant_contract(golden, &expected);
}

// ---------------------------------------------------------------------------
// RegisterWaslPeer — without endpoint (None → field omitted on encode)
// ---------------------------------------------------------------------------

#[test]
fn register_wasl_peer_no_endpoint_round_trips() {
    let golden = r#"{"type":"RegisterWaslPeer","host_id":"host-a","peer_id":"marsad-host-b","peer_host":"host-b"}"#;
    let expected = IpcMsg::RegisterWaslPeer {
        host_id: "host-a".to_string(),
        peer_id: "marsad-host-b".to_string(),
        peer_host: "host-b".to_string(),
        peer_endpoint: None,
    };
    assert_variant_contract(golden, &expected);
}

// ---------------------------------------------------------------------------
// UnregisterWaslPeer — round-trip
// ---------------------------------------------------------------------------

#[test]
fn unregister_wasl_peer_round_trips() {
    let golden = r#"{"type":"UnregisterWaslPeer","host_id":"host-a","peer_id":"marsad-host-b"}"#;
    let expected = IpcMsg::UnregisterWaslPeer {
        host_id: "host-a".to_string(),
        peer_id: "marsad-host-b".to_string(),
    };
    assert_variant_contract(golden, &expected);
}

// ---------------------------------------------------------------------------
// KNOWN-list closure: both new tags must be in the known discriminant set.
// ---------------------------------------------------------------------------

#[test]
fn register_wasl_peer_is_in_known_list() {
    let line = r#"{"type":"RegisterWaslPeer","host_id":"h","peer_id":"p","peer_host":"ph"}"#;
    // Must NOT return UnknownDiscriminant.
    assert!(
        !matches!(parse_ipc(line), Err(ProtoError::UnknownDiscriminant(_))),
        "RegisterWaslPeer must be in KNOWN"
    );
}

#[test]
fn unregister_wasl_peer_is_in_known_list() {
    let line = r#"{"type":"UnregisterWaslPeer","host_id":"h","peer_id":"p"}"#;
    assert!(
        !matches!(parse_ipc(line), Err(ProtoError::UnknownDiscriminant(_))),
        "UnregisterWaslPeer must be in KNOWN"
    );
}
