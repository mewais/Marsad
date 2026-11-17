//! capabilities.rs — U1 dynamic capability discovery, `marsad-core` half.
//!
//! Authoritative spec: `docs/specs/dynamic-capabilities.md` §3.5 (core state +
//! `on_agent_line` arms + `refresh_capabilities`/`capabilities()`), §4 item 2
//! (the core-level non-blocking rule), and §6 ("marsad-core" test plan).
//!
//! Mirrors the house pattern already established by `tests/fakes.rs` /
//! `tests/happy.rs`: drive `Core` over a `LoopbackFake` transport, push raw
//! IPC lines into the peer end to simulate the agent, and assert both the
//! `VmUpdate`s `on_agent_line` returns and the outbound lines `Core` sends
//! (via `drain_outbound`, `parse_ipc`-checked per M2 convention).
//!
//! This file WILL fail to compile until the code-agent lands `Core::capabilities()`,
//! `Core::refresh_capabilities()`, the `on_agent_line` `CapabilitiesUpdate` /
//! `CapabilitiesRefreshQueued` arms, and `VmUpdate::Capabilities` (§3.5) — plus,
//! transitively, `marsad_viewmodel::CapabilitiesView` (a sibling block's
//! responsibility per the spec's block list, §0: "B02-adjacent marsad-viewmodel").
//! That is the expected tests-first compile-failure signal.

use marsad_core::{Core, CoreConfig, CoreError, VmUpdate};
use marsad_proto::{parse_ipc, IpcMsg};
use marsad_transport::LoopbackFake;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn cfg() -> CoreConfig {
    CoreConfig {
        agent_endpoint: "loopback://fake".to_string(),
        data_root: "/tmp/marsad-capabilities-core-test".to_string(),
    }
}

fn make_core() -> (Core, LoopbackFake) {
    let (a, b) = LoopbackFake::pair();
    let core = Core::new(cfg(), Box::new(a)).expect("Core::new");
    (core, b)
}

/// The §2 golden `CapabilitiesUpdate` line for the codex/AllKnown case, quoted
/// verbatim from `dynamic-capabilities.md` §2 (same golden the proto suite
/// pins byte-for-byte).
fn capabilities_update_line_codex() -> String {
    r#"{"type":"CapabilitiesUpdate","host_id":"local","driver":"codex","capabilities":{"driver":"codex","host_id":"local","cli_available":true,"models":{"models":[{"id":"gpt-5.5"}],"source":"Listed","free_text":false},"session_scope":"AllKnown","sessions":[{"id":"97197d46-2091-40e2-b36f-0dcedd34bcc4","project":"/work/mewais/OpenSource/Marsad","snippet":"x","modified_ms":1783100616000}],"fetched_at_ms":1783100620000,"errors":[]}}"#.to_string()
}

/// A second snapshot for the SAME (driver, host) key as
/// `capabilities_update_line_codex`, with visibly different content (fewer
/// models, a later `fetched_at_ms`) — used to prove replace-not-merge
/// semantics (§2: "Always a COMPLETE snapshot (replace, not merge, on the
/// receiving side)").
fn capabilities_update_line_codex_v2() -> String {
    r#"{"type":"CapabilitiesUpdate","host_id":"local","driver":"codex","capabilities":{"driver":"codex","host_id":"local","cli_available":true,"models":{"models":[],"source":"Listed","free_text":false},"session_scope":"AllKnown","sessions":[],"fetched_at_ms":1783100999000,"errors":["codex models: exit 1: boom"]}}"#.to_string()
}

/// The §2 golden line for a DIFFERENT (driver, host) key — used for
/// snapshot-accumulation tests (proves the second key doesn't clobber the
/// first).
fn capabilities_update_line_claude_hpc() -> String {
    r#"{"type":"CapabilitiesUpdate","host_id":"hpc","driver":"claude","capabilities":{"driver":"claude","host_id":"hpc","cli_available":true,"models":{"models":[{"id":"claude-opus-4-8"}],"source":"Observed","free_text":true},"session_scope":{"Projects":["/home/u/proj"]},"sessions":[],"fetched_at_ms":0,"errors":[]}}"#.to_string()
}

