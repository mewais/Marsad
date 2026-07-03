//! errors.rs — failure/error paths for the SSE client + decision POST against
//! the loopback fake server (spec §7.1): 409/404/400 decision acks,
//! connection-refused, and reconnect-with-backoff after a server-side drop.
//!
//! This file WILL fail to compile until a later worker implements
//! `marsad_wasl`'s public surface (§2) — expected tests-first signal.
mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use marsad_wasl::{DecisionAck, Hello, WaslClientConfig, WaslError, WaslEvent, WaslOperatorClient};

use support::{
    compact_fixture, read_fixture, sse_frame, unused_addr, val, BufferedResponse, EventsScript,
    MockServer,
};

const RECV_TIMEOUT: Duration = Duration::from_secs(3);

fn expect_event(rx: &std::sync::mpsc::Receiver<WaslEvent>, what: &str) -> WaslEvent {
    rx.recv_timeout(RECV_TIMEOUT)
        .unwrap_or_else(|e| panic!("expected {what}: {e}"))
}

// ---------------------------------------------------------------------------
// Decision POST: 409 already_decided → AlreadyDecided (benign race).
// ---------------------------------------------------------------------------

#[test]
fn post_decision_409_already_decided_is_benign_ack() {
    let server = MockServer::start();
    server.push_decision_response(BufferedResponse::json(
        409,
        compact_fixture("error_already_decided.json"),
    ));

    let client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let ack = client
        .post_decision("gate-1", "approved", None)
        .expect("409 is a benign ack, not Err");
    assert_eq!(ack, DecisionAck::AlreadyDecided);
}

// ---------------------------------------------------------------------------
// Decision POST: 404 unknown_gate → UnknownGate (benign, stale UI).
// ---------------------------------------------------------------------------

#[test]
fn post_decision_404_unknown_gate_is_benign_ack() {
    let server = MockServer::start();
    server.push_decision_response(BufferedResponse::json(
        404,
        compact_fixture("error_unknown_gate.json"),
    ));

    let client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let ack = client
        .post_decision("gate-9", "approved", None)
        .expect("404 is a benign ack, not Err");
    assert_eq!(ack, DecisionAck::UnknownGate);
}

// ---------------------------------------------------------------------------
// Decision POST: 400 malformed → Err(WaslError::Http{status:400, ..}) (a bug,
// not a race — Marsad only ever sends schema-correct bodies).
// ---------------------------------------------------------------------------

#[test]
fn post_decision_400_malformed_is_http_error() {
    let server = MockServer::start();
    server.push_decision_response(BufferedResponse::json(
        400,
        compact_fixture("error_malformed.json"),
    ));

    let client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let err = client
        .post_decision("gate-1", "approved", None)
        .expect_err("400 must surface as an error, never as an ack");

    match err {
        WaslError::Http { status, body } => {
            assert_eq!(status, 400);
            let body_json: serde_json::Value =
                serde_json::from_str(&body).expect("error body must be JSON");
            assert_eq!(body_json, val(&read_fixture("error_malformed.json")));
        }
        other => panic!("expected WaslError::Http{{status: 400, ..}}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Decision POST: connection refused → Err(WaslError::Io(_)).
// ---------------------------------------------------------------------------

#[test]
fn post_decision_connection_refused_is_io_error() {
    let dead_addr = unused_addr();
    let client = WaslOperatorClient::new(WaslClientConfig {
        base_url: format!("http://{dead_addr}"),
        initial_backoff_ms: 1,
    });
    let err = client
        .post_decision("gate-1", "approved", None)
        .expect_err("connection refused must be an error");
    assert!(
        matches!(err, WaslError::Io(_)),
        "expected WaslError::Io(_) for connection refused, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Reconnect with backoff: server drops the connection right after hello;
// client must emit Disconnected, then reconnect (a fresh GET /v1/events
// arrives) and emit Connected again.
// ---------------------------------------------------------------------------

#[test]
fn server_drop_after_hello_triggers_disconnect_then_reconnect() {
    let server = MockServer::start();

    let hello_data = compact_fixture("hello.json");

    // First connection: hello only, then an abrupt drop (no graceful chunk
    // terminator) — simulates a node crash / network partition.
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));
    server.push_events_script(
        EventsScript::new(sse_frame("hello", None, &hello_data)).drop_after(true),
    );

    // Second connection (the reconnect): hello again, held open.
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));
    server.push_events_script(EventsScript::new(sse_frame("hello", None, &hello_data)));

    let mut client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let handle = thread::spawn(move || client.run(tx, stop_thread));

    let expected_hello: Hello = serde_json::from_str(&read_fixture("hello.json")).unwrap();

    assert_eq!(
        expect_event(&rx, "first Connected"),
        WaslEvent::Connected {
            hello: expected_hello.clone()
        }
    );
    match expect_event(&rx, "Disconnected") {
        WaslEvent::Disconnected { .. } => {}
        other => panic!("expected Disconnected after the server drop, got {other:?}"),
    }
    assert_eq!(
        expect_event(&rx, "second Connected (after reconnect)"),
        WaslEvent::Connected {
            hello: expected_hello
        }
    );

    stop.store(true, Ordering::SeqCst);
    handle.join().expect("client thread must exit after stop");

    assert_eq!(
        server.events_connection_count(),
        2,
        "exactly one reconnect must have happened (fresh GET /v1/events)"
    );
}

// ---------------------------------------------------------------------------
// wasl_backoff_next — pure doubling-with-cap table, incl. 0 and cap inputs.
// ---------------------------------------------------------------------------

#[test]
fn wasl_backoff_next_doubles_and_caps() {
    use marsad_wasl::{wasl_backoff_next, WASL_BACKOFF_MAX_MS};

    assert_eq!(WASL_BACKOFF_MAX_MS, 60_000);

    let table: &[(u64, u64)] = &[
        // Mirrors marsad_agent::reconnect_backoff_next's
        // `current.saturating_mul(2).min(CAP)` shape: doubling zero stays zero.
        (0, 0),
        (1, 2),
        (2, 4),
        (4, 8),
        (8, 16),
        (16, 32),
        (1_000, 2_000),
        (30_000, 60_000),
        (40_000, 60_000), // doubling would overshoot the cap
        (59_999, 60_000),
        (60_000, 60_000), // already at cap: stays at cap
    ];
    for &(input, expected) in table {
        assert_eq!(
            wasl_backoff_next(input),
            expected,
            "wasl_backoff_next({input}) should be {expected}"
        );
    }
}
