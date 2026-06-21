/// errors.rs — every enumerated error: ConnectFailed, BrokenPipe, ClosedMidStream.
use marsad_transport::{LoopbackFake, Transport, TransportError};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns true if the error is TransportError::ConnectFailed.
fn is_connect_failed(e: &TransportError) -> bool {
    matches!(e, TransportError::ConnectFailed(_))
}

/// Returns true if the error is TransportError::BrokenPipe.
fn is_broken_pipe(e: &TransportError) -> bool {
    matches!(e, TransportError::BrokenPipe)
}

/// Returns true if the error is TransportError::ClosedMidStream.
fn is_closed_mid_stream(e: &TransportError) -> bool {
    matches!(e, TransportError::ClosedMidStream)
}

// ---------------------------------------------------------------------------
// ConnectFailed — calling send_line or recv_line before connect should surface
// a ConnectFailed (or the implementation chooses Io; both are valid — but the
// spec lists ConnectFailed as the error for a failed connection attempt).
//
// For LocalIpc/SshStdio the constructor takes a path/target that does not exist,
// so connect() itself returns ConnectFailed.  For LoopbackFake there is no real
// network, so we use LocalIpc (which can be constructed cheaply) with a
// nonsensical path to drive the ConnectFailed branch.
// ---------------------------------------------------------------------------

#[test]
fn local_ipc_connect_failed_bad_path() {
    // NOTE(test-agent): LocalIpc::new takes a path; we point it at a socket
    // that does not exist to trigger ConnectFailed.
    use marsad_transport::LocalIpc;
    let mut ipc = LocalIpc::new("/nonexistent/path/that/cannot/exist.sock");
    let err = ipc.connect().expect_err("should fail to connect");
    assert!(
        is_connect_failed(&err),
        "expected ConnectFailed, got {err:?}"
    );
}

#[test]
fn ssh_stdio_connect_failed_bad_target() {
    // NOTE(test-agent): SshStdio::new takes a target; we use an unreachable
    // host to trigger ConnectFailed (the OS will refuse or time out immediately
    // for a clearly invalid hostname format).
    use marsad_transport::SshStdio;
    let mut ssh = SshStdio::new(
        "invalid-host-that-does-not-exist-xyz.invalid",
        "marsad-agent",
    );
    let err = ssh.connect().expect_err("should fail to connect");
    assert!(
        is_connect_failed(&err) || matches!(err, TransportError::Io(_)),
        "expected ConnectFailed or Io, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// BrokenPipe — writing to a closed transport should return BrokenPipe.
// We simulate this with LoopbackFake: close the receiver end so the sender
// gets BrokenPipe on its next send_line.
// ---------------------------------------------------------------------------

#[test]
fn broken_pipe_after_peer_close() {
    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().expect("connect a");
    b.connect().expect("connect b");

    // Close receiver side (b) to simulate peer disappearing.
    b.close().expect("close b");

    // Now a's next send should surface BrokenPipe.
    let err = a.send_line("should fail").expect_err("expected BrokenPipe");
    assert!(is_broken_pipe(&err), "expected BrokenPipe, got {err:?}");
}

// ---------------------------------------------------------------------------
// ClosedMidStream — peer drops the connection while a partial frame is still
// buffered (i.e., a line without its terminating '\n').
//
// We use push_inbound to inject a partial frame (no '\n') and then close the
// writer side, so recv_line sees a buffered partial and returns ClosedMidStream.
// ---------------------------------------------------------------------------

#[test]
fn closed_mid_stream_partial_frame_buffered() {
    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().expect("connect a");
    b.connect().expect("connect b");

    // Push a partial frame (no trailing newline) into b's inbound buffer.
    b.push_inbound("incomplete-json-without-newline");

    // Close the writer (a) so b's next recv sees EOF with a buffered partial.
    a.close().expect("close a");

    // recv_line must return Err(ClosedMidStream).
    let err = b.recv_line().expect_err("expected ClosedMidStream");
    assert!(
        is_closed_mid_stream(&err),
        "expected ClosedMidStream, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// ClosedMidStream — variant: partial frame pushed directly via push_inbound,
// then no close — but the inbound is "closed" (no more data), simulating a
// connection that was severed after a partial write.
// ---------------------------------------------------------------------------

#[test]
fn closed_mid_stream_via_push_inbound_then_eof() {
    let (mut _a, mut b) = LoopbackFake::pair();
    // NOTE(test-agent): we use only one end; we push a raw partial line into b
    // and then signal EOF without a newline terminator.  The exact API for
    // "signal EOF without close()" may be implementation-specific; if the impl
    // only exposes close() for this, we simply close _a and rely on the pair
    // wiring to propagate EOF to b's recv side.
    _a.connect().expect("connect a");
    b.connect().expect("connect b");

    b.push_inbound("partial-line");
    _a.close().expect("close a");

    let result = b
        .recv_line()
        .expect_err("expected ClosedMidStream or error");
    assert!(
        is_closed_mid_stream(&result),
        "expected ClosedMidStream, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// TransportError display / debug (all variants are constructible and debug-printable).
// ---------------------------------------------------------------------------

#[test]
fn transport_error_variants_debug() {
    let variants: &[TransportError] = &[
        TransportError::ConnectFailed("test reason".to_string()),
        TransportError::BrokenPipe,
        TransportError::Io("io failure".to_string()),
        TransportError::ClosedMidStream,
    ];
    for v in variants {
        // Must not panic; format checks the Debug impl is present.
        let _ = format!("{v:?}");
    }
}

// ---------------------------------------------------------------------------
// send_line on an unconnected transport should error (not panic).
// ---------------------------------------------------------------------------

#[test]
fn send_before_connect_errors() {
    let (mut a, _b) = LoopbackFake::pair();
    // a is NOT connected.
    let result = a.send_line("premature");
    // Any error variant is acceptable — the key requirement is it does not panic.
    assert!(result.is_err(), "send before connect must return Err");
}

// ---------------------------------------------------------------------------
// recv_line on an unconnected transport should error (not panic).
// ---------------------------------------------------------------------------

#[test]
fn recv_before_connect_errors() {
    let (_a, mut b) = LoopbackFake::pair();
    // b is NOT connected.
    let result = b.recv_line();
    assert!(result.is_err(), "recv before connect must return Err");
}