// ---------------------------------------------------------------------------
// on_agent_line: CapabilitiesUpdate → VmUpdate::Capabilities (§3.5)
// ---------------------------------------------------------------------------

/// §6: "`on_agent_line` + `CapabilitiesUpdate` golden → one
/// `VmUpdate::Capabilities` with the mapped view."
#[test]
fn on_agent_line_capabilities_update_golden_yields_one_capabilities_vm_update() {
    let (mut core, _peer) = make_core();
    let updates = core
        .on_agent_line(&capabilities_update_line_codex())
        .expect("well-formed CapabilitiesUpdate must not error");

    assert_eq!(
        updates.len(),
        1,
        "exactly one VmUpdate for one CapabilitiesUpdate line"
    );
    match &updates[0] {
        VmUpdate::Capabilities(view) => {
            // §3.6: "CapabilitiesView — display-ready mirror of Capabilities
            // (ids verbatim; no label synthesis) — pure From<Capabilities>
            // mapping." The proto-level field values must survive unmangled.
            assert_eq!(view.driver, "codex");
            assert_eq!(view.host_id, "local");
        }
        other => panic!("expected VmUpdate::Capabilities, got {other:?}"),
    }
}

/// §6: "second update for same key replaces (not merges)."
#[test]
fn on_agent_line_second_update_for_same_key_replaces_not_merges() {
    let (mut core, _peer) = make_core();
    core.on_agent_line(&capabilities_update_line_codex())
        .expect("first update must apply");
    core.on_agent_line(&capabilities_update_line_codex_v2())
        .expect("second update for the same (driver, host) must apply");

    let snapshot = core.capabilities();
    let matching: Vec<_> = snapshot
        .iter()
        .filter(|v| v.driver == "codex" && v.host_id == "local")
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "the (driver, host) key must have exactly one entry after two updates — replace, not accumulate"
    );
}

/// A FAILED probe (errors non-empty) must NOT wipe previously-discovered
/// models/sessions. On a slow remote host the bounded probe can time out and
/// come back empty + `cli_available:false`; a plain replace would erase the good
/// model list + session list that a prior successful probe found (the "models
/// vanished after a timeout on agent33" bug). The errored update advances only
/// the error log — the discovered data is preserved.
#[test]
fn on_agent_line_errored_update_preserves_prior_models_and_sessions() {
    let (mut core, _peer) = make_core();
    // A good claude probe for a project on "agent33": one model + one session.
    let good = r#"{"type":"CapabilitiesUpdate","host_id":"agent33","driver":"claude","capabilities":{"driver":"claude","host_id":"agent33","cli_available":true,"models":{"models":[{"id":"claude-opus-4-8"}],"source":"Observed","free_text":true},"session_scope":{"Projects":["/work/mewais/Tajir"]},"sessions":[{"id":"sess-1","project":"/work/mewais/Tajir","snippet":"prev","modified_ms":1}],"fetched_at_ms":1,"errors":[]}}"#;
    core.on_agent_line(good).expect("good update applies");

    // A later scoped probe TIMED OUT: empty models/sessions, cli_available:false,
    // errors present. It must preserve the model + session from the good probe.
    let timed_out = r#"{"type":"CapabilitiesUpdate","host_id":"agent33","driver":"claude","capabilities":{"driver":"claude","host_id":"agent33","cli_available":false,"models":{"models":[],"source":"Observed","free_text":true},"session_scope":{"Projects":["/work/mewais/Tajir"]},"sessions":[],"fetched_at_ms":2,"errors":["claude discovery: timed out"]}}"#;
    core.on_agent_line(timed_out).expect("errored update applies");

    let snapshot = core.capabilities();
    let caps = snapshot
        .iter()
        .find(|v| v.driver == "claude" && v.host_id == "agent33")
        .expect("claude/agent33 caps present");
    assert!(
        caps.models.models.iter().any(|m| m.id == "claude-opus-4-8"),
        "a timed-out probe must NOT wipe the model list; got {:?}",
        caps.models.models
    );
    assert!(
        caps.sessions.iter().any(|s| s.id == "sess-1"),
        "a timed-out probe must NOT wipe the discovered sessions; got {:?}",
        caps.sessions
    );
    assert!(caps.cli_available, "a timeout can't prove the CLI is absent");
    assert_eq!(
        caps.errors,
        vec!["claude discovery: timed out".to_string()],
        "the failed probe's error is still surfaced"
    );
}

