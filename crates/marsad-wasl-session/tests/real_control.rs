//! real_control.rs — black-box tests for the fallible control verbs of
//! `RealWaslSessionClient` (design §3, §4.2):
//!   * `resolve_native_gate` → `POST /v1/targets/{id}/gate` (approve + deny),
//!     body = `gate_request_*.json` shape, keyed by turn_id (no gate id);
//!   * `interrupt` → `POST /v1/targets/{id}/cancel`, body = `cancel_request.json`;
//!   * `shutdown` → `POST /v1/targets/{id}/shutdown`, body =
//!     `shutdown_request_drain.json` (and the immediate mode).
//! Each returns `Result<(), WaslSessionError>` (204 → Ok; 4xx → the right error;
//! transport failure → bounded Io, never a hang).
//!
//! Loopback-real tier. RED until `RealWaslSessionClient` exists.

mod real_support;

use marsad_wasl_session::{
    RealWaslSessionProvider, WaslCancelReason, WaslGateDecision, WaslSessionClient,
    WaslSessionClientConfig, WaslSessionError, WaslSessionProvider, WaslShutdownMode,
    WaslTargetKey, WaslTurnId,
};

use real_support::{compact_fixture, fixture_value, val, BufferedResponse, MockRuntimeServer};

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

// ---------------------------------------------------------------------------
// resolve_native_gate — POST /v1/targets/{id}/gate, approve + deny.
// ---------------------------------------------------------------------------

#[test]
fn resolve_native_gate_approve_posts_gate_request_approve_shape() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_gate(BufferedResponse::empty(204));

    client
        .resolve_native_gate(&WaslTurnId("turn-7".to_string()), WaslGateDecision::Approve)
        .expect("204 → Ok");

    let posts = server.requests_to("POST", "/v1/targets/tgt-1/gate");
    assert_eq!(posts.len(), 1, "one gate POST");
    let body = posts[0].body_json();
    // Keyed by turn_id (there is no gate id); decision is the GateDecision wire
    // form {"decision":"approve"}. Value-equality with the vendored fixture.
    assert_eq!(body.get("turn_id"), Some(&val(r#""turn-7""#)));
    assert_eq!(
        body.get("decision"),
        fixture_value("gate_request_approve.json").get("decision"),
        "approve decision matches the vendored gate_request_approve.json, got {body}"
    );
}

#[test]
fn resolve_native_gate_deny_carries_reason() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_gate(BufferedResponse::empty(204));

    client
        .resolve_native_gate(
            &WaslTurnId("turn-7".to_string()),
            WaslGateDecision::Deny {
                reason: "not allowed".to_string(),
            },
        )
        .expect("204 → Ok");

    let body = server.requests_to("POST", "/v1/targets/tgt-1/gate")[0].body_json();
    // deny + reason → {"decision":{"decision":"deny","reason":"not allowed"}}.
    assert_eq!(
        body.get("decision"),
        fixture_value("gate_request_deny.json").get("decision"),
        "deny decision + reason matches gate_request_deny.json, got {body}"
    );
}

#[test]
fn resolve_native_gate_409_maps_to_unavailable() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    // 409 gate_unresolvable: the target is not in AwaitingGate.
    server.push_gate(BufferedResponse::json(
        409,
        r#"{ "error": "gate_unresolvable", "detail": "target not awaiting a gate" }"#,
    ));

    let err = client
        .resolve_native_gate(&WaslTurnId("turn-7".to_string()), WaslGateDecision::Approve)
        .expect_err("409 gate_unresolvable is an error");
    // Not a malformed/protocol fault — the target simply cannot service this
    // resolution now → Unavailable, carrying the detail.
    match err {
        WaslSessionError::Unavailable(detail) => {
            assert!(detail.contains("not awaiting a gate"), "got {detail:?}");
        }
        other => panic!("expected Unavailable for gate_unresolvable, got {other:?}"),
    }
}

#[test]
fn resolve_native_gate_404_unknown_target_maps_to_unavailable() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_gate(BufferedResponse::json(
        404,
        compact_fixture("error_unknown_target.json"),
    ));

    let err = client
        .resolve_native_gate(&WaslTurnId("turn-7".to_string()), WaslGateDecision::Approve)
        .expect_err("404 is an error");
    assert!(
        matches!(err, WaslSessionError::Unavailable(_)),
        "unknown target on a control op → Unavailable, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// interrupt — POST /v1/targets/{id}/cancel, cancel_request.json shape.
// ---------------------------------------------------------------------------

#[test]
fn interrupt_posts_cancel_request_shape() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_cancel(BufferedResponse::empty(204));

    client.interrupt(WaslCancelReason::Human).expect("204 → Ok");

    let posts = server.requests_to("POST", "/v1/targets/tgt-1/cancel");
    assert_eq!(posts.len(), 1, "one cancel POST");
    let body = posts[0].body_json();
    // cancel_request.json is {"turn_id":"turn-7","reason":"operator_request"};
    // the client must carry a turn_id and a CancelReason. Human maps to the
    // wire's human/operator-request reason.
    assert!(
        body.get("turn_id").is_some(),
        "cancel body carries a turn_id, got {body}"
    );
    assert!(
        body.get("reason").is_some(),
        "cancel body carries a reason, got {body}"
    );
}

#[test]
fn interrupt_409_unsupported_not_interruptible_maps_to_unavailable() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    // 409 unsupported: cancel on a !interruptible target — an honest refusal.
    server.push_cancel(BufferedResponse::json(
        409,
        compact_fixture("error_unsupported.json"),
    ));

    let err = client
        .interrupt(WaslCancelReason::Human)
        .expect_err("409 unsupported is an error");
    match err {
        WaslSessionError::Unavailable(detail) => {
            assert!(detail.contains("not interruptible"), "got {detail:?}");
        }
        other => panic!("expected Unavailable for unsupported cancel, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// shutdown — POST /v1/targets/{id}/shutdown, drain + immediate.
// ---------------------------------------------------------------------------

#[test]
fn shutdown_drain_posts_shutdown_request_drain_shape() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_shutdown(BufferedResponse::empty(204));

    client.shutdown(WaslShutdownMode::Drain).expect("204 → Ok");

    let posts = server.requests_to("POST", "/v1/targets/tgt-1/shutdown");
    assert_eq!(posts.len(), 1, "one shutdown POST");
    // shutdown_request_drain.json is {"mode":"drain"} — value equality.
    assert_eq!(
        posts[0].body_json(),
        fixture_value("shutdown_request_drain.json"),
        "drain shutdown body matches the vendored fixture"
    );
}

#[test]
fn shutdown_immediate_serializes_immediate_mode() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_shutdown(BufferedResponse::empty(204));

    client
        .shutdown(WaslShutdownMode::Immediate)
        .expect("204 → Ok");

    let body = server.requests_to("POST", "/v1/targets/tgt-1/shutdown")[0].body_json();
    assert_eq!(
        body.get("mode"),
        Some(&val(r#""immediate""#)),
        "Immediate serializes to {{\"mode\":\"immediate\"}}, got {body}"
    );
}

#[test]
fn shutdown_404_unknown_target_maps_to_unavailable() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_shutdown(BufferedResponse::json(
        404,
        compact_fixture("error_unknown_target.json"),
    ));

    let err = client
        .shutdown(WaslShutdownMode::Drain)
        .expect_err("404 is an error");
    assert!(
        matches!(err, WaslSessionError::Unavailable(_)),
        "unknown target on shutdown → Unavailable, got {err:?}"
    );
}
