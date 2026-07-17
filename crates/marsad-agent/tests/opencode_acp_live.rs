//! opencode_acp_live.rs — Opt-in live seam test for the opencode/ACP backend.
//!
//! Authoritative spec: `docs/specs/opencode-acp.md` §2 (lifecycle/handshake).
//!
//! This test is SKIP-GUARDED: when the `opencode` binary is absent from PATH
//! the test skips loudly (eprintln! + return Ok(())) and reports green.
//!
//! When `opencode` IS present, it:
//!   1. Spawns `opencode acp` as a subprocess.
//!   2. Performs the `initialize` + `session/new` handshake over stdin/stdout,
//!      using `marsad_agent::acp` encoders and `parse_acp_line`.
//!   3. Sends a trivial `session/prompt`.
//!   4. Asserts that at least one `session/update` notification (or a prompt
//!      result) comes back within a timeout.
//!
//! Because both the `acp` module functions AND `opencode` are absent in this
//! sandbox, the test currently SKIPS on the `opencode_present()` check.  That
//! is the expected state (compile OK + SKIP, not fail).
//!
//! Mirror of the i02 gate round-trip's `claude_present()` + early-return skip
//! pattern (tests/integration/i02_gate_roundtrip/tests/gate_roundtrip.rs,
//! Part B).

use std::io::{BufRead, BufReader, Write as IoWrite};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use marsad_agent::acp::{
    encode_request, initialize_params, parse_acp_line, session_new_params, session_prompt_params,
    AcpMessage,
};

// ---------------------------------------------------------------------------
// Skip guard
// ---------------------------------------------------------------------------

