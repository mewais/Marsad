//! WAVE 3b — pty escape-hatch terminal: REAL pty spawn/IO tests.
//!
//! Unlike `TmuxContainer`/`ClaudeFifoTransport` (real IO gated the same way
//! but only exercised by the live `i03` integration test — they need a real
//! `tmux` + `claude`), spawning a local pty running a plain shell has no
//! external dependency beyond a pty device + a shell binary, both present in
//! this sandbox. So this IS genuinely test-first-verified here, driving the
//! real `spawn_pty`/`resize_pty`/`PtyEntry` API (`marsad_agent`'s public
//! re-export of `pty.rs`'s `#[cfg(not(coverage))]` surface) directly:
//! spawn a shell, write a command, assert its echoed output arrives, resize,
//! and exit.
//!
//! Scope note: this exercises the NOVEL real-IO surface (`spawn_pty`,
//! `resize_pty`) directly — the surrounding Agent-registry/broadcast glue
//! (`pty::spawn_pty_session`/`pump_pty_output`, `ipc.rs`'s
//! `handle_pty_start`/`handle_pty_input`/`handle_pty_resize`) is `pub(crate)`
//! (not reachable from this black-box test crate) and mirrors
//! already-proven patterns elsewhere in this crate (`session_writers`/
//! `pending_gates`); it is not independently wire-tested here, same
//! disposition as `serve()`'s own real-socket glue (needs a live process to
//! observe, see `docs/GAPS.md`).
//!
//! `#![cfg(not(coverage))]`: the `spawn_pty`/`resize_pty`/`PtyEntry` symbols
//! this file calls only exist under `#[cfg(not(coverage))]` (see
//! `crates/marsad-agent/src/pty.rs`'s module doc) — under `cargo llvm-cov`
//! (which sets `--cfg coverage`) this attribute makes the WHOLE file compile
//! to zero tests instead of a build error (verified: `#![cfg(not(x))]` at a
//! test-binary's crate root strips its entire content cleanly).

#![cfg(not(coverage))]

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use marsad_agent::{resize_pty, spawn_pty, PtyError};