/// §3.5: "Missing `host_id`/`driver` is a serde `SchemaMismatch` →
/// `CoreError::Protocol`, consistent with the M2 handler contract."
#[test]
fn on_agent_line_capabilities_update_missing_host_id_is_protocol_error() {
    let (mut core, _peer) = make_core();
    let line = r#"{"type":"CapabilitiesUpdate","driver":"codex","capabilities":{"driver":"codex","host_id":"local"}}"#;
    let err = core
        .on_agent_line(line)
        .expect_err("missing host_id must be a hard Protocol error, not tolerant-skip");
    assert!(
        matches!(err, CoreError::Protocol(_)),
        "expected CoreError::Protocol, got {err:?}"
    );
}

/// §3.5: `CapabilitiesRefreshQueued` "joins the existing ack no-op arm" →
/// `Ok(vec![])`.
#[test]
fn on_agent_line_capabilities_refresh_queued_is_ack_noop() {
    let (mut core, _peer) = make_core();
    let updates = core
        .on_agent_line(r#"{"type":"CapabilitiesRefreshQueued","host_id":"local"}"#)
        .expect("ack-class inbound must not error");
    // `VmUpdate` does not derive `PartialEq` (it carries large view payloads
    // that are constructed and immediately consumed, never compared) — assert
    // emptiness structurally instead of via `assert_eq!(updates, vec![])`.
    assert!(
        updates.is_empty(),
        "CapabilitiesRefreshQueued is a pure ack — no VmUpdate"
    );
}

/// A `RefreshCapabilities` message (GUI/core → agent direction) arriving
/// INBOUND on the agent channel is a routing bug — same disposition as every
/// other misdirected GUI→agent message (typed-ipc.md §3, the `on_agent_line`
/// catch-all `other => Err(CoreError::Protocol(...))` arm).
#[test]
fn on_agent_line_misdirected_refresh_capabilities_is_protocol_error() {
    let (mut core, _peer) = make_core();
    let err = core
        .on_agent_line(r#"{"type":"RefreshCapabilities"}"#)
        .expect_err("a GUI->agent message arriving inbound must be a routing-bug Protocol error");
    assert!(
        matches!(err, CoreError::Protocol(_)),
        "expected CoreError::Protocol, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Core::refresh_capabilities() — one IPC line, never waits (§3.5, §4 item 2)
// ---------------------------------------------------------------------------

/// §3.5: "`Core::refresh_capabilities(...)` → `send_ipc(&IpcMsg::RefreshCapabilities{...})`
/// — a single line write... never waits for a reply." §6: "`refresh_capabilities()`
/// emits the exact typed line (asserted through `parse_ipc`, M2 style)."
#[test]
fn refresh_capabilities_emits_exact_typed_line_all_fields() {
    let (mut core, mut peer) = make_core();
    core.refresh_capabilities(
        Some("hpc".to_string()),
        Some("codex".to_string()),
        Some("/work/mewais/OpenSource/Marsad".to_string()),
    );

    let outbound = peer.drain_outbound();
    assert_eq!(
        outbound.len(),
        1,
        "refresh_capabilities must send exactly one IPC line"
    );
    let parsed = parse_ipc(&outbound[0]).expect("outbound line must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::RefreshCapabilities {
            host_id: Some("hpc".to_string()),
            driver: Some("codex".to_string()),
            project: Some("/work/mewais/OpenSource/Marsad".to_string()),
            // "hpc" is not a registered host here, so the launch config resolves
            // to defaults (the agent then discovers on its local machine).
            host_target: None,
            setup: None,
            agent_commands: vec![],
            claude_bin: None,
        }
    );
}

/// A registered remote host's launch config (target / setup / agent_commands /
/// claude_bin) must be RESOLVED from the registry and threaded into the outbound
/// `RefreshCapabilities` line — this is what lets the agent's probe run
/// setup-first over the right connection with the exact per-driver command
/// (deliverable #1). Proves the resolution is live, not defaulted.
#[test]
fn refresh_capabilities_threads_registered_host_launch_config() {
    let (mut core, mut peer) = make_core();
    // Register a remote host carrying a full launch config.
    let updates = core
        .add_host(
            "HPC",
            "user@hpc.example",
            Some("genie".to_string()),
            Some("module load cuda".to_string()),
            vec![("PATH".to_string(), "/opt/bin".to_string())],
            vec![("codex".to_string(), "codex --sandbox".to_string())],
        )
        .expect("add_host");
    // Recover the generated host id from the returned Hosts update.
    let host_id = match updates.into_iter().find_map(|u| match u {
        VmUpdate::Hosts(list) => Some(list),
        _ => None,
    }) {
        Some(list) => list
            .into_iter()
            .find(|h| h.target == "user@hpc.example")
            .expect("added host present")
            .id,
        None => panic!("add_host must return a Hosts update"),
    };
    let _ = peer.drain_outbound();

    core.refresh_capabilities(Some(host_id.clone()), None, None);

    let outbound = peer.drain_outbound();
    assert_eq!(outbound.len(), 1);
    let parsed = parse_ipc(&outbound[0]).expect("outbound line must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::RefreshCapabilities {
            host_id: Some(host_id),
            driver: None,
            project: None,
            host_target: Some("user@hpc.example".to_string()),
            setup: Some("module load cuda".to_string()),
            // The registry folds `claude_bin` into `agent_commands["claude"]`
            // (marsad_hosts::fold_claude_bin), so both survive: the codex
            // override AND the resolved claude entry.
            agent_commands: vec![
                ("codex".to_string(), "codex --sandbox".to_string()),
                ("claude".to_string(), "genie".to_string()),
            ],
            claude_bin: Some("genie".to_string()),
        }
    );
}

/// Connect-time discovery (deliverable #4a): `refresh_capabilities_all_hosts()`
/// must emit ONE `RefreshCapabilities` line per registered host (all drivers,
/// `driver:None`), each carrying that host's resolved launch config. Proves the
/// startup trigger truly fans out over every host, not just the frontend's
/// explicit target.
#[test]
fn refresh_capabilities_all_hosts_emits_one_line_per_registered_host() {
    let (mut core, mut peer) = make_core();
    // Register two remote hosts; localhost is seeded automatically → 3 total.
    let _ = core
        .add_host("A", "user@a", None, None, vec![], vec![])
        .expect("add A");
    let _ = core
        .add_host(
            "B",
            "user@b",
            Some("genie".to_string()),
            Some("setup-b".to_string()),
            vec![],
            vec![],
        )
        .expect("add B");
    let _ = peer.drain_outbound();

    core.refresh_capabilities_all_hosts();

    let outbound = peer.drain_outbound();
    assert_eq!(
        outbound.len(),
        3,
        "one RefreshCapabilities per host (localhost + A + B)"
    );
    // Every line is a driver-None (all-drivers) RefreshCapabilities.
    let mut targets: Vec<Option<String>> = outbound
        .iter()
        .map(|l| match parse_ipc(l).expect("valid IpcMsg") {
            IpcMsg::RefreshCapabilities {
                driver,
                host_target,
                ..
            } => {
                assert_eq!(driver, None, "all-hosts refresh must probe all drivers");
                host_target
            }
            other => panic!("expected RefreshCapabilities, got {other:?}"),
        })
        .collect();
    targets.sort();
    // localhost resolves to host_target None (discovered locally); the two
    // remote hosts carry their real targets.
    assert_eq!(
        targets,
        vec![
            None,
            Some("user@a".to_string()),
            Some("user@b".to_string())
        ]
    );
}

/// The all-`None` form (§2's minimal golden: "None = all") must round-trip
/// through `refresh_capabilities` too — proves the fn doesn't force a default
/// host/driver.
#[test]
fn refresh_capabilities_emits_exact_typed_line_all_none() {
    let (mut core, mut peer) = make_core();
    core.refresh_capabilities(None, None, None);

    let outbound = peer.drain_outbound();
    assert_eq!(outbound.len(), 1);
    let parsed = parse_ipc(&outbound[0]).expect("outbound line must be valid IpcMsg");
    assert_eq!(
        parsed,
        IpcMsg::RefreshCapabilities {
            host_id: None,
            driver: None,
            project: None,
            host_target: None,
            setup: None,
            agent_commands: vec![],
            claude_bin: None,
        }
    );
}

/// §4 item 2 (the core-level proving test, verbatim): "call
/// `refresh_capabilities(...)` on a Core with a fake transport, then
/// immediately call `board()` / `capabilities()` and assert both return
/// current state without touching the transport reply path (there is
/// nothing to wait ON — pinned so a future 'wait for fresh data' regression
/// cannot land silently)."
///
/// Proof shape: `refresh_capabilities` is fire-and-forget (one send, no
/// recv), so `board()`/`capabilities()` succeeding immediately afterward,
/// AND `drain_outbound()` showing no MORE than the single `RefreshCapabilities`
/// line, together demonstrate neither accessor performed any additional
/// transport IO (a lock-clone-return implementation cannot emit wire traffic;
/// an implementation that mistakenly blocked on/awaited a reply would either
/// hang this single-threaded test or send a second outbound line trying to
/// solicit one — this asserts neither happened).
#[test]
fn refresh_capabilities_then_board_and_capabilities_return_immediately_without_more_io() {
    let (mut core, mut peer) = make_core();

    // Seed one cached entry so `capabilities()` has something to reflect.
    core.on_agent_line(&capabilities_update_line_codex())
        .expect("seed update must apply");
    let _ = peer.drain_outbound(); // discard anything from the seed step

    core.refresh_capabilities(Some("local".to_string()), Some("codex".to_string()), None);

    // Immediately, on the SAME thread, with nothing having replied on `peer`:
    let _board = core.board();
    let snapshot = core.capabilities();
    assert!(
        snapshot.iter().any(|v| v.driver == "codex" && v.host_id == "local"),
        "capabilities() must reflect the pre-existing cached entry without waiting for a fresh probe"
    );

    // Exactly the one RefreshCapabilities line — board()/capabilities() sent nothing.
    let outbound = peer.drain_outbound();
    assert_eq!(
        outbound.len(),
        1,
        "board()/capabilities() must not perform additional transport IO; outbound: {outbound:?}"
    );
    let parsed = parse_ipc(&outbound[0]).expect("must be valid IpcMsg");
    assert!(matches!(parsed, IpcMsg::RefreshCapabilities { .. }));
}

// ---------------------------------------------------------------------------
// Core::capabilities() — snapshot accessor (§3.5, §6)
// ---------------------------------------------------------------------------

/// §6: "`capabilities()` snapshot reflects upserts."
#[test]
fn capabilities_snapshot_reflects_upserts_across_distinct_keys() {
    let (mut core, _peer) = make_core();
    assert_eq!(
        core.capabilities(),
        vec![],
        "no capabilities before any update"
    );

    core.on_agent_line(&capabilities_update_line_codex())
        .expect("first key must apply");
    core.on_agent_line(&capabilities_update_line_claude_hpc())
        .expect("second, distinct key must apply");

    let snapshot = core.capabilities();
    assert_eq!(
        snapshot.len(),
        2,
        "two distinct (driver, host) keys must both be present"
    );
    assert!(snapshot
        .iter()
        .any(|v| v.driver == "codex" && v.host_id == "local"));
    assert!(snapshot
        .iter()
        .any(|v| v.driver == "claude" && v.host_id == "hpc"));
}