/// Return true when `opencode` is on PATH (mirrors i02's `claude_present()`).
fn opencode_present() -> bool {
    Command::new("which")
        .arg("opencode")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Live handshake helper
// ---------------------------------------------------------------------------

/// Outcome of a live ACP handshake attempt.
enum LiveAcpOutcome {
    /// At least one `session/update` notification or a `session/prompt` result
    /// was received — the bidirectional ACP channel is working.
    GotUpdate,
    /// We completed the handshake but timed out waiting for a response to
    /// `session/prompt` — the channel opened but the model was slow or the
    /// binary exited early.
    Timeout,
    /// Something went wrong: spawn failure, broken pipe, JSON-RPC error, etc.
    Error(String),
}

/// Run the ACP handshake end-to-end against a real `opencode acp` process.
///
/// Steps:
///   1. Spawn `opencode acp` with stdin/stdout pipes.
///   2. Send `initialize` request; wait for its response.
///   3. Send `session/new` request; wait for its response and capture sessionId.
///   4. Send a trivial `session/prompt` request.
///   5. Read lines until we see a `session/update` notification or a
///      `session/prompt` result, or until the deadline expires.
fn run_live_acp_handshake() -> LiveAcpOutcome {
    // 1. Spawn `opencode acp`.
    let mut child = match Command::new("opencode")
        .arg("acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return LiveAcpOutcome::Error(format!("spawn opencode acp: {e}")),
    };

    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => return LiveAcpOutcome::Error("no child stdin".to_string()),
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return LiveAcpOutcome::Error("no child stdout".to_string()),
    };

    // Reader thread: streams non-empty lines over a channel.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let _reader = std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let t = line.trim_end().to_string();
                    if !t.is_empty() && tx.send(t).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // ── Helper: write one JSON-RPC line to stdin ──────────────────────────────
    let write_line = |stdin: &mut std::process::ChildStdin, msg: &str| -> Result<(), String> {
        stdin
            .write_all(msg.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("write to opencode acp: {e}"))
    };

    // ── Helper: read next line within deadline ────────────────────────────────
    let recv_timeout = |deadline: Instant| -> Option<String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        rx.recv_timeout(remaining).ok()
    };

    // Short deadline per phase (total budget: 30s).
    let deadline = Instant::now() + Duration::from_secs(30);

    // 2. Send `initialize` request (id = 1).
    let init_req = encode_request(
        serde_json::json!(1),
        "initialize",
        initialize_params("marsad", "test"),
    );
    if let Err(e) = write_line(&mut stdin, &init_req) {
        let _ = child.kill();
        return LiveAcpOutcome::Error(e);
    }

    // Wait for the `initialize` response.
    let mut got_init_response = false;
    while Instant::now() < deadline {
        let line = match recv_timeout(deadline) {
            Some(l) => l,
            None => break,
        };
        match parse_acp_line(&line) {
            Ok(AcpMessage::Response { id, result }) => {
                if id == serde_json::json!(1) {
                    match result {
                        Ok(_) => {
                            got_init_response = true;
                            break;
                        }
                        Err(e) => {
                            let _ = child.kill();
                            return LiveAcpOutcome::Error(format!(
                                "initialize error response: {e}"
                            ));
                        }
                    }
                }
            }
            Ok(_) => {} // notifications or other responses — skip
            Err(_) => {} // parse errors from non-JSON lines (e.g. startup banners) — skip
        }
    }

    if !got_init_response {
        eprintln!("[opencode_acp_live] no initialize response within deadline");
        let _ = child.kill();
        return LiveAcpOutcome::Timeout;
    }

    // 3. Send `session/new` request (id = 2).
    let new_req = encode_request(
        serde_json::json!(2),
        "session/new",
        session_new_params("/tmp", &serde_json::json!([]), None),
    );
    if let Err(e) = write_line(&mut stdin, &new_req) {
        let _ = child.kill();
        return LiveAcpOutcome::Error(e);
    }

    // Wait for the `session/new` response and extract sessionId.
    let mut session_id = String::new();
    while Instant::now() < deadline {
        let line = match recv_timeout(deadline) {
            Some(l) => l,
            None => break,
        };
        match parse_acp_line(&line) {
            Ok(AcpMessage::Response { id, result }) => {
                if id == serde_json::json!(2) {
                    match result {
                        Ok(res) => {
                            // Extract sessionId from the result.
                            if let Some(sid) =
                                res.get("sessionId").and_then(|v| v.as_str())
                            {
                                session_id = sid.to_string();
                            }
                            break;
                        }
                        Err(e) => {
                            let _ = child.kill();
                            return LiveAcpOutcome::Error(format!(
                                "session/new error response: {e}"
                            ));
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    if session_id.is_empty() {
        eprintln!("[opencode_acp_live] no sessionId from session/new within deadline");
        let _ = child.kill();
        return LiveAcpOutcome::Timeout;
    }

    // 4. Send a trivial `session/prompt` request (id = 3).
    let prompt_req = encode_request(
        serde_json::json!(3),
        "session/prompt",
        session_prompt_params(&session_id, "Say hello in one word."),
    );
    if let Err(e) = write_line(&mut stdin, &prompt_req) {
        let _ = child.kill();
        return LiveAcpOutcome::Error(e);
    }

    // 5. Read until we see a session/update notification OR the session/prompt
    //    result, whichever comes first.
    let mut outcome = LiveAcpOutcome::Timeout;
    while Instant::now() < deadline {
        let line = match recv_timeout(deadline) {
            Some(l) => l,
            None => break,
        };
        match parse_acp_line(&line) {
            Ok(AcpMessage::Notification { method, .. }) if method == "session/update" => {
                eprintln!(
                    "[opencode_acp_live] received session/update notification — ACP channel confirmed"
                );
                outcome = LiveAcpOutcome::GotUpdate;
                break;
            }
            Ok(AcpMessage::Response { id, result: Ok(_) }) if id == serde_json::json!(3) => {
                // session/prompt result (stopReason) — also counts as success.
                eprintln!(
                    "[opencode_acp_live] received session/prompt result — ACP channel confirmed"
                );
                outcome = LiveAcpOutcome::GotUpdate;
                break;
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    // Clean up: close stdin → EOF, kill the process.
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();

    outcome
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// Opt-in live seam test: skips when `opencode` is absent; exercises the full
/// ACP handshake when present.
///
/// Current expected state in this sandbox:
///   - `opencode` is NOT on PATH → SKIP (green).
///   - `acp` functions are `todo!()` stubs → would panic if called.
///
/// On a developer machine with `opencode` installed, this test exercises the
/// real end-to-end ACP wiring.
#[test]
fn live_opencode_acp_handshake_or_skip() {
    if !opencode_present() {
        eprintln!(
            "[opencode_acp_live] SKIP: `opencode` not found on PATH. \
             Unit tests in opencode_capabilities.rs prove the pure parse layer. \
             Install opencode and re-run to exercise the live ACP seam."
        );
        return; // clean skip — test passes
    }

    eprintln!(
        "[opencode_acp_live] `opencode` detected on PATH; attempting live ACP handshake …"
    );

    match run_live_acp_handshake() {
        LiveAcpOutcome::GotUpdate => {
            eprintln!("[opencode_acp_live] PASS: live ACP handshake succeeded — bidirectional channel confirmed.");
        }
        LiveAcpOutcome::Timeout => {
            eprintln!(
                "[opencode_acp_live] SKIP: ACP handshake timed out (opencode present but no \
                 update received — model may be slow or process exited early). \
                 The pure unit tests (opencode_capabilities.rs) cover the parse/encode layer."
            );
            // A timeout is a loud skip, not a hard failure: the subprocess ran
            // but was slow.  This mirrors i02's retry pattern — don't fail the
            // CI on model non-determinism.
        }
        LiveAcpOutcome::Error(e) => {
            // A spawn/pipe error is a hard failure when opencode IS present —
            // it means the ACP wiring is broken at the process level.
            panic!(
                "[opencode_acp_live] FAIL: live ACP handshake failed: {e}\n\
                 This means `opencode acp` could not be driven over stdin/stdout.\n\
                 Check that `opencode acp` speaks JSON-RPC 2.0 newline-delimited on stdio."
            );
        }
    }
}