/// Pump `reader` on a background thread, forwarding each chunk (lossy UTF-8,
/// fine for shell echo text) through an `mpsc` channel so the test can
/// `recv_timeout` instead of risking an indefinite block if something in the
/// pty/child never produces the expected output.
fn spawn_output_pump(mut reader: Box<dyn Read + Send>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if tx.send(chunk).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// Read from `rx` until `needle` has appeared `occurrences` times in the
/// concatenated output, or `timeout` elapses (in which case it panics with
/// whatever was collected — a clear failure, never a silent hang).
///
/// `occurrences` matters because a pty ECHOES typed input back verbatim
/// before the shell even processes it — a command containing `needle` in
/// its own text (e.g. `echo needle`) produces ONE match from that echo
/// alone, before the shell has actually run anything. Callers that want to
/// prove the shell's REAL output (not just terminal echo of the input) must
/// wait for a second occurrence.
fn wait_for_output(
    rx: &mpsc::Receiver<String>,
    needle: &str,
    occurrences: usize,
    timeout: Duration,
) -> String {
    let deadline = std::time::Instant::now() + timeout;
    let mut collected = String::new();
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(remaining.min(Duration::from_millis(200))) {
            Ok(chunk) => {
                collected.push_str(&chunk);
                if collected.matches(needle).count() >= occurrences {
                    return collected;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    panic!(
        "timed out waiting for {occurrences}x {needle:?} in pty output; collected so far: {collected:?}"
    );
}

// ---------------------------------------------------------------------------
// spawn_pty — happy path: echo roundtrip
// ---------------------------------------------------------------------------

#[test]
fn spawn_local_shell_and_echo_command_roundtrips_through_the_real_pty() {
    let (mut entry, reader) =
        spawn_pty(&["/bin/sh".to_string()], None, 80, 24).expect("spawn_pty should succeed");
    let rx = spawn_output_pump(reader);

    // A fresh shell doesn't necessarily print a prompt before the first
    // write lands, so just write straight away — the pty faithfully echoes
    // input back (this IS a real pty, not stream-json mode).
    entry
        .writer
        .write_all(b"echo marsad_pty_roundtrip_marker\n")
        .expect("write to pty stdin should succeed");
    entry.writer.flush().expect("flush should succeed");

    // 2 occurrences: the pty's own echo of the typed command, THEN the
    // shell's real `echo` output — proving the command actually ran, not
    // just that the pty echoed our keystrokes back.
    let output = wait_for_output(
        &rx,
        "marsad_pty_roundtrip_marker",
        2,
        Duration::from_secs(5),
    );
    assert!(
        output.contains("marsad_pty_roundtrip_marker"),
        "expected the echoed marker in pty output, got: {output:?}"
    );

    // Clean teardown.
    entry
        .writer
        .write_all(b"exit\n")
        .expect("write exit should succeed");
    let _ = entry.child.wait();
}

// ---------------------------------------------------------------------------
// spawn_pty — cwd plumbing
// ---------------------------------------------------------------------------

#[test]
fn spawn_pty_honors_the_requested_working_directory() {
    let tmp = std::env::temp_dir();
    // Resolve to the canonical form up front (macOS/BSD symlink /tmp → /private/tmp,
    // and the shell's own `pwd` may report either — compare against what the
    // shell itself resolves).
    let (mut entry, reader) = spawn_pty(
        &["/bin/sh".to_string()],
        Some(tmp.to_str().expect("tmp dir must be valid UTF-8")),
        80,
        24,
    )
    .expect("spawn_pty should succeed");
    let rx = spawn_output_pump(reader);

    entry
        .writer
        .write_all(b"echo MARSAD_CWD_MARKER; pwd\n")
        .expect("write should succeed");
    entry.writer.flush().expect("flush should succeed");

    // 2 occurrences: the pty's echo of the typed command (which itself
    // contains the literal text "MARSAD_CWD_MARKER"), THEN the shell's
    // real output line.
    let output = wait_for_output(&rx, "MARSAD_CWD_MARKER", 2, Duration::from_secs(5));
    // The shell's cwd should resolve to the same directory we asked for (compare
    // canonicalized paths so a symlinked tmp dir like macOS's doesn't false-fail).
    let canon_tmp = std::fs::canonicalize(&tmp)
        .unwrap_or_else(|_| tmp.clone())
        .to_string_lossy()
        .into_owned();
    assert!(
        output.contains(&canon_tmp) || output.contains(tmp.to_str().unwrap()),
        "expected pwd output to contain the requested cwd {tmp:?}, got: {output:?}"
    );

    let _ = entry.child.kill();
    let _ = entry.child.wait();
}

// ---------------------------------------------------------------------------
// spawn_pty — error paths
// ---------------------------------------------------------------------------

#[test]
fn spawn_pty_empty_argv_is_a_spawn_error() {
    // `(PtyEntry, Box<dyn Read + Send>)` isn't `Debug` (trait objects
    // aren't), so `expect_err`/`unwrap_err` don't apply here — match the
    // `Result` directly instead.
    match spawn_pty(&[], None, 80, 24) {
        Err(PtyError::Spawn(msg)) => assert!(msg.contains("empty argv"), "got: {msg}"),
        Err(other) => panic!("expected PtyError::Spawn, got {other:?}"),
        Ok(_) => panic!("empty argv must fail"),
    }
}

#[test]
fn spawn_pty_nonexistent_binary_is_a_spawn_error() {
    match spawn_pty(
        &["/marsad/definitely/not/a/real/binary-xyz".to_string()],
        None,
        80,
        24,
    ) {
        Err(PtyError::Spawn(_)) => {}
        Err(other) => panic!("expected PtyError::Spawn, got {other:?}"),
        Ok(_) => panic!("spawning a nonexistent binary must fail"),
    }
}

#[test]
fn pty_error_display_is_human_readable() {
    let spawn = PtyError::Spawn("boom".to_string());
    let io = PtyError::Io("boom2".to_string());
    assert!(spawn.to_string().contains("boom"));
    assert!(io.to_string().contains("boom2"));
}

// ---------------------------------------------------------------------------
// resize_pty
// ---------------------------------------------------------------------------

#[test]
fn resize_pty_updates_a_live_ptys_viewport() {
    let (mut entry, _reader) =
        spawn_pty(&["/bin/sh".to_string()], None, 80, 24).expect("spawn_pty should succeed");

    resize_pty(&entry, 120, 40).expect("resize should succeed on a live pty");

    let _ = entry.child.kill();
    let _ = entry.child.wait();
}

// ---------------------------------------------------------------------------
// Exit — EOF + exit status
// ---------------------------------------------------------------------------

#[test]
fn shell_exit_produces_eof_and_a_successful_exit_status() {
    let (mut entry, reader) =
        spawn_pty(&["/bin/sh".to_string()], None, 80, 24).expect("spawn_pty should succeed");

    entry
        .writer
        .write_all(b"exit 0\n")
        .expect("write exit should succeed");
    entry.writer.flush().expect("flush should succeed");

    // Drain the reader to EOF (Ok(0)) with a bound, proving the pty tears
    // down cleanly rather than hanging forever.
    let mut r = reader;
    let mut buf = [0u8; 4096];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "pty reader never reached EOF after `exit 0`"
        );
        match r.read(&mut buf) {
            Ok(0) => break, // EOF — the shell exited and closed the pty.
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    let status = entry.child.wait().expect("wait should succeed after EOF");
    assert!(status.success(), "expected a clean exit, got {status:?}");
    assert_eq!(status.exit_code(), 0);
}

#[test]
fn shell_nonzero_exit_is_reported_as_a_failed_status() {
    let (mut entry, reader) =
        spawn_pty(&["/bin/sh".to_string()], None, 80, 24).expect("spawn_pty should succeed");

    entry
        .writer
        .write_all(b"exit 7\n")
        .expect("write exit should succeed");
    entry.writer.flush().expect("flush should succeed");

    let mut r = reader;
    let mut buf = [0u8; 4096];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "pty reader never reached EOF after `exit 7`"
        );
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    let status = entry.child.wait().expect("wait should succeed after EOF");
    assert!(!status.success());
    assert_eq!(status.exit_code(), 7);
}
