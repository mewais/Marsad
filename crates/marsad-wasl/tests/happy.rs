//! happy.rs — nominal paths for the SSE client against a real loopback fake
//! server (spec §7.1, second half), plus a couple of dependency-free nominal
//! paths (config round-trip) that don't need the server.
//!
//! This is the "loopback-real tier": `support::MockServer` speaks real
//! HTTP/1.1 over a real `std::net::TcpListener`, so `WaslOperatorClient`'s
//! HTTP/SSE code runs end to end, exactly as it would against a live Wasl
//! node.
//!
//! This file WILL fail to compile until a later worker implements
//! `marsad_wasl`'s public surface (§2) — expected tests-first signal.
mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use marsad_wasl::{
    DecisionAck, GateDecided, GateRequest, GateRequested, Hello, WaslClientConfig, WaslEvent,
    WaslOperatorClient, WaslOperatorEntry, WaslOperatorsConfig,
};

use support::{
    compact_fixture, read_fixture, sse_comment_line, sse_frame, val, BufferedResponse,
    EventsScript, MockServer, SSE_BLANK,
};

const RECV_TIMEOUT: Duration = Duration::from_secs(3);

fn expect_event(rx: &std::sync::mpsc::Receiver<WaslEvent>, what: &str) -> WaslEvent {
    rx.recv_timeout(RECV_TIMEOUT)
        .unwrap_or_else(|e| panic!("expected {what}: {e}"))
}

// ---------------------------------------------------------------------------
// 1. hello-then-frames: the canonical connect + stream happy path.
// ---------------------------------------------------------------------------

#[test]
fn hello_then_frames_emits_connected_gate_requested_gate_decided() {
    let server = MockServer::start();

    // Connect-time resnapshot: empty, so it contributes nothing extra.
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));

    let hello_data = compact_fixture("hello.json");
    let gr_data = compact_fixture("gate_requested_external.json");
    let gd_data = compact_fixture("gate_decided_approved_operator.json");

    let mut lines = sse_frame("hello", None, &hello_data);
    lines.push(sse_comment_line("ka"));
    lines.push(SSE_BLANK.to_string());
    lines.extend(sse_frame("gate_requested", Some(43), &gr_data));
    lines.extend(sse_frame("gate_decided", Some(44), &gd_data));
    server.push_events_script(EventsScript::new(lines));

    let mut client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let handle = thread::spawn(move || client.run(tx, stop_thread));

    let expected_hello: Hello = serde_json::from_str(&read_fixture("hello.json")).unwrap();
    let expected_gr: GateRequested =
        serde_json::from_str(&read_fixture("gate_requested_external.json")).unwrap();
    let expected_gd: GateDecided =
        serde_json::from_str(&read_fixture("gate_decided_approved_operator.json")).unwrap();

    assert_eq!(
        expect_event(&rx, "Connected"),
        WaslEvent::Connected {
            hello: expected_hello
        }
    );
    assert_eq!(
        expect_event(&rx, "GateRequested"),
        WaslEvent::GateRequested(expected_gr)
    );
    assert_eq!(
        expect_event(&rx, "GateDecided"),
        WaslEvent::GateDecided(expected_gd)
    );

    stop.store(true, Ordering::SeqCst);
    handle.join().expect("client thread must exit after stop");
}

// ---------------------------------------------------------------------------
// 2. connect-resnapshot: /v1/gates returns gates_snapshot.json → synthesized
//    GateRequested (mode=="external", decidable==true, timeout_ms==0).
// ---------------------------------------------------------------------------

#[test]
fn connect_resnapshot_synthesizes_gate_requested_from_snapshot() {
    let server = MockServer::start();

    server.push_gates_response(BufferedResponse::json(
        200,
        compact_fixture("gates_snapshot.json"),
    ));

    let hello_data = compact_fixture("hello.json");
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
        expect_event(&rx, "Connected"),
        WaslEvent::Connected {
            hello: expected_hello
        }
    );

    let expected_synth = GateRequested {
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
        timeout_ms: 0,
        deadline: "2026-07-02T12:02:00.000Z".to_string(),
    };
    assert_eq!(
        expect_event(&rx, "synthesized GateRequested"),
        WaslEvent::GateRequested(expected_synth)
    );

    stop.store(true, Ordering::SeqCst);
    handle.join().expect("client thread must exit after stop");
    assert_eq!(server.requests_to("/v1/gates").len(), 1);
}

// ---------------------------------------------------------------------------
// 3. WaslOperatorsConfig / WaslOperatorEntry — the shared config shape (§2.7).
// ---------------------------------------------------------------------------

#[test]
fn wasl_operators_config_parses_the_spec_example() {
    let json = r#"{ "operators": [ { "host_id": "local", "url": "http://127.0.0.1:8787" } ] }"#;
    let parsed: WaslOperatorsConfig = serde_json::from_str(json).expect("must parse");
    assert_eq!(
        parsed,
        WaslOperatorsConfig {
            operators: vec![WaslOperatorEntry {
                host_id: "local".to_string(),
                url: "http://127.0.0.1:8787".to_string(),
            }],
        }
    );
}

#[test]
fn wasl_operators_config_round_trips_through_serialize() {
    let original = WaslOperatorsConfig {
        operators: vec![
            WaslOperatorEntry {
                host_id: "local".to_string(),
                url: "http://127.0.0.1:8787".to_string(),
            },
            WaslOperatorEntry {
                host_id: "remote-1".to_string(),
                url: "http://10.0.0.5:8787".to_string(),
            },
        ],
    };
    let encoded = serde_json::to_string(&original).expect("serialize");
    let reparsed: WaslOperatorsConfig = serde_json::from_str(&encoded).expect("reparse");
    assert_eq!(original, reparsed);
}

// ---------------------------------------------------------------------------
// 4. Decision POST happy paths: 204 → Delivered, for both approve and deny.
// ---------------------------------------------------------------------------

#[test]
fn post_decision_approved_returns_delivered_and_sends_the_approve_golden() {
    let server = MockServer::start();
    server.push_decision_response(BufferedResponse::empty(204));

    let client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let ack = client
        .post_decision("gate-1", "approved", None)
        .expect("post_decision must succeed");
    assert_eq!(ack, DecisionAck::Delivered);

    let reqs = server.requests_to("/v1/gates/gate-1/decision");
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.header("Content-Type"),
        Some("application/json"),
        "decision POST must set Content-Type: application/json"
    );
    assert_eq!(
        req.body_json(),
        val(&read_fixture("decision_request_approved.json"))
    );
}

#[test]
fn post_decision_denied_returns_delivered_and_sends_the_deny_golden() {
    let server = MockServer::start();
    server.push_decision_response(BufferedResponse::empty(204));

    let client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let ack = client
        .post_decision("gate-1", "denied", Some("not during business hours"))
        .expect("post_decision must succeed");
    assert_eq!(ack, DecisionAck::Delivered);

    let reqs = server.requests_to("/v1/gates/gate-1/decision");
    assert_eq!(reqs.len(), 1);
    assert_eq!(
        reqs[0].body_json(),
        val(&read_fixture("decision_request_denied.json"))
    );
}
