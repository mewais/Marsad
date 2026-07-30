//! real_provider.rs — black-box tests for `RealWaslSessionProvider` (the
//! hand-rolled HTTP client factory seam; design `docs/design/wasl-backend-plan.md`
//! §3, §4.3, §11: the real client is the "separate, later phase" these tests
//! specify). Loopback-real tier: the real HTTP code path runs end to end against
//! `real_support::MockRuntimeServer`, whose canned responses are built from the
//! VENDORED `fixtures/runtime-control-v1/` files.
//!
//! Covers the provider's two verbs:
//!   * `get_or_start(key)` → `POST /v1/targets` (create-or-reuse) → parses a
//!     `target.json`-shaped `TargetSnapshot` into a bound `RealWaslSessionClient`
//!     PLUS a caller-side `WaslTargetSnapshot` (carrying `last_seq`, …).
//!   * `list()` → `GET /v1/targets` → parses `targets_snapshot.json`.
//!
//! ── SPEC-CORRECTION (v3, revision-log item 2 + §4.3 + §6.1) ──────────────────
//! The real Wasl wire `POST /v1/targets` is **create-or-reuse and ALWAYS returns
//! a `TargetSnapshot`** — the response shape is IDENTICAL whether the target was
//! freshly spawned or an already-live one was reused (confirmed against
//! `Wasl/crates/wasl-runtime-control/` and the vendored `target.json` fixture,
//! which carries a `last_seq` field). There is therefore **NO wire signal that
//! says "created" vs "reused"** — exactly what v3 states: *"there is NO `created`
//! flag anywhere in real Wasl … Fresh-vs-resumed is decided by comparing the
//! returned snapshot's `last_seq`."*
//!
//! `get_or_start` therefore returns `(Box<dyn WaslSessionClient>,
//! WaslTargetSnapshot)` — NOT a `created: bool`. The caller (marsad-core, §6.1)
//! runs the `last_seq` comparison against its persisted `WaslSessionBinding` to
//! decide fresh-vs-resume. These tests pin that the provider surfaces the
//! snapshot verbatim (target_id, capabilities, status, `last_seq`) from the wire
//! `target.json`, so the caller has what §6.1 needs.
//!
//! This file WILL fail to compile until a later worker implements
//! `RealWaslSessionProvider`/`RealWaslSessionClient`, `WaslSessionClientConfig`,
//! and the `WaslTargetSnapshot`-returning `get_or_start` — expected tests-first
//! RED signal (§10 / house test-first rule).

mod real_support;

use marsad_wasl_session::{
    RealWaslSessionProvider, WaslSessionClientConfig, WaslSessionProvider, WaslTargetKey,
    WaslTargetStatus,
};

use real_support::{compact_fixture, fixture_value, val, BufferedResponse, MockRuntimeServer};

/// Build a provider pointed at the mock server.
fn provider(server: &MockRuntimeServer) -> RealWaslSessionProvider {
    RealWaslSessionProvider::new(WaslSessionClientConfig {
        base_url: server.base_url(),
    })
}

