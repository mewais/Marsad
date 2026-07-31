//! real_timeout.rs — connection / read-deadline discipline for
//! `RealWaslSessionClient` (design §3; mirrors `marsad-wasl`'s
//! CONNECT_TIMEOUT / MAX_READ_RETRIES read-deadline pattern —
//! `crates/marsad-wasl/src/wire.rs`). A dead/wedged/absent Wasl node must
//! surface as a BOUNDED, non-hanging error on every blocking call — a hang here
//! would freeze the agent's single command-processing thread (the X4.3 lesson
//! that motivated marsad-wasl's read cap).
//!
//! These are wall-clock-bounded tests: each blocking call is spawned on a thread
//! and must complete within a generous ceiling far below "forever". If the real
//! client lacks the deadline discipline, the join times out and the test fails
//! (rather than hanging the suite).
//!
//! RED until `RealWaslSessionClient` exists.

mod real_support;

use std::sync::mpsc::{channel, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use marsad_wasl_session::{
    RealWaslSessionProvider, WaslCancelReason, WaslGateDecision, WaslSessionClient,
    WaslSessionClientConfig, WaslSessionError, WaslSessionProvider, WaslShutdownMode,
    WaslTargetKey, WaslTurnId,
};

use real_support::{compact_fixture, unused_base_url, val, BufferedResponse, MockRuntimeServer};

/// Generous wall-clock ceiling: the client's own read deadline (a few seconds,
/// per the marsad-wasl MAX_READ_RETRIES * READ_TIMEOUT budget) must fire well
/// inside this. A truly-unbounded client would blow past it → test fails.
const CEILING: Duration = Duration::from_secs(20);

/// Run `f` on a thread and return its value; fail (not hang) if it doesn't
/// finish within `CEILING`.
fn within_ceiling_value<T: Send + 'static>(
    what: &str,
    f: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(CEILING) {
        Ok(v) => v,
        Err(RecvTimeoutError::Timeout) => panic!(
            "{what} did not complete within {CEILING:?} — the client is not bounding its \
             connect/read deadline (mirror marsad-wasl CONNECT_TIMEOUT/MAX_READ_RETRIES)"
        ),
        Err(RecvTimeoutError::Disconnected) => panic!("{what} thread panicked"),
    }
}

fn config_for(base_url: String) -> WaslSessionClientConfig {
    WaslSessionClientConfig { base_url }
}

