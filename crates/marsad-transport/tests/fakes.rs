/// fakes.rs — exercise the Transport trait contract via LoopbackFake.
///
/// Covers: connect/send_line/recv_line/close ordering; pair() wiring;
/// push_inbound delivering partial frames; drain_outbound capturing sent lines.
use marsad_transport::{LoopbackFake, Transport, TransportError};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn both_connected() -> (LoopbackFake, LoopbackFake) {
    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().expect("connect a");
    b.connect().expect("connect b");
    (a, b)
}

// ---------------------------------------------------------------------------
// 1. pair() produces two distinct ends that communicate.
// ---------------------------------------------------------------------------

#[test]
fn pair_wires_two_ends() {
    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().expect("a.connect");
    b.connect().expect("b.connect");

    a.send_line("from-a").expect("a.send");
    let received_by_b = b.recv_line().expect("b.recv").expect("not EOF");
    assert_eq!(received_by_b, "from-a");

    b.send_line("from-b").expect("b.send");
    let received_by_a = a.recv_line().expect("a.recv").expect("not EOF");
    assert_eq!(received_by_a, "from-b");
}

// ---------------------------------------------------------------------------
// 2. connect → send_line → recv_line → close: canonical lifecycle.
// ---------------------------------------------------------------------------

#[test]
fn canonical_lifecycle() {
    let (mut a, mut b) = both_connected();
    a.send_line("payload").expect("send");
    let line = b.recv_line().expect("recv").expect("not EOF");
    assert_eq!(line, "payload");
    a.close().expect("a.close");
    b.close().expect("b.close");
}

// ---------------------------------------------------------------------------
// 3. close() on one end causes the other to see Ok(None) on recv_line.
// ---------------------------------------------------------------------------

#[test]
fn close_signals_eof_to_peer() {
    let (mut a, mut b) = both_connected();
    a.close().expect("a.close");
    let eof = b.recv_line().expect("no error on peer close");
    assert!(eof.is_none(), "peer close must give Ok(None), got {eof:?}");
}

// ---------------------------------------------------------------------------
// 4. push_inbound delivers a complete frame to recv_line.
// ---------------------------------------------------------------------------

#[test]
fn push_inbound_complete_frame() {
    let (mut _a, mut b) = both_connected();
    // Inject a fully-terminated line into b's inbound.
    b.push_inbound("injected-line\n");
    let got = b.recv_line().expect("recv").expect("not EOF");
    assert_eq!(got, "injected-line");
}

// ---------------------------------------------------------------------------
// 5. push_inbound with a partial frame — recv_line must block / return nothing
//    until the frame is complete.  We test by injecting a second push that
//    completes the frame.
// ---------------------------------------------------------------------------

#[test]
fn push_inbound_partial_then_complete() {
    let (mut _a, mut b) = both_connected();

    // Partial — no newline.
    b.push_inbound("partial");
    // NOTE(test-agent): the spec does not guarantee that recv_line is
    // non-blocking on the LoopbackFake; a conforming implementation may park
    // internally.  We test by completing the frame in a second push_inbound
    // before calling recv_line.
    b.push_inbound("-complete\n");

    let got = b.recv_line().expect("recv").expect("not EOF");
    assert_eq!(got, "partial-complete");
}

// ---------------------------------------------------------------------------
// 6. drain_outbound captures every sent line in order.
// ---------------------------------------------------------------------------

#[test]
fn drain_outbound_order() {
    let (mut a, mut b) = both_connected();
    a.send_line("one").expect("send");
    a.send_line("two").expect("send");
    a.send_line("three").expect("send");
    // Consume on the receive side so the fake can buffer correctly.
    for _ in 0..3 {
        let _ = b.recv_line().expect("recv").expect("not EOF");
    }
    let drained = a.drain_outbound();
    assert_eq!(drained, vec!["one", "two", "three"]);
}

// ---------------------------------------------------------------------------
// 7. drain_outbound is destructive — second call returns empty.
// ---------------------------------------------------------------------------

#[test]
fn drain_outbound_is_destructive() {
    let (mut a, mut b) = both_connected();
    a.send_line("x").expect("send");
    let _ = b.recv_line().expect("recv").expect("not EOF");
    let first = a.drain_outbound();
    assert_eq!(first, vec!["x"]);
    let second = a.drain_outbound();
    assert!(second.is_empty(), "second drain must be empty");
}

