//! real_status.rs — black-box tests for `RealWaslSessionClient::status()`
//! (`GET /v1/targets/{id}`; design §3, §4.2). status() is ADVISORY and its
//! signature is infallible (`-> WaslTargetStatus`), so a transport failure must
//! degrade to a conservative not-live status rather than panic — pinned here.
//!
//! Loopback-real tier against `real_support::MockRuntimeServer`, fixture-derived
//! responses. RED until `RealWaslSessionClient` exists.

mod real_support;

use marsad_wasl_session::{
    RealWaslSessionProvider, WaslSessionClient, WaslSessionClientConfig, WaslSessionProvider,
    WaslTargetKey, WaslTargetStatus,
};

use real_support::{compact_fixture, val, BufferedResponse, MockRuntimeServer};

/// Provision a client bound to `tgt-1` via a create (`POST /v1/targets` →
/// target.json), the spec-correct way to obtain a bound `RealWaslSessionClient`.
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

// ---------------------------------------------------------------------------
// status — GET /v1/targets/{id}, parse the snapshot's status field.
// ---------------------------------------------------------------------------

#[test]
fn status_parses_idle_from_target_json() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);

    // The status() snapshot GET returns the golden target.json (status: idle).
    server.push_target(BufferedResponse::json(200, compact_fixture("target.json")));

    let status = client.status();
    match status {
        WaslTargetStatus::Idle { since } => {
            assert_eq!(since, "2026-07-02T12:00:00.000Z");
        }
        other => panic!("expected Idle from target.json, got {other:?}"),
    }

    // The status snapshot hit GET /v1/targets/tgt-1 exactly once.
    let gets = server.requests_to("GET", "/v1/targets/tgt-1");
    assert_eq!(gets.len(), 1, "status() does one GET /v1/targets/{{id}}");
}

#[test]
fn status_parses_busy_status_shape() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);

    // A busy snapshot: same target shape with a Busy status (internally-tagged
    // snake_case, exactly as real Wasl serializes TargetStatus).
    let busy_body = r#"{
        "target_id": "tgt-1", "key": "ctx-42",
        "capabilities": {
            "persistent_process": true, "persistent_conversation": true, "structured_turns": true,
            "streaming_output": false, "attachable": true, "interruptible": false,
            "structured_provenance": true, "resumable": true
        },
        "status": { "type": "busy", "active_turn": "turn-7", "source": "local_human",
                    "phase": "generating", "since": "2026-07-02T12:00:01.000Z", "queued": 0 },
        "last_seq": 13, "queued": 0, "native_session_id": "ses_1"
    }"#;
    server.push_target(BufferedResponse::json(200, busy_body));

    match client.status() {
        WaslTargetStatus::Busy {
            active_turn,
            source,
            phase,
            queued,
            ..
        } => {
            assert_eq!(active_turn, "turn-7");
            assert_eq!(source, "local_human");
            assert_eq!(phase, "generating");
            assert_eq!(queued, 0);
        }
        other => panic!("expected Busy, got {other:?}"),
    }
}

#[test]
fn status_parses_awaiting_gate_with_native_detail() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);

    // AwaitingGate carries a {summary, preview} native detail (no gate_id/tool).
    let gate_body = r#"{
        "target_id": "tgt-1", "key": "ctx-42",
        "capabilities": {
            "persistent_process": true, "persistent_conversation": true, "structured_turns": true,
            "streaming_output": false, "attachable": true, "interruptible": false,
            "structured_provenance": true, "resumable": true
        },
        "status": { "type": "awaiting_gate", "active_turn": "turn-7",
                    "detail": { "summary": "write access", "preview": "rm -rf /tmp/x" },
                    "since": "2026-07-02T12:00:02.000Z", "queued": 0 },
        "last_seq": 14, "queued": 0, "native_session_id": "ses_1"
    }"#;
    server.push_target(BufferedResponse::json(200, gate_body));

    match client.status() {
        WaslTargetStatus::AwaitingGate {
            active_turn,
            detail,
            ..
        } => {
            assert_eq!(active_turn, "turn-7");
            assert_eq!(detail.summary, "write access");
            assert_eq!(detail.preview, "rm -rf /tmp/x");
        }
        other => panic!("expected AwaitingGate, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// status is advisory + infallible — a transport failure degrades, never panics.
// ---------------------------------------------------------------------------

#[test]
fn status_on_unknown_target_404_degrades_to_offline() {
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);

    // The node no longer knows this target (it went away) → 404.
    server.push_target(BufferedResponse::json(
        404,
        compact_fixture("error_unknown_target.json"),
    ));

    // status() cannot return a Result (advisory signature); a target that the
    // node no longer knows is not live → Offline (the conservative not-live
    // status, which projects to Phase::Detached — the "reconnect?" bucket).
    assert_eq!(client.status(), WaslTargetStatus::Offline);
}

#[test]
fn status_on_connection_refused_degrades_to_offline_not_panic() {
    // Bind a client then point status() at a dead port by giving the server a
    // create response, obtaining the client, THEN dropping the server so the
    // status GET hits a refused connection.
    let server = MockRuntimeServer::start();
    let client = bound_client(&server);
    drop(server); // nothing listening now → GET /v1/targets/tgt-1 is refused.

    // Advisory + infallible: a refused connection degrades to Offline, never
    // panics and never hangs (bounded connect timeout, mirroring marsad-wasl's
    // CONNECT_TIMEOUT discipline).
    assert_eq!(client.status(), WaslTargetStatus::Offline);
}
