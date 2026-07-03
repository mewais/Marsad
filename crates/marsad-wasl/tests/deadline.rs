//! deadline.rs — proves `post_decision` (and, transitively, the read helpers
//! it shares with the SSE connect/resnapshot paths) is bounded by an OVERALL
//! read deadline, not just `connect_timeout`.
//!
//! Regression coverage for the review finding: a Wasl operator that accepts
//! a TCP connection but never writes a byte back used to hang
//! `post_decision` forever (`wire::read_batch`'s WouldBlock retry loop had no
//! cap), which — since `marsad-agent::handle_answer_wasl_gate` calls
//! `decide()` synchronously on the daemon's single command-processing thread
//! — froze the entire agent for every session. `read_batch` now caps its
//! retry count, so a wedged peer returns `WaslError::Io` within a bounded
//! wall-clock window instead of blocking the caller forever.
mod support;

use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use marsad_wasl::{WaslClientConfig, WaslError, WaslOperatorClient};

use support::{BufferedResponse, MockServer};

/// Generous upper bound for the test itself — well above the production
/// read-deadline budget, but far below "hangs forever". If `post_decision`
/// doesn't return within this window, something is still unbounded.
const TEST_BOUND: Duration = Duration::from_secs(15);

#[test]
fn post_decision_against_a_hung_peer_returns_promptly_instead_of_hanging_forever() {
    let server = MockServer::start();
    server.push_decision_response(BufferedResponse::hang());

    let client = WaslOperatorClient::new(WaslClientConfig {
        base_url: server.base_url(),
        initial_backoff_ms: 1,
    });

    // Run the call on its own thread so THIS test fails with a clear
    // "did not return within TEST_BOUND" message (via recv_timeout) rather
    // than hanging the whole `cargo test` process if the deadline is ever
    // regressed away — the failure mode we're specifically guarding against.
    let (tx, rx) = channel();
    thread::spawn(move || {
        let result = client.post_decision("gate-1", "approved", None);
        let _ = tx.send(result);
    });

    let result = rx.recv_timeout(TEST_BOUND).unwrap_or_else(|_| {
        panic!(
            "post_decision did not return within {TEST_BOUND:?} against a peer that accepts \
             a connection and never responds — the read loop has no overall deadline"
        )
    });

    let err = result
        .expect_err("a peer that accepts but never responds must surface as an error, not an ack");
    assert!(
        matches!(err, WaslError::Io(_)),
        "expected WaslError::Io for a read-deadline timeout, got {err:?}"
    );
}
