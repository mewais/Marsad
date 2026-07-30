//! real_attach.rs — black-box tests for `RealWaslSessionClient::attach()`
//! (`GET /v1/targets/{id}/events`, SSE; design §3, §4.2, §4.4). Asserts the
//! real SSE parser:
//!   * consumes the connection-local `hello` handshake frame (no `id:`), does
//!     NOT surface it as a stream event, and continues into the live stream;
//!   * maps each shipped `turn_event_*.json` payload to the correct
//!     `WaslSessionEventKind`, carrying the v3 fields (turn_id, source,
//!     certainty, native detail, …), tagging each `Event` with its SSE `id:` seq;
//!   * surfaces `truncated.json` as a distinct `WaslStreamItem::Truncated
//!     { earliest, latest }` — NEVER an Event, NEVER folded into `Unknown`;
//!   * passes `after_seq` on the request line; ignores keep-alive comments and
//!     unknown event/`type` values (tolerant-skip);
//!   * ends cleanly (`next()` → None) when the stream closes.
//!
//! Loopback-real tier. RED until `RealWaslSessionClient` exists.

mod real_support;

use marsad_wasl_session::{
    RealWaslSessionProvider, WaslSessionClient, WaslSessionClientConfig, WaslSessionEventKind,
    WaslSessionProvider, WaslStreamItem, WaslTargetKey,
};

use real_support::{
    compact_fixture, sse_comment_line, sse_frame, val, BufferedResponse, EventsScript,
    MockRuntimeServer, SSE_BLANK,
};

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

/// Drain a `Box<dyn WaslEventStream>` to clean end-of-stream.
fn drain(stream: &mut dyn marsad_wasl_session::WaslEventStream) -> Vec<WaslStreamItem> {
    let mut out = Vec::new();
    while let Some(item) = stream.next() {
        out.push(item);
    }
    out
}

/// The `Event` kinds only, unwrapping `WaslStreamItem::Event`; panics on a
/// `Truncated` (callers expecting one use `drain` directly), so a truncation is
/// never silently folded into the event list.
fn events(items: Vec<WaslStreamItem>) -> Vec<(u64, WaslSessionEventKind)> {
    items
        .into_iter()
        .map(|it| match it {
            WaslStreamItem::Event(ev) => (ev.seq, ev.kind),
            WaslStreamItem::Truncated { earliest, latest } => {
                panic!("unexpected Truncated{{{earliest},{latest}}} — use drain() when expected")
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// hello handshake + a happy turn's events, each mapped to the right kind.
// ---------------------------------------------------------------------------

#[test]
fn attach_consumes_hello_then_maps_turn_events_with_seq() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);

    let mut lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    // A keep-alive comment mid-stream must be ignored.
    lines.push(sse_comment_line("ka"));
    lines.push(SSE_BLANK.to_string());
    lines.extend(sse_frame(
        "turn_event",
        Some(13),
        &compact_fixture("turn_event_turn_accepted.json"),
    ));
    lines.extend(sse_frame(
        "turn_event",
        Some(14),
        &compact_fixture("turn_event_assistant_delta.json"),
    ));
    lines.extend(sse_frame(
        "turn_event",
        Some(15),
        &compact_fixture("turn_event_turn_completed.json"),
    ));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(12);
    let got = events(drain(&mut *stream));

    // The hello frame is NOT surfaced as an event; exactly the three turn_events
    // are, each tagged with its SSE id: seq.
    assert_eq!(got.len(), 3, "hello is consumed, not surfaced: {got:?}");

    assert_eq!(got[0].0, 13);
    assert_eq!(
        got[0].1,
        WaslSessionEventKind::TurnAccepted {
            turn_id: "turn-7".to_string(),
            source: "local_human".to_string(),
        }
    );

    assert_eq!(got[1].0, 14);
    assert_eq!(
        got[1].1,
        WaslSessionEventKind::AssistantDelta {
            turn_id: "turn-7".to_string(),
            text: "PON".to_string(),
        }
    );

    assert_eq!(got[2].0, 15);
    assert_eq!(
        got[2].1,
        WaslSessionEventKind::TurnCompleted {
            turn_id: "turn-7".to_string(),
            reply: "PONG".to_string(),
            certainty: "completed".to_string(),
        }
    );
}

#[test]
fn attach_sends_after_seq_on_the_request_line() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);

    let lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(7);
    let _ = drain(&mut *stream);

    // The subscribe carries ?after_seq=7 (replay-then-live from EventSeq(8)).
    let subs = server.requests_with_path_prefix("GET", "/v1/targets/tgt-1/events");
    assert_eq!(subs.len(), 1, "one events subscription");
    assert!(
        subs[0].path.contains("after_seq=7"),
        "attach(7) must request ?after_seq=7, got path {:?}",
        subs[0].path
    );
}

// ---------------------------------------------------------------------------
// Each shipped turn_event_*.json maps to the correct kind (richer v3 vocab).
// ---------------------------------------------------------------------------

