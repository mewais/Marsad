//! branches.rs — table-driven / edge-path cases closing remaining regions
//! (spec §7.1): `SseParser` framing details exercised directly (no server
//! needed), plus server-backed branch coverage of the connect/reconnect/gap
//! state machine (§2.5): unknown event types, observe-only gates, duplicate
//! reconcile, non-hello/bad-version first frames, and chunked vs
//! Content-Length / raw streaming body variants.
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
    AuditFrame, GateDecided, GateRequest, GateRequested, Hello, SseFrame, SseParser,
    WaslClientConfig, WaslEvent, WaslOperatorClient,
};

use support::{
    compact_fixture, read_fixture, sse_frame, val, BufferedResponse, EventsScript, MockServer,
};

const RECV_TIMEOUT: Duration = Duration::from_secs(3);
const QUIET_TIMEOUT: Duration = Duration::from_millis(400);

fn expect_event(rx: &std::sync::mpsc::Receiver<WaslEvent>, what: &str) -> WaslEvent {
    rx.recv_timeout(RECV_TIMEOUT)
        .unwrap_or_else(|e| panic!("expected {what}: {e}"))
}

fn assert_channel_quiet(rx: &std::sync::mpsc::Receiver<WaslEvent>) {
    match rx.recv_timeout(QUIET_TIMEOUT) {
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        other => panic!("expected no further events, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// SseParser — direct, dependency-free framing tests.
// ---------------------------------------------------------------------------

#[test]
fn sse_dispatches_a_complete_frame_on_blank_line() {
    let mut p = SseParser::new();
    assert_eq!(p.push_line("event: hello"), None);
    assert_eq!(p.push_line("data: {\"a\":1}"), None);
    assert_eq!(
        p.push_line(""),
        Some(SseFrame {
            event: "hello".to_string(),
            id: None,
            data: "{\"a\":1}".to_string()
        })
    );
}

#[test]
fn sse_multiple_data_lines_join_with_newline() {
    let mut p = SseParser::new();
    p.push_line("event: audit");
    p.push_line("data: line1");
    p.push_line("data: line2");
    let frame = p.push_line("").expect("frame must dispatch");
    assert_eq!(frame.data, "line1\nline2");
}

#[test]
fn sse_id_parses_as_u64() {
    let mut p = SseParser::new();
    p.push_line("event: gate_decided");
    p.push_line("id: 44");
    p.push_line("data: {}");
    let frame = p.push_line("").expect("frame must dispatch");
    assert_eq!(frame.id, Some(44));
}

#[test]
fn sse_unparsable_id_becomes_none() {
    let mut p = SseParser::new();
    p.push_line("event: gate_decided");
    p.push_line("id: not-a-number");
    p.push_line("data: {}");
    let frame = p.push_line("").expect("frame must dispatch");
    assert_eq!(frame.id, None);
}

#[test]
fn sse_comment_lines_produce_no_frame() {
    let mut p = SseParser::new();
    assert_eq!(p.push_line(": ka"), None);
    assert_eq!(
        p.push_line(""),
        None,
        "a comment-only block has no event: and must be dropped"
    );
}

#[test]
fn sse_frame_with_no_event_is_dropped() {
    let mut p = SseParser::new();
    p.push_line("data: orphan");
    assert_eq!(
        p.push_line(""),
        None,
        "a dispatched frame with no event: must be dropped"
    );
}

#[test]
fn sse_unknown_field_names_are_ignored() {
    let mut p = SseParser::new();
    p.push_line("retry: 3000"); // not one of event:/data:/id: — must be ignored, not error
    p.push_line("event: hello");
    p.push_line("data: {}");
    let frame = p
        .push_line("")
        .expect("frame must dispatch despite the unknown field");
    assert_eq!(frame.event, "hello");
    assert_eq!(frame.data, "{}");
}

#[test]
fn sse_blank_line_with_nothing_pending_is_none() {
    let mut p = SseParser::new();
    assert_eq!(p.push_line(""), None);
    assert_eq!(p.push_line(""), None);
}

#[test]
fn sse_two_frames_in_sequence_both_dispatch_independently() {
    let mut p = SseParser::new();
    p.push_line("event: hello");
    p.push_line("data: {}");
    let f1 = p.push_line("").expect("first frame");
    assert_eq!(f1.event, "hello");

    p.push_line("event: audit");
    p.push_line("id: 5");
    p.push_line("data: {}");
    let f2 = p.push_line("").expect("second frame");
    assert_eq!(f2.event, "audit");
    assert_eq!(f2.id, Some(5));
}

// ---------------------------------------------------------------------------
// Unknown event type: skipped, but its id: still advances gap tracking.
// ---------------------------------------------------------------------------

#[test]
fn unknown_event_type_is_skipped_but_id_still_advances_gap_tracking() {
    let server = MockServer::start();
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));

    let hello_data = compact_fixture("hello.json");
    let gr_data = compact_fixture("gate_requested_external.json");

    let mut lines = sse_frame("hello", None, &hello_data);
    // Unknown event type occupies id 43; if the client correctly advances
    // last_id past it, the next real frame (id 44) is NOT a gap.
    lines.extend(sse_frame(
        "future_event_type",
        Some(43),
        r#"{"whatever":true}"#,
    ));
    lines.extend(sse_frame("gate_requested", Some(44), &gr_data));
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

    assert_eq!(
        expect_event(&rx, "Connected"),
        WaslEvent::Connected {
            hello: expected_hello
        }
    );
    assert_eq!(
        expect_event(&rx, "GateRequested for id-44 frame"),
        WaslEvent::GateRequested(expected_gr)
    );
    assert_channel_quiet(&rx);

    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();

    assert_eq!(
        server.requests_to("/v1/gates").len(),
        1,
        "id must have advanced past the unknown frame — no false-positive gap, no extra resnapshot"
    );
}

// ---------------------------------------------------------------------------
// Observe-only gate_requested (decidable == false): not tracked, so its
// later absence from a snapshot never yields GateVanished.
// ---------------------------------------------------------------------------

#[test]
fn observe_only_gate_requested_is_not_tracked_and_never_vanishes() {
    let server = MockServer::start();
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));
    // Second resnapshot, triggered by the gap below — still empty.
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":45,"gates":[]}"#));

    let hello_data = compact_fixture("hello.json");
    let elicitation_data = compact_fixture("gate_requested_elicitation.json");
    let decided_data = compact_fixture("gate_decided_approved_operator.json");

    let mut lines = sse_frame("hello", None, &hello_data);
    lines.extend(sse_frame("gate_requested", Some(43), &elicitation_data)); // decidable: false
    lines.extend(sse_frame("gate_decided", Some(45), &decided_data)); // skips 44 -> gap
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
    let expected_elicitation: GateRequested =
        serde_json::from_str(&read_fixture("gate_requested_elicitation.json")).unwrap();
    let expected_decided: GateDecided =
        serde_json::from_str(&read_fixture("gate_decided_approved_operator.json")).unwrap();

    assert_eq!(
        expect_event(&rx, "Connected"),
        WaslEvent::Connected {
            hello: expected_hello
        }
    );
    assert_eq!(
        expect_event(&rx, "observe-only GateRequested"),
        WaslEvent::GateRequested(expected_elicitation)
    );
    assert_eq!(
        expect_event(&rx, "GateDecided (gap-triggering frame)"),
        WaslEvent::GateDecided(expected_decided)
    );
    // The elicitation gate was never tracked (decidable: false), so its
    // absence from the post-gap resnapshot must NOT produce GateVanished.
    assert_channel_quiet(&rx);

    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
    assert_eq!(
        server.requests_to("/v1/gates").len(),
        2,
        "the gap must trigger exactly one extra resnapshot"
    );
}

