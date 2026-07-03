//! malformed_frames.rs — regression coverage for review finding #3
//! ([MEDIUM] a malformed Wasl SSE frame silently kills a host's connector
//! via panic): `client.rs` used `.expect()` on every known-frame parse
//! (hello, gate_requested, gate_decided, audit, gates_snapshot) and on the
//! post-hello `id:` presence, all under a "trusted v1 node" assumption. A
//! single malformed frame from a buggy/compromised node panicked the
//! connector's dedicated thread SILENTLY — bypassing the existing
//! `Err` -> `Disconnected` -> backoff -> reconnect path entirely (the
//! `run()` loop's `Err(e) => events.send(Disconnected{..})` arm never runs
//! for a `panic!`, only for a real `Err` return) — so push updates stop
//! forever with no signal, while `handle_answer_wasl_gate` keeps
//! "succeeding" (fresh connection per call for decisions) with no sign
//! anything is wrong.
//!
//! Every scenario below proves: (1) the connector thread does NOT panic
//! (`JoinHandle::join()` returns `Ok`), and (2) a malformed frame instead
//! surfaces as `WaslEvent::Disconnected` and the client reconnects.
mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use marsad_wasl::{Hello, WaslClientConfig, WaslEvent, WaslOperatorClient};

use support::{
    compact_fixture, read_fixture, sse_frame, BufferedResponse, EventsScript, MockServer,
};

const RECV_TIMEOUT: Duration = Duration::from_secs(3);

fn expect_event(rx: &std::sync::mpsc::Receiver<WaslEvent>, what: &str) -> WaslEvent {
    rx.recv_timeout(RECV_TIMEOUT)
        .unwrap_or_else(|e| panic!("expected {what}: {e}"))
}

fn expected_hello() -> Hello {
    serde_json::from_str(&read_fixture("hello.json")).unwrap()
}

/// Shared scenario shape: script a first connection that emits SOME
/// malformed content, followed by a clean second connection (the
/// reconnect), then assert: Disconnected observed, reconnect succeeds, and
/// — the crux of this regression test — the connector thread never panics.
fn assert_malformed_input_reconnects_without_panicking(
    first_connection_lines: Vec<String>,
    first_gates_response: BufferedResponse,
) {
    let server = MockServer::start();
    server.push_gates_response(first_gates_response);
    server.push_events_script(EventsScript::new(first_connection_lines));

    // Second connection (the reconnect): clean hello, held open.
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));
    server.push_events_script(EventsScript::new(sse_frame(
        "hello",
        None,
        &compact_fixture("hello.json"),
    )));

    let mut client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let handle = thread::spawn(move || client.run(tx, stop_thread));

    // Drain events until Disconnected shows up (a well-formed hello may or
    // may not have been emitted first, depending on which scenario this is
    // — some scenarios' malformation IS the hello frame itself).
    let mut saw_disconnected = false;
    for _ in 0..4 {
        match expect_event(&rx, "Disconnected or Connected") {
            WaslEvent::Disconnected { error } => {
                assert!(
                    !error.is_empty(),
                    "Disconnected error message must be non-empty"
                );
                saw_disconnected = true;
                break;
            }
            WaslEvent::Connected { .. } => continue,
            other => panic!("unexpected event while waiting for Disconnected: {other:?}"),
        }
    }
    assert!(
        saw_disconnected,
        "a malformed frame/response must surface as WaslEvent::Disconnected (the existing \
         reconnect path), not silently kill the connector thread"
    );

    // Reconnect: a fresh Connected must follow.
    assert_eq!(
        expect_event(&rx, "Connected after reconnect"),
        WaslEvent::Connected {
            hello: expected_hello()
        }
    );

    stop.store(true, Ordering::SeqCst);
    let join_result = handle.join();
    assert!(
        join_result.is_ok(),
        "the connector thread must NEVER panic on a malformed frame; join() returned: {join_result:?}"
    );
}

#[test]
fn malformed_hello_frame_reconnects_without_panicking() {
    assert_malformed_input_reconnects_without_panicking(
        sse_frame("hello", None, "{not valid json"),
        BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#),
    );
}