/// The target key the create_request fixture is keyed by (`"ctx-42"`).
fn ctx_key() -> WaslTargetKey {
    WaslTargetKey(val(r#""ctx-42""#))
}

/// The golden `target.json` snapshot with its `last_seq` overridden to `n`, so a
/// test can drive both the fresh (`n == 0`) and reused (`n > 0`) create cases off
/// the same real wire shape.
fn target_snapshot_with_last_seq(n: u64) -> String {
    let mut v = fixture_value("target.json");
    v["last_seq"] = serde_json::json!(n);
    serde_json::to_string(&v).expect("snapshot must serialize")
}

// ---------------------------------------------------------------------------
// get_or_start — POST /v1/targets, parse target.json into a bound client + the
// caller-side WaslTargetSnapshot.
// ---------------------------------------------------------------------------

#[test]
fn get_or_start_posts_create_request_and_binds_target_from_target_json() {
    let server = MockRuntimeServer::start();
    // 200 OK carrying the golden `target.json` snapshot.
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    let provider = provider(&server);
    let (client, snapshot) = provider
        .get_or_start(&ctx_key())
        .expect("get_or_start should succeed on a 200 target snapshot");

    // The client is bound to the target_id the snapshot carried.
    assert_eq!(client.target_id(), "tgt-1");
    // The returned snapshot mirrors the wire `target.json` verbatim: same
    // target_id, and the `last_seq` (12) the golden fixture carries — this is
    // what the caller (§6.1) compares against its persisted binding.last_seq to
    // decide fresh-vs-resume. There is NO `created` bool: the wire never signals
    // create-vs-reuse (create-or-reuse always returns this same shape).
    assert_eq!(snapshot.target_id, "tgt-1");
    assert_eq!(
        snapshot.last_seq, 12,
        "the snapshot must surface the wire `last_seq` verbatim (target.json = 12) \
         so the caller can run the §6.1 last_seq comparison"
    );
    assert_eq!(
        snapshot.key,
        ctx_key(),
        "the snapshot carries the target key it was created/reused for"
    );

    // Exactly one POST /v1/targets, carrying a create_request.json-shaped body.
    let creates = server.requests_to("POST", "/v1/targets");
    assert_eq!(creates.len(), 1, "exactly one create call");
    let sent = &creates[0];
    assert_eq!(
        sent.header("content-type"),
        Some("application/json"),
        "create body must be sent as application/json"
    );
    // The body carries the requested key (value-equality, key order irrelevant).
    assert_eq!(
        sent.body_json().get("key"),
        Some(&val(r#""ctx-42""#)),
        "create request carries the requested target key"
    );
}

#[test]
fn get_or_start_snapshot_carries_status_from_target_json() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(200, compact_fixture("target.json")));

    let provider = provider(&server);
    let (_client, snapshot) = provider.get_or_start(&ctx_key()).expect("create ok");

    // target.json's status is `{ "type": "idle", "since": … }` → Idle.
    assert!(
        matches!(snapshot.status, WaslTargetStatus::Idle { .. }),
        "the snapshot must carry the wire status (target.json = idle), got {:?}",
        snapshot.status
    );
}

/// The returned `last_seq` is read PURELY from the wire snapshot, not from call
/// ordering or a "first call is fresh" shortcut: two independent `get_or_start`
/// calls, each served a `last_seq > 0` snapshot, both surface that non-zero
/// `last_seq` verbatim. This rules out the old dishonest hardcode
/// (`RealWaslSessionProvider::get_or_start` returning a fixed value regardless of
/// the snapshot) that discarded the real `last_seq` entirely.
#[test]
fn get_or_start_last_seq_is_read_from_the_snapshot_not_synthesized() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(
        200,
        target_snapshot_with_last_seq(5),
    ));
    server.push_create(BufferedResponse::json(
        200,
        target_snapshot_with_last_seq(9),
    ));

    let p = provider(&server);
    let (_c1, s1) = p.get_or_start(&ctx_key()).expect("first create");
    let (_c2, s2) = p.get_or_start(&ctx_key()).expect("second create");

    assert_eq!(
        s1.last_seq, 5,
        "the first snapshot's last_seq must be read verbatim from the wire (5)"
    );
    assert_eq!(
        s2.last_seq, 9,
        "the second snapshot's last_seq must be read verbatim from the wire (9), \
         proving it is derived from the snapshot, not synthesized/hardcoded"
    );
}

/// A genuinely fresh target's journal starts empty, so the wire reports
/// `last_seq == 0`; the provider surfaces that verbatim. (The caller then
/// recognizes `last_seq == 0` as a fresh/reset target per §6.1 — the provider
/// itself makes no fresh-vs-reused claim.)
#[test]
fn get_or_start_surfaces_zero_last_seq_for_a_fresh_target() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(
        200,
        target_snapshot_with_last_seq(0),
    ));

    let (_client, snapshot) = provider(&server)
        .get_or_start(&ctx_key())
        .expect("get_or_start on a 200 snapshot must succeed");

    assert_eq!(
        snapshot.last_seq, 0,
        "a fresh target's journal is empty → wire last_seq == 0, surfaced verbatim"
    );
}

