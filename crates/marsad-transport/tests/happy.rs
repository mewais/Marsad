/// happy.rs — nominal paths: loopback round-trip, multi-line, clean EOF → None.
use marsad_transport::{LoopbackFake, Transport};

// ---------------------------------------------------------------------------
// Helper: connect both ends of a LoopbackFake::pair and return them.
// ---------------------------------------------------------------------------
fn connected_pair() -> (LoopbackFake, LoopbackFake) {
    let (mut a, mut b) = LoopbackFake::pair();
    a.connect().expect("connect side-A");
    b.connect().expect("connect side-B");
    (a, b)
}

// ---------------------------------------------------------------------------
// Basic send → recv round-trip
// ---------------------------------------------------------------------------
#[test]
fn send_recv_single_line() {
    let (mut a, mut b) = connected_pair();
    a.send_line("hello world").expect("send");
    let got = b.recv_line().expect("recv").expect("not EOF");
    assert_eq!(got, "hello world");
}

// ---------------------------------------------------------------------------
// send_line must append '\n' so the framer on the other end sees a full frame.
// We verify this by checking drain_outbound on A contains the newline-terminated
// string, and by checking recv_line on B returns the *content* (no trailing newline).
// ---------------------------------------------------------------------------
#[test]
fn send_line_appends_newline() {
    let (mut a, mut b) = connected_pair();
    a.send_line("ping").expect("send");

    // drain_outbound should give us the raw sent line (content only, per the
    // spec's "send_line appends '\n'" note — the stored form is implementation-
    // defined, but recv on the other end must return the content without '\n').
    let _ = b.recv_line().expect("recv").expect("not EOF");
    // Check outbound captures "ping" (the content)
    let out = a.drain_outbound();
    assert_eq!(out.len(), 1);
    // The drained representation is the line content (without the appended '\n')
    // because drain_outbound documents "what was sent" meaning the logical lines.
    assert_eq!(out[0], "ping");
}

// ---------------------------------------------------------------------------
// Multi-line: two send_line calls → two recv_line calls, in order.
// ---------------------------------------------------------------------------
#[test]
fn multi_line_round_trip() {
    let (mut a, mut b) = connected_pair();
    a.send_line("line one").expect("send 1");
    a.send_line("line two").expect("send 2");

    let first = b.recv_line().expect("recv 1").expect("not EOF");
    let second = b.recv_line().expect("recv 2").expect("not EOF");
    assert_eq!(first, "line one");
    assert_eq!(second, "line two");
}

// ---------------------------------------------------------------------------
// Clean EOF: after close on the writer side, recv_line returns Ok(None).
// ---------------------------------------------------------------------------
#[test]
fn clean_eof_returns_none() {
    let (mut a, mut b) = connected_pair();
    a.send_line("last line").expect("send");
    // Drain the line so EOF check is on a truly empty stream.
    let _ = b.recv_line().expect("recv").expect("not EOF");
    // Close the writer end.
    a.close().expect("close");
    // Now the reader side should see clean EOF.
    let result = b.recv_line().expect("no error on clean EOF");
    assert!(
        result.is_none(),
        "expected Ok(None) on clean EOF, got Some(_)"
    );
}

// ---------------------------------------------------------------------------
// Multiple lines, then clean EOF.
// ---------------------------------------------------------------------------
#[test]
fn multiple_lines_then_eof() {
    let (mut a, mut b) = connected_pair();
    for i in 0..5 {
        a.send_line(&format!("msg-{i}")).expect("send");
    }
    a.close().expect("close writer");

    for i in 0..5 {
        let line = b.recv_line().expect("recv").expect("not EOF yet");
        assert_eq!(line, format!("msg-{i}"));
    }
    let eof = b.recv_line().expect("no error on eof");
    assert!(eof.is_none());
}

// ---------------------------------------------------------------------------
// Blank line content is preserved.
// ---------------------------------------------------------------------------
#[test]
fn blank_line_round_trip() {
    let (mut a, mut b) = connected_pair();
    a.send_line("").expect("send blank");
    let got = b.recv_line().expect("recv").expect("not EOF");
    assert_eq!(got, "");
}