#[test]
fn post_hello_frame_missing_id_reconnects_without_panicking() {
    let hello_data = compact_fixture("hello.json");
    let mut lines = sse_frame("hello", None, &hello_data);
    // A `gate_requested` frame with NO `id:` line — the contract says every
    // frame after `hello` carries one; a node that violates this must not
    // panic the connector.
    lines.push("event: gate_requested\n".to_string());
    lines.push(format!(
        "data: {}\n",
        compact_fixture("gate_requested_external.json")
    ));
    lines.push("\n".to_string());

    assert_malformed_input_reconnects_without_panicking(
        lines,
        BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#),
    );
}

#[test]
fn malformed_gate_requested_frame_reconnects_without_panicking() {
    let hello_data = compact_fixture("hello.json");
    let mut lines = sse_frame("hello", None, &hello_data);
    lines.extend(sse_frame("gate_requested", Some(43), "{not valid json"));

    assert_malformed_input_reconnects_without_panicking(
        lines,
        BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#),
    );
}

#[test]
fn malformed_gate_decided_frame_reconnects_without_panicking() {
    let hello_data = compact_fixture("hello.json");
    let mut lines = sse_frame("hello", None, &hello_data);
    lines.extend(sse_frame("gate_decided", Some(43), "{not valid json"));

    assert_malformed_input_reconnects_without_panicking(
        lines,
        BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#),
    );
}

#[test]
fn malformed_audit_frame_reconnects_without_panicking() {
    let hello_data = compact_fixture("hello.json");
    let mut lines = sse_frame("hello", None, &hello_data);
    lines.extend(sse_frame("audit", Some(43), "{not valid json"));

    assert_malformed_input_reconnects_without_panicking(
        lines,
        BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#),
    );
}

#[test]
fn malformed_gates_snapshot_on_connect_resnapshot_reconnects_without_panicking() {
    let hello_data = compact_fixture("hello.json");
    // hello is fine; the CONNECT-TIME resnapshot's `/v1/gates` body is what's
    // malformed this time.
    assert_malformed_input_reconnects_without_panicking(
        sse_frame("hello", None, &hello_data),
        BufferedResponse::json(200, "{not valid json"),
    );
}

/// Same malformed-`/v1/gates`-body failure, but via the OTHER call site: the
/// GAP-triggered resnapshot (step 4, not the connect-time one in step 3) —
/// a distinct source location or two same-`?` call sites can otherwise hide
/// an under-tested branch even when the shared `resnapshot` function body
/// itself is exercised elsewhere.
#[test]
fn malformed_gates_snapshot_on_gap_triggered_resnapshot_reconnects_without_panicking() {
    let server = MockServer::start();

    // Connect-time resnapshot: valid, empty.
    server.push_gates_response(BufferedResponse::json(200, r#"{"last_seq":42,"gates":[]}"#));

    let hello_data = compact_fixture("hello.json");
    let gd_data = compact_fixture("gate_decided_approved_operator.json");
    let mut lines = sse_frame("hello", None, &hello_data);
    lines.extend(sse_frame("gate_decided", Some(43), &gd_data));
    lines.extend(sse_frame("gate_decided", Some(45), &gd_data)); // skips 44 -> gap
    server.push_events_script(EventsScript::new(lines));

    // GAP-triggered resnapshot: malformed body.
    server.push_gates_response(BufferedResponse::json(200, "{not valid json"));

    // Reconnect: clean hello, held open.
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

    assert_eq!(
        expect_event(&rx, "first Connected"),
        WaslEvent::Connected {
            hello: expected_hello()
        }
    );
    // Drain the two well-formed `GateDecided` events (ids 43, 45) before the
    // gap they created triggers the malformed resnapshot.
    let mut saw_disconnected = false;
    for _ in 0..4 {
        match expect_event(&rx, "GateDecided or Disconnected") {
            WaslEvent::GateDecided(_) => continue,
            WaslEvent::Disconnected { error } => {
                assert!(!error.is_empty());
                saw_disconnected = true;
                break;
            }
            other => panic!("unexpected event while waiting for Disconnected: {other:?}"),
        }
    }
    assert!(
        saw_disconnected,
        "a malformed gap-triggered resnapshot must surface as Disconnected, not silently kill \
         the connector thread"
    );
    assert_eq!(
        expect_event(&rx, "Connected after reconnect"),
        WaslEvent::Connected {
            hello: expected_hello()
        }
    );

    stop.store(true, Ordering::SeqCst);
    let join_result = handle.join();
    assert!(
        join_result.is_ok(),
        "the connector thread must NEVER panic on a malformed frame; join() returned: {join_result:?}"
    );
}