// ---------------------------------------------------------------------------
// 8. send_line does not include the '\n' in drain_outbound output (logical lines).
// ---------------------------------------------------------------------------

#[test]
fn drain_outbound_no_newline_suffix() {
    let (mut a, mut b) = both_connected();
    a.send_line("check-newline").expect("send");
    let _ = b.recv_line().expect("recv").expect("not EOF");
    let drained = a.drain_outbound();
    assert_eq!(drained.len(), 1);
    assert!(
        !drained[0].ends_with('\n'),
        "drain_outbound should not include the appended newline"
    );
    assert_eq!(drained[0], "check-newline");
}

// ---------------------------------------------------------------------------
// 9. push_inbound can deliver multiple complete frames at once.
// ---------------------------------------------------------------------------

#[test]
fn push_inbound_multiple_frames_at_once() {
    let (mut _a, mut b) = both_connected();
    b.push_inbound("alpha\nbeta\ngamma\n");

    let l1 = b.recv_line().expect("recv 1").expect("not EOF");
    let l2 = b.recv_line().expect("recv 2").expect("not EOF");
    let l3 = b.recv_line().expect("recv 3").expect("not EOF");
    assert_eq!(
        (l1.as_str(), l2.as_str(), l3.as_str()),
        ("alpha", "beta", "gamma")
    );
}

// ---------------------------------------------------------------------------
// 10. send_line before connect must not panic (error is acceptable).
// ---------------------------------------------------------------------------

#[test]
fn send_before_connect_no_panic() {
    let (mut a, _b) = LoopbackFake::pair();
    let _ = a.send_line("too-early"); // must not panic
}

// ---------------------------------------------------------------------------
// 11. recv_line before connect must not panic (error is acceptable).
// ---------------------------------------------------------------------------

#[test]
fn recv_before_connect_no_panic() {
    let (_a, mut b) = LoopbackFake::pair();
    let _ = b.recv_line(); // must not panic
}

// ---------------------------------------------------------------------------
// 12. close before connect must not panic.
// ---------------------------------------------------------------------------

#[test]
fn close_before_connect_no_panic() {
    let (mut a, _b) = LoopbackFake::pair();
    let _ = a.close(); // must not panic
}

// ---------------------------------------------------------------------------
// 13. Transport is object-safe — can be boxed as dyn Transport.
// ---------------------------------------------------------------------------

#[test]
fn transport_is_object_safe() {
    let (a, _b) = LoopbackFake::pair();
    let _boxed: Box<dyn Transport> = Box::new(a);
    // The mere existence of this code proves object safety.
}

// ---------------------------------------------------------------------------
// 14. Multiple connect calls are idempotent (or at least do not panic).
// ---------------------------------------------------------------------------

#[test]
fn double_connect_no_panic() {
    let (mut a, _b) = LoopbackFake::pair();
    let _ = a.connect();
    let _ = a.connect(); // second connect must not panic
}

// ---------------------------------------------------------------------------
// 15. Multiple close calls are idempotent (or at least do not panic).
// ---------------------------------------------------------------------------

#[test]
fn double_close_no_panic() {
    let (mut a, _b) = LoopbackFake::pair();
    let _ = a.connect();
    let _ = a.close();
    let _ = a.close(); // second close must not panic
}

// ---------------------------------------------------------------------------
// 16. ClosedMidStream surfaces when push_inbound has a partial frame and
//     the writer side is closed.
// ---------------------------------------------------------------------------

#[test]
fn push_inbound_partial_then_eof_is_closed_mid_stream() {
    let (mut a, mut b) = both_connected();
    b.push_inbound("no-newline-at-end");
    a.close().expect("close writer");
    let err = b.recv_line().expect_err("expected ClosedMidStream");
    assert!(
        matches!(err, TransportError::ClosedMidStream),
        "expected ClosedMidStream, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 17. drain_outbound before any sends returns empty.
// ---------------------------------------------------------------------------

#[test]
fn drain_outbound_empty_initially() {
    let (mut a, _b) = LoopbackFake::pair();
    a.connect().expect("connect");
    let drained = a.drain_outbound();
    assert!(
        drained.is_empty(),
        "drain_outbound before any sends must be empty"
    );
}