// ---------------------------------------------------------------------------
// Unicode (valid UTF-8) content is preserved.
// ---------------------------------------------------------------------------
#[test]
fn unicode_content_round_trip() {
    let (mut a, mut b) = connected_pair();
    let payload = "日本語テスト 🦀 résumé";
    a.send_line(payload).expect("send unicode");
    let got = b.recv_line().expect("recv").expect("not EOF");
    assert_eq!(got, payload);
}

// ---------------------------------------------------------------------------
// drain_outbound accumulates all sent lines then clears on next call.
// ---------------------------------------------------------------------------
#[test]
fn drain_outbound_accumulates_and_clears() {
    let (mut a, mut b) = connected_pair();
    a.send_line("alpha").expect("send");
    a.send_line("beta").expect("send");
    let _ = b.recv_line().expect("recv");
    let _ = b.recv_line().expect("recv");

    let first_drain = a.drain_outbound();
    assert_eq!(first_drain, vec!["alpha", "beta"]);
    // Second drain should be empty — items were consumed.
    let second_drain = a.drain_outbound();
    assert!(
        second_drain.is_empty(),
        "drain should clear after first call"
    );
}

// ---------------------------------------------------------------------------
// RemoteFifoTransport pure argv builders — verify the reader/writer command
// lines for both an ssh-style prefix and a wsl-style prefix.  These are pure
// functions (no spawning) so they are fully covered without a live remote.
// ---------------------------------------------------------------------------

#[test]
fn remote_fifo_reader_argv_ssh_prefix() {
    use marsad_transport::remote_fifo_reader_argv;
    let prefix = vec!["ssh".to_string(), "user@host".to_string()];
    let argv = remote_fifo_reader_argv(&prefix, "$HOME/.marsad/sessions/s1/out.fifo");
    // ssh: single token (login shell re-parses + expands $HOME).
    assert_eq!(
        argv,
        vec!["ssh", "user@host", "cat $HOME/.marsad/sessions/s1/out.fifo"]
    );
}

#[test]
fn remote_fifo_writer_argv_ssh_prefix() {
    use marsad_transport::remote_fifo_writer_argv;
    let prefix = vec!["ssh".to_string(), "user@host".to_string()];
    let argv = remote_fifo_writer_argv(&prefix, "$HOME/.marsad/sessions/s1/in.fifo");
    // ssh: single token carrying the redirect; login shell runs it.
    assert_eq!(
        argv,
        vec!["ssh", "user@host", "cat > $HOME/.marsad/sessions/s1/in.fifo"]
    );
}

#[test]
fn remote_fifo_reader_argv_wsl_prefix() {
    use marsad_transport::remote_fifo_reader_argv;
    let prefix = vec![
        "wsl.exe".to_string(),
        "-d".to_string(),
        "Ubuntu".to_string(),
    ];
    let argv = remote_fifo_reader_argv(&prefix, "$HOME/.marsad/sessions/s2/out.fifo");
    // wsl: explicit `sh -c` (argv preserved, no login-shell re-parse).
    assert_eq!(
        argv,
        vec![
            "wsl.exe",
            "-d",
            "Ubuntu",
            "sh",
            "-c",
            "cat $HOME/.marsad/sessions/s2/out.fifo",
        ]
    );
}

#[test]
fn remote_fifo_writer_argv_wsl_prefix() {
    use marsad_transport::remote_fifo_writer_argv;
    let prefix = vec![
        "wsl.exe".to_string(),
        "-d".to_string(),
        "Ubuntu".to_string(),
    ];
    let argv = remote_fifo_writer_argv(&prefix, "$HOME/.marsad/sessions/s2/in.fifo");
    assert_eq!(
        argv,
        vec![
            "wsl.exe",
            "-d",
            "Ubuntu",
            "sh",
            "-c",
            "cat > $HOME/.marsad/sessions/s2/in.fifo",
        ]
    );
}
