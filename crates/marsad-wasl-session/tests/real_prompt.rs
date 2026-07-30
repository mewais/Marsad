//! real_prompt.rs — black-box tests for `RealWaslSessionClient::prompt()`
//! (`POST /v1/targets/{id}/turns`; design §3, §4.2, §4.4). Asserts:
//!   * the client sends a `submit_request.json`-shaped body — including the
//!     `source: local_human`, a `busy_policy`, and the fresh single-hop
//!     `causation` (root_request, parent_request:null, path:[], hop:0);
//!   * each of the four shipped `receipt_*.json` variants parses into the
//!     correct `WaslTurnReceipt` (Started / Queued / RejectedBusy / Duplicate),
//!     with Duplicate wrapping the original verbatim;
//!   * the un-fixtured `rejected_unavailable` variant parses too (wire vocab
//!     variant with no golden fixture — constructed inline, documented).
//!
//! Loopback-real tier. RED until `RealWaslSessionClient` exists.

mod real_support;

use marsad_wasl_session::{
    RealWaslSessionProvider, WaslBusyPolicy, WaslCausation, WaslSessionClient,
    WaslSessionClientConfig, WaslSessionProvider, WaslTargetKey, WaslTurnReceipt, WaslTurnRequest,
};

use real_support::{compact_fixture, val, BufferedResponse, MockRuntimeServer};