/// Bind a client against a live server (so we have a `tgt-1` handle), returning
/// both so the caller can then wedge/drop the server for the operation itself.
fn bound_client(server: &MockRuntimeServer) -> Box<dyn WaslSessionClient> {
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));
    let provider = RealWaslSessionProvider::new(config_for(server.base_url()));
    let (client, _snapshot) = provider
        .get_or_start(&WaslTargetKey(val(r#""ctx-42""#)))
        .expect("create should succeed");
    client
}

// ---------------------------------------------------------------------------
// Connection refused — a provider pointed at a dead port.
// ---------------------------------------------------------------------------

#[test]
fn provider_get_or_start_connection_refused_is_bounded_io_error() {
    let base = unused_base_url();
    let err = within_ceiling_value("get_or_start (refused)", move || {
        let provider = RealWaslSessionProvider::new(config_for(base));
        provider.get_or_start(&WaslTargetKey(val(r#""ctx-42""#)))
    })
    .expect_err("a refused connection must be an error, not a hang");
    assert!(
        matches!(err, WaslSessionError::Io(_)),
        "connection refused → Io, got {err:?}"
    );
}

#[test]
fn provider_list_connection_refused_is_bounded_io_error() {
    let base = unused_base_url();
    let err = within_ceiling_value("list (refused)", move || {
        RealWaslSessionProvider::new(config_for(base)).list()
    })
    .expect_err("refused → error");
    assert!(matches!(err, WaslSessionError::Io(_)), "got {err:?}");
}

#[test]
fn provider_unregister_peer_connection_refused_is_bounded_io_error() {
    let base = unused_base_url();
    let err = within_ceiling_value("unregister_peer (refused)", move || {
        RealWaslSessionProvider::new(config_for(base)).unregister_peer("node-a")
    })
    .expect_err("refused → error");
    assert!(matches!(err, WaslSessionError::Io(_)), "got {err:?}");
}

#[test]
fn provider_register_peer_connection_refused_is_bounded_io_error() {
    let base = unused_base_url();
    let err = within_ceiling_value("register_peer (refused)", move || {
        RealWaslSessionProvider::new(config_for(base)).register_peer(
            marsad_wasl_session::WaslAgentCard {
                id: "node-a".to_string(),
                host: "host-a".to_string(),
                endpoint: None,
            },
        )
    })
    .expect_err("refused → error");
    assert!(matches!(err, WaslSessionError::Io(_)), "got {err:?}");
}

#[test]
fn provider_list_peers_connection_refused_is_bounded_io_error() {
    let base = unused_base_url();
    let err = within_ceiling_value("list_peers (refused)", move || {
        RealWaslSessionProvider::new(config_for(base)).list_peers()
    })
    .expect_err("refused → error");
    assert!(matches!(err, WaslSessionError::Io(_)), "got {err:?}");
}

// ---------------------------------------------------------------------------
// Wedged peer — accepts the connection, reads the request, never responds.
// The CLIENT-side read deadline (not a server close) must bound the call.
// ---------------------------------------------------------------------------

#[test]
fn get_or_start_wedged_peer_hits_read_deadline() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::hang());
    let base = server.base_url();

    let err = within_ceiling_value("get_or_start (wedged)", move || {
        // keep `server` alive for the duration by moving it into the closure.
        let _keep = server;
        RealWaslSessionProvider::new(config_for(base))
            .get_or_start(&WaslTargetKey(val(r#""ctx-42""#)))
    })
    .expect_err("a wedged peer must surface as a bounded Io error, not a hang");
    assert!(
        matches!(err, WaslSessionError::Io(_)),
        "wedged peer → Io within the read deadline, got {err:?}"
    );
}

#[test]
fn unregister_peer_wedged_peer_hits_read_deadline() {
    let server = MockRuntimeServer::start();
    server.push_peers_delete(BufferedResponse::hang());
    let base = server.base_url();

    let err = within_ceiling_value("unregister_peer (wedged)", move || {
        let _keep = server;
        RealWaslSessionProvider::new(config_for(base)).unregister_peer("node-a")
    })
    .expect_err("a wedged peer must surface as a bounded Io error, not a hang");
    assert!(
        matches!(err, WaslSessionError::Io(_)),
        "wedged peer → Io within the read deadline, got {err:?}"
    );
}

#[test]
fn resolve_native_gate_wedged_peer_hits_read_deadline() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_gate(BufferedResponse::hang());

    let err = within_ceiling_value("resolve_native_gate (wedged)", move || {
        let _keep = server;
        client.resolve_native_gate(&WaslTurnId("turn-7".to_string()), WaslGateDecision::Approve)
    })
    .expect_err("wedged gate POST → bounded Io, not a hang");
    assert!(matches!(err, WaslSessionError::Io(_)), "got {err:?}");
}

#[test]
fn interrupt_wedged_peer_hits_read_deadline() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_cancel(BufferedResponse::hang());

    let err = within_ceiling_value("interrupt (wedged)", move || {
        let _keep = server;
        client.interrupt(WaslCancelReason::Human)
    })
    .expect_err("wedged cancel POST → bounded Io, not a hang");
    assert!(matches!(err, WaslSessionError::Io(_)), "got {err:?}");
}

#[test]
fn shutdown_wedged_peer_hits_read_deadline() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    server.push_shutdown(BufferedResponse::hang());

    let err = within_ceiling_value("shutdown (wedged)", move || {
        let _keep = server;
        client.shutdown(WaslShutdownMode::Drain)
    })
    .expect_err("wedged shutdown POST → bounded Io, not a hang");
    assert!(matches!(err, WaslSessionError::Io(_)), "got {err:?}");
}

// ---------------------------------------------------------------------------
// attach against a dropped/absent stream ends bounded (no infinite pump).
// ---------------------------------------------------------------------------

#[test]
fn attach_to_dead_stream_ends_bounded() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    drop(server); // the events subscription connect is refused.

    // attach() yields a stream; pumping it to end must terminate (the connect
    // failed → the stream is immediately exhausted / yields None) within the
    // ceiling, never an infinite retry loop inside a single next().
    let items = within_ceiling_value("attach (dead) drain", move || {
        let mut stream = client.attach(0);
        let mut out = Vec::new();
        while let Some(item) = stream.next() {
            out.push(item);
            if out.len() > 1000 {
                break; // safety: a runaway stream must not accumulate forever.
            }
        }
        out.len()
    });
    assert!(
        items <= 1000,
        "a dead attach must not produce an unbounded stream"
    );
}