#[test]
fn get_or_start_maps_admission_refused_429_to_unavailable() {
    let server = MockRuntimeServer::start();
    // 429 + error_admission_refused.json body (the wire's target-explosion refusal).
    server.push_create(BufferedResponse::json(
        429,
        compact_fixture("error_admission_refused.json"),
    ));

    let provider = provider(&server);
    let err = provider
        .get_or_start(&ctx_key())
        .expect_err("a 429 admission refusal must surface as an error");

    // admission_refused is a capacity refusal, not a malformed/protocol fault →
    // Unavailable (the target cannot be serviced right now), carrying the detail.
    match err {
        marsad_wasl_session::WaslSessionError::Unavailable(detail) => {
            assert!(
                detail.contains("per-sender cap reached"),
                "detail should relay the wire error detail, got {detail:?}"
            );
        }
        other => panic!("expected Unavailable for admission_refused, got {other:?}"),
    }
}

#[test]
fn get_or_start_maps_malformed_400_to_protocol() {
    let server = MockRuntimeServer::start();
    server.push_create(BufferedResponse::json(
        400,
        compact_fixture("error_malformed.json"),
    ));

    let provider = provider(&server);
    let err = provider
        .get_or_start(&ctx_key())
        .expect_err("a 400 malformed must surface as an error");

    assert!(
        matches!(err, marsad_wasl_session::WaslSessionError::Protocol(_)),
        "a 400 malformed request maps to Protocol, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// list — GET /v1/targets, parse targets_snapshot.json.
// ---------------------------------------------------------------------------

#[test]
fn list_parses_targets_snapshot_json_into_target_ids() {
    let server = MockRuntimeServer::start();
    server.push_list(BufferedResponse::json(
        200,
        compact_fixture("targets_snapshot.json"),
    ));

    let provider = provider(&server);
    let ids = provider.list().expect("list should parse the snapshot");

    // targets_snapshot.json carries exactly one target, id "tgt-1".
    assert_eq!(ids, vec!["tgt-1".to_string()]);

    let gets = server.requests_to("GET", "/v1/targets");
    assert_eq!(gets.len(), 1, "exactly one GET /v1/targets");
}

#[test]
fn list_empty_registry_is_empty_vec_not_error() {
    let server = MockRuntimeServer::start();
    server.push_list(BufferedResponse::json(200, r#"{"targets":[]}"#));

    let provider = provider(&server);
    let ids = provider
        .list()
        .expect("an empty registry is a valid snapshot");
    assert!(ids.is_empty(), "no targets → empty list, not an error");
}

#[test]
fn list_malformed_body_is_protocol_error() {
    let server = MockRuntimeServer::start();
    // 200 but a body that is NOT a valid TargetsSnapshotResponse.
    server.push_list(BufferedResponse::json(200, r#"{"not":"a snapshot"}"#));

    let provider = provider(&server);
    let err = provider
        .list()
        .expect_err("a 200 with an unparseable body must be a Protocol error");
    assert!(
        matches!(err, marsad_wasl_session::WaslSessionError::Protocol(_)),
        "malformed 200 snapshot → Protocol, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Fixture drift guard: the vendored targets_snapshot.json really does contain
// exactly the one tgt-1 entry the parsing assertions above rely on. Keeps the
// wire-contract copy honest independent of the (not-yet-existing) client.
// ---------------------------------------------------------------------------

#[test]
fn vendored_targets_snapshot_fixture_shape_is_pinned() {
    let snap = fixture_value("targets_snapshot.json");
    let targets = snap
        .get("targets")
        .and_then(|t| t.as_array())
        .expect("targets_snapshot.json has a targets array");
    assert_eq!(targets.len(), 1, "the golden snapshot has one target");
    assert_eq!(
        targets[0].get("target_id"),
        Some(&val(r#""tgt-1""#)),
        "the golden snapshot's single target is tgt-1"
    );
}

// ---------------------------------------------------------------------------
// Fixture drift guard: target.json's last_seq is the value the reconnect
// comparison in real_provider tests + the §6.1 caller logic rely on (12).
// ---------------------------------------------------------------------------

#[test]
fn vendored_target_fixture_last_seq_is_pinned() {
    let target = fixture_value("target.json");
    assert_eq!(
        target.get("last_seq"),
        Some(&serde_json::json!(12)),
        "the golden target.json carries last_seq = 12 (a reused/live target); \
         the caller's §6.1 comparison uses exactly this against its persisted seq"
    );
}