fn bound_client(server: &MockRuntimeServer) -> Box<dyn WaslSessionClient> {
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));
    let provider = RealWaslSessionProvider::new(WaslSessionClientConfig {
        base_url: server.base_url(),
    });
    let (client, _snapshot) = provider
        .get_or_start(&WaslTargetKey(val(r#""ctx-42""#)))
        .expect("create should succeed");
    client
}

/// One human prompt carrying a fresh single-hop causation.
fn sample_request(root: &str, text: &str) -> WaslTurnRequest {
    WaslTurnRequest {
        idempotency_key: "opt-key-1".to_string(),
        text: text.to_string(),
        causation: WaslCausation::fresh(root.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The submitted body — submit_request.json shape (source, busy_policy, causation).
// ---------------------------------------------------------------------------

#[test]
fn prompt_posts_submit_request_shaped_body_with_local_human_source_and_causation() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_turns(BufferedResponse::json(
        200,
        compact_fixture("receipt_started.json"),
    ));

    let _ = client.prompt(
        sample_request("req-1", "remember token VIOLET-7F29"),
        WaslBusyPolicy::Queue { max_wait_ms: 30000 },
    );

    let turns = server.requests_to("POST", "/v1/targets/tgt-1/turns");
    assert_eq!(turns.len(), 1, "prompt does one POST .../turns");
    let body = turns[0].body_json();

    // source is ALWAYS local_human (Marsad never originates an Agent turn, §11)
    // — internally tagged {"type":"local_human"}.
    assert_eq!(
        body.get("source"),
        Some(&val(r#"{ "type": "local_human" }"#)),
        "prompt source must be local_human, got {body}"
    );

    // busy_policy is on the wire as {"type":"queue","max_wait_ms":30000}.
    assert_eq!(
        body.get("busy_policy"),
        Some(&val(r#"{ "type": "queue", "max_wait_ms": 30000 }"#)),
        "busy_policy must serialize to the wire tagged form, got {body}"
    );

    // causation: fresh single-hop, plain-string RequestIds, null parent.
    let causation = body.get("causation").expect("body carries causation");
    assert_eq!(causation.get("root_request"), Some(&val(r#""req-1""#)));
    assert_eq!(causation.get("parent_request"), Some(&val("null")));
    assert_eq!(causation.get("path"), Some(&val("[]")));
    assert_eq!(causation.get("hop"), Some(&val("0")));

    // idempotency_key is carried so a resend does not double-inject.
    assert_eq!(body.get("idempotency_key"), Some(&val(r#""opt-key-1""#)));

    // delivery carries the human text (context/from/kind/text/data shape).
    let delivery = body.get("delivery").expect("body carries delivery");
    assert_eq!(
        delivery.get("text"),
        Some(&val(r#""remember token VIOLET-7F29""#))
    );

    assert_eq!(
        turns[0].header("content-type"),
        Some("application/json"),
        "turns body sent as application/json"
    );
}

#[test]
fn prompt_reject_policy_serializes_reject_busy_policy() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_turns(BufferedResponse::json(
        200,
        compact_fixture("receipt_rejected_busy.json"),
    ));

    let _ = client.prompt(sample_request("req-2", "hi"), WaslBusyPolicy::Reject);

    let turns = server.requests_to("POST", "/v1/targets/tgt-1/turns");
    assert_eq!(
        turns[0].body_json().get("busy_policy"),
        Some(&val(r#"{ "type": "reject" }"#)),
        "Reject serializes to {{\"type\":\"reject\"}}"
    );
}

// ---------------------------------------------------------------------------
// Receipt variants — each receipt_*.json fixture parses to the right variant.
// ---------------------------------------------------------------------------

#[test]
fn prompt_parses_receipt_started() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_turns(BufferedResponse::json(
        200,
        compact_fixture("receipt_started.json"),
    ));

    match client.prompt(sample_request("r", "x"), WaslBusyPolicy::Reject) {
        WaslTurnReceipt::Started { turn_id, first_seq } => {
            assert_eq!(turn_id, "turn-7");
            assert_eq!(first_seq, 13);
        }
        other => panic!("expected Started, got {other:?}"),
    }
}

#[test]
fn prompt_parses_receipt_queued() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_turns(BufferedResponse::json(
        200,
        compact_fixture("receipt_queued.json"),
    ));

    match client.prompt(
        sample_request("r", "x"),
        WaslBusyPolicy::Queue { max_wait_ms: 1 },
    ) {
        WaslTurnReceipt::Queued { turn_id, position } => {
            assert_eq!(turn_id, "turn-8");
            assert_eq!(position, 1);
        }
        other => panic!("expected Queued, got {other:?}"),
    }
}

#[test]
fn prompt_parses_receipt_rejected_busy() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_turns(BufferedResponse::json(
        200,
        compact_fixture("receipt_rejected_busy.json"),
    ));

    match client.prompt(sample_request("r", "x"), WaslBusyPolicy::Reject) {
        WaslTurnReceipt::RejectedBusy { active_turn } => {
            assert_eq!(active_turn, "turn-7");
        }
        other => panic!("expected RejectedBusy, got {other:?}"),
    }
}

#[test]
fn prompt_parses_receipt_duplicate_wrapping_original_verbatim() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    // receipt_duplicate.json wraps a started original — a resend of an
    // already-seen (key, idempotency_key): NO second injection, original verbatim.
    server.push_turns(BufferedResponse::json(
        200,
        compact_fixture("receipt_duplicate.json"),
    ));

    match client.prompt(sample_request("r", "x"), WaslBusyPolicy::Reject) {
        WaslTurnReceipt::Duplicate { original } => match *original {
            WaslTurnReceipt::Started { turn_id, first_seq } => {
                assert_eq!(turn_id, "turn-7");
                assert_eq!(first_seq, 13);
            }
            other => panic!("Duplicate must wrap the original Started verbatim, got {other:?}"),
        },
        other => panic!("expected Duplicate, got {other:?}"),
    }
}

#[test]
fn prompt_parses_rejected_unavailable_variant() {
    // No golden fixture ships for rejected_unavailable (the wire vocabulary has
    // it, but Wasl vendors no receipt_rejected_unavailable.json). Constructed
    // inline from the documented wire shape: internally-tagged, wrapping a
    // TargetStatus. The append-only fixture policy forbids inventing a new file.
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    let body = r#"{ "type": "rejected_unavailable",
                    "status": { "type": "faulted", "error": "boom", "recoverable": false } }"#;
    server.push_turns(BufferedResponse::json(200, body));

    match client.prompt(sample_request("r", "x"), WaslBusyPolicy::Reject) {
        WaslTurnReceipt::RejectedUnavailable { status } => {
            assert_eq!(
                status,
                marsad_wasl_session::WaslTargetStatus::Faulted {
                    error: "boom".to_string(),
                    recoverable: false,
                }
            );
        }
        other => panic!("expected RejectedUnavailable, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// prompt is infallible in signature — a transport failure degrades gracefully.
// ---------------------------------------------------------------------------

#[test]
fn prompt_on_connection_refused_degrades_to_rejected_unavailable() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    drop(server); // turns POST now hits a refused connection.

    // prompt() cannot return a Result. A transport failure means the turn was
    // NOT admitted → RejectedUnavailable with a not-live status (never a bogus
    // Started that would make the caller wait for a reply that never comes).
    match client.prompt(sample_request("r", "x"), WaslBusyPolicy::Reject) {
        WaslTurnReceipt::RejectedUnavailable { status } => {
            assert!(
                !marsad_wasl_session::status_is_live(&status),
                "a transport-failure receipt must carry a not-live status, got {status:?}"
            );
        }
        other => panic!("a refused prompt must be RejectedUnavailable, got {other:?}"),
    }
}