/// Run one turn_event fixture through attach and return the single kind it maps to.
fn single_event_kind(fixture: &str) -> WaslSessionEventKind {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    let mut lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    lines.extend(sse_frame("turn_event", Some(20), &compact_fixture(fixture)));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(19);
    let got = events(drain(&mut *stream));
    assert_eq!(got.len(), 1, "{fixture} produced one event: {got:?}");
    got.into_iter().next().unwrap().1
}

#[test]
fn attach_maps_session_ready() {
    assert_eq!(
        single_event_kind("turn_event_session_ready.json"),
        WaslSessionEventKind::SessionReady {
            native: Some("ses_07abc123".to_string()),
        }
    );
}

#[test]
fn attach_maps_tool_started() {
    assert_eq!(
        single_event_kind("turn_event_tool_started.json"),
        WaslSessionEventKind::ToolStarted {
            turn_id: "turn-7".to_string(),
            name: "ask_wasl".to_string(),
        }
    );
}

#[test]
fn attach_maps_native_gate_requested_with_detail_only() {
    // The real NativeGateDetail is {summary, preview} — no gate_id, no tool.
    assert_eq!(
        single_event_kind("turn_event_native_gate_requested.json"),
        WaslSessionEventKind::NativeGateRequested {
            turn_id: "turn-7".to_string(),
            detail: marsad_wasl_session::WaslNativeGateDetail {
                summary: "write access".to_string(),
                preview: "rm -rf /tmp/x".to_string(),
            },
        }
    );
}

#[test]
fn attach_maps_turn_failed_with_certainty() {
    assert_eq!(
        single_event_kind("turn_event_turn_failed.json"),
        WaslSessionEventKind::TurnFailed {
            turn_id: "turn-7".to_string(),
            error: "boom".to_string(),
            certainty: "outcome_unknown".to_string(),
        }
    );
}

#[test]
fn attach_maps_process_exited_to_session_exited() {
    // Wire event is `process_exited{code}`; Marsad's caller-mirror kind is
    // `SessionExited{code}` (the terminal process-exit event).
    assert_eq!(
        single_event_kind("turn_event_process_exited.json"),
        WaslSessionEventKind::SessionExited { code: Some(0) }
    );
}

// ---------------------------------------------------------------------------
// truncated is a DISTINCT stream item — never an Event, never Unknown.
// ---------------------------------------------------------------------------

#[test]
fn attach_surfaces_truncated_frame_distinctly() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);

    let mut lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    // A real event, then a truncated control frame (no id:), then another event.
    lines.extend(sse_frame(
        "turn_event",
        Some(9),
        &compact_fixture("turn_event_assistant_delta.json"),
    ));
    lines.extend(sse_frame(
        "truncated",
        None,
        &compact_fixture("truncated.json"),
    ));
    lines.extend(sse_frame(
        "turn_event",
        Some(21),
        &compact_fixture("turn_event_turn_completed.json"),
    ));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(8);
    let items = drain(&mut *stream);
    assert_eq!(items.len(), 3, "event, truncated, event: {items:?}");

    // First item: a normal event.
    assert!(matches!(items[0], WaslStreamItem::Event(_)));

    // Second item: the truncated control frame, surfaced DISTINCTLY — earliest 8,
    // latest 20 per truncated.json — NOT an Event and NOT an Unknown event.
    match &items[1] {
        WaslStreamItem::Truncated { earliest, latest } => {
            assert_eq!(*earliest, 8);
            assert_eq!(*latest, 20);
        }
        other => panic!("expected a distinct Truncated item, got {other:?}"),
    }

    // Third item: a normal event again (the stream continues after truncation).
    assert!(matches!(items[2], WaslStreamItem::Event(_)));
}

// ---------------------------------------------------------------------------
// Tolerant-skip + clean end.
// ---------------------------------------------------------------------------

#[test]
fn attach_ignores_unknown_event_frame_type() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);

    let mut lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    // An unknown SSE `event:` type a future Wasl might add — tolerantly skipped.
    lines.extend(sse_frame(
        "some_future_frame",
        Some(30),
        r#"{"whatever":true}"#,
    ));
    lines.extend(sse_frame(
        "turn_event",
        Some(31),
        &compact_fixture("turn_event_assistant_delta.json"),
    ));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(29);
    let got = events(drain(&mut *stream));
    // Only the known turn_event surfaced; the unknown frame was skipped, not
    // surfaced as Unknown or errored.
    assert_eq!(got.len(), 1, "unknown frame type is skipped: {got:?}");
    assert_eq!(got[0].0, 31);
}

#[test]
fn attach_ends_cleanly_when_stream_closes() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    // hello then an immediate drop → the stream ends; next() must return None.
    let lines = sse_frame("hello", None, &compact_fixture("hello.json"));
    server.push_events(EventsScript::new(lines).drop_after(true));

    let mut stream = client.attach(12);
    let items = drain(&mut *stream);
    assert!(
        items.is_empty(),
        "only a hello then close → no stream items, got {items:?}"
    );
    // A second next() after end-of-stream stays None (idempotent end).
    assert!(stream.next().is_none());
}