// ---------------------------------------------------------------------------
// Gap → resnapshot, stream stays open: a tracked gate absent from the
// post-gap snapshot vanishes; the TCP connection is never reopened.
// ---------------------------------------------------------------------------

#[test]
fn gap_triggers_resnapshot_on_the_same_connection_and_vanishes_untracked_gate() {
    let server = MockServer::start();

    // Connect-time resnapshot: one pre-existing pending gate, "gate-9",
    // synthesized as tracked.
    server.push_gates_response(BufferedResponse::json(
        200,
        r#"{"last_seq":42,"gates":[{"gate_id":"gate-9","request":{"id":"call-99","kind":"command","from":"wasl-c","summary":"command to 1 peer(s)","preview":"do the thing"},"requested_at":"2026-07-02T12:05:00.000Z","age_ms":500,"deadline":"2026-07-02T12:07:00.000Z"}]}"#,
    ));
    // Post-gap resnapshot: gate-9 is gone (resolved while this connection lagged).
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":45,"gates":[]}"#));

    let hello_data = compact_fixture("hello.json");
    let gr_data = compact_fixture("gate_requested_external.json"); // tracks "gate-1"
    let gd_data = compact_fixture("gate_decided_approved_operator.json"); // decides "gate-1"

    let mut lines = sse_frame("hello", None, &hello_data);
    lines.extend(sse_frame("gate_requested", Some(43), &gr_data));
    lines.extend(sse_frame("gate_decided", Some(45), &gd_data)); // skips 44 -> gap

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
    let expected_gate9 = GateRequested {
        gate_id: "gate-9".to_string(),
        mode: "external".to_string(),
        decidable: true,
        request: GateRequest {
            id: "call-99".to_string(),
            kind: "command".to_string(),
            from: "wasl-c".to_string(),
            summary: "command to 1 peer(s)".to_string(),
            preview: "do the thing".to_string(),
        },
        requested_at: "2026-07-02T12:05:00.000Z".to_string(),
        timeout_ms: 0,
        deadline: "2026-07-02T12:07:00.000Z".to_string(),
    };
    let expected_gate1: GateRequested =
        serde_json::from_str(&read_fixture("gate_requested_external.json")).unwrap();
    let expected_decided: GateDecided =
        serde_json::from_str(&read_fixture("gate_decided_approved_operator.json")).unwrap();

    assert_eq!(
        expect_event(&rx, "Connected"),
        WaslEvent::Connected {
            hello: expected_hello
        }
    );
    assert_eq!(
        expect_event(&rx, "synthesized GateRequested(gate-9)"),
        WaslEvent::GateRequested(expected_gate9)
    );
    assert_eq!(
        expect_event(&rx, "stream GateRequested(gate-1)"),
        WaslEvent::GateRequested(expected_gate1)
    );
    assert_eq!(
        expect_event(&rx, "stream GateDecided(gate-1)"),
        WaslEvent::GateDecided(expected_decided)
    );
    assert_eq!(
        expect_event(&rx, "GateVanished(gate-9)"),
        WaslEvent::GateVanished {
            gate_id: "gate-9".to_string()
        }
    );
    assert_channel_quiet(&rx);

    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();

    assert_eq!(
        server.requests_to("/v1/gates").len(),
        2,
        "gap must trigger exactly one extra resnapshot"
    );
    assert_eq!(
        server.events_connection_count(),
        1,
        "the gap handling must NOT reopen the /v1/events TCP stream"
    );
}

// ---------------------------------------------------------------------------
// A snapshot gate already tracked yields no duplicate GateRequested.
// ---------------------------------------------------------------------------

#[test]
fn snapshot_gate_already_tracked_is_not_re_emitted() {
    let server = MockServer::start();

    let snapshot_body = compact_fixture("gates_snapshot.json"); // contains gate-1, pending
    server.push_gates_response(BufferedResponse::json(200, &snapshot_body));
    // Post-gap resnapshot: gate-1 is STILL pending (identical snapshot).
    server.push_gates_response(BufferedResponse::json(200, &snapshot_body));

    let hello_data = compact_fixture("hello.json");
    let audit_data = compact_fixture("audit_decision.json");

    let mut lines = sse_frame("hello", None, &hello_data);
    // id 44 skips 43 (hello.last_seq == 42) -> gap on the very first stream frame.
    lines.extend(sse_frame("audit", Some(44), &audit_data));
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
    let expected_gate1 = GateRequested {
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
    let expected_audit: AuditFrame =
        serde_json::from_str(&read_fixture("audit_decision.json")).unwrap();

    assert_eq!(
        expect_event(&rx, "Connected"),
        WaslEvent::Connected {
            hello: expected_hello
        }
    );
    assert_eq!(
        expect_event(&rx, "synthesized GateRequested(gate-1)"),
        WaslEvent::GateRequested(expected_gate1)
    );
    assert_eq!(
        expect_event(&rx, "Audit"),
        WaslEvent::Audit(expected_audit.entry)
    );
    // gate-1 was already tracked before the gap; it must NOT be re-emitted by
    // the post-gap reconcile, and it's still present so it must not vanish.
    assert_channel_quiet(&rx);

    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
    assert_eq!(server.requests_to("/v1/gates").len(), 2);
}

// ---------------------------------------------------------------------------
// Non-hello first frame / api_version mismatch: both are protocol
// violations per §2.5 step 2 ("MUST be hello... api_version != 1 -> log,
// drop the connection, back off, retry") — the client drops the bad
// connection (surfacing as Disconnected, reusing the same backoff/retry
// mechanics as an IO error) and never emits Connected for it, then
// reconnects successfully once the server serves a valid hello.
// ---------------------------------------------------------------------------

#[test]
fn non_hello_first_frame_is_dropped_then_client_reconnects() {
    let server = MockServer::start();

    let gr_data = compact_fixture("gate_requested_external.json");
    // Bad connection: a gate_requested frame arrives first, no hello at all.
    server.push_events_script(
        EventsScript::new(sse_frame("gate_requested", Some(1), &gr_data)).drop_after(true),
    );

    // Good connection (the reconnect): a valid hello, held open.
    let hello_data = compact_fixture("hello.json");
    server.push_events_script(EventsScript::new(sse_frame("hello", None, &hello_data)));
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));

    let mut client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let handle = thread::spawn(move || client.run(tx, stop_thread));

    match expect_event(&rx, "Disconnected for the bad first connection") {
        WaslEvent::Disconnected { .. } => {}
        other => {
            panic!("expected Disconnected for the non-hello-first-frame connection, got {other:?}")
        }
    }
    let expected_hello: Hello = serde_json::from_str(&read_fixture("hello.json")).unwrap();
    assert_eq!(
        expect_event(&rx, "Connected after reconnect"),
        WaslEvent::Connected {
            hello: expected_hello
        }
    );

    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
    assert_eq!(server.events_connection_count(), 2);
}

#[test]
fn hello_with_wrong_api_version_is_dropped_then_client_reconnects() {
    let server = MockServer::start();

    let mut bad_hello = val(&read_fixture("hello.json"));
    bad_hello["api_version"] = serde_json::json!(2);
    let bad_hello_data = serde_json::to_string(&bad_hello).unwrap();

    server.push_events_script(
        EventsScript::new(sse_frame("hello", None, &bad_hello_data)).drop_after(true),
    );

    let good_hello_data = compact_fixture("hello.json");
    server.push_events_script(EventsScript::new(sse_frame(
        "hello",
        None,
        &good_hello_data,
    )));
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));

    let mut client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let handle = thread::spawn(move || client.run(tx, stop_thread));

    match expect_event(&rx, "Disconnected for the api_version:2 connection") {
        WaslEvent::Disconnected { .. } => {}
        other => panic!("expected Disconnected for the api_version mismatch, got {other:?}"),
    }
    let expected_hello: Hello = serde_json::from_str(&read_fixture("hello.json")).unwrap();
    assert_eq!(
        expect_event(&rx, "Connected after reconnect"),
        WaslEvent::Connected {
            hello: expected_hello
        }
    );

    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
    assert_eq!(server.events_connection_count(), 2);
}

// ---------------------------------------------------------------------------
// Chunked vs Content-Length body variants (§2.3): /v1/gates served either
// way must parse identically.
// ---------------------------------------------------------------------------

fn run_connect_resnapshot_with(gates_response: BufferedResponse) -> WaslEvent {
    let server = MockServer::start();
    server.push_gates_response(gates_response);
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

    let _connected = expect_event(&rx, "Connected");
    let synthesized = expect_event(&rx, "synthesized GateRequested");

    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
    synthesized
}

#[test]
fn gates_snapshot_content_length_and_chunked_bodies_parse_identically() {
    let body = compact_fixture("gates_snapshot.json");

    let via_content_length = run_connect_resnapshot_with(BufferedResponse::json(200, &body));
    let via_chunked = run_connect_resnapshot_with(BufferedResponse::chunked_json(200, &body));

    assert_eq!(via_content_length, via_chunked);
    assert!(matches!(via_content_length, WaslEvent::GateRequested(_)));
}

// ---------------------------------------------------------------------------
// Raw (Content-Length-less, non-chunked) streaming body for /v1/events.
// ---------------------------------------------------------------------------

#[test]
fn raw_streaming_events_body_without_chunked_encoding_is_handled() {
    let server = MockServer::start();
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));

    let hello_data = compact_fixture("hello.json");
    server.push_events_script(
        EventsScript::new(sse_frame("hello", None, &hello_data)).chunked(false),
    );

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

    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
}
