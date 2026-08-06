//! wasl_peer_registration.rs — tests for dynamic Wasl peer registration.
//!
//! Verifies that `Core::add_connection` emits `RegisterWaslPeer` IPC to the
//! agent for the receiving node's host, and `Core::remove_connection` emits
//! `UnregisterWaslPeer`. The host_id in the IPC is the `to` node's host; the
//! peer_id is `marsad-<from_host>`.
//!
//! Uses `LoopbackFake::drain_outbound()` to capture IPC lines sent by Core.

use marsad_core::{Core, CoreConfig, LaunchSpec, VmUpdate};
use marsad_proto::{parse_ipc, IpcMsg};
use marsad_store::MemoryBackend;
use marsad_transport::LoopbackFake;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn make_core() -> (Core, LoopbackFake) {
    let (a, b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://peer-test".to_string(),
        data_root: "/tmp/marsad-peer-test".to_string(),
    };
    let core = Core::with_deps(
        cfg,
        Box::new(a),
        Box::new(marsad_core::RealGit),
        Box::new(marsad_core::RealTranscript),
        Box::new(MemoryBackend::new()),
    )
    .expect("with_deps");
    (core, b)
}

fn launch_on_host(core: &mut Core, project: &str, host_id: &str) -> String {
    core.launch_session(LaunchSpec {
        project: project.to_string(),
        host_id: host_id.to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "claude-opus-4-5".to_string(),
        session_mode: String::new(),
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "claude".to_string(),
    })
    .expect("launch_session")
}

/// Collect all `RegisterWaslPeer` IPC messages from a set of drained lines.
fn register_peers(lines: &[String]) -> Vec<(String, String, String)> {
    // Returns Vec<(host_id, peer_id, peer_host)>
    lines
        .iter()
        .filter_map(|l| match parse_ipc(l) {
            Ok(IpcMsg::RegisterWaslPeer {
                host_id,
                peer_id,
                peer_host,
                ..
            }) => Some((host_id, peer_id, peer_host)),
            _ => None,
        })
        .collect()
}

/// Collect all `UnregisterWaslPeer` IPC messages from a set of drained lines.
fn unregister_peers(lines: &[String]) -> Vec<(String, String)> {
    // Returns Vec<(host_id, peer_id)>
    lines
        .iter()
        .filter_map(|l| match parse_ipc(l) {
            Ok(IpcMsg::UnregisterWaslPeer { host_id, peer_id }) => Some((host_id, peer_id)),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Test: add_connection emits RegisterWaslPeer for the `to` node's host
// ---------------------------------------------------------------------------

#[test]
fn add_connection_emits_register_wasl_peer_for_to_host() {
    let (mut core, mut peer) = make_core();

    // Two sessions on different hosts.
    let s_from = launch_on_host(&mut core, "proj-a", "host-a");
    let s_to = launch_on_host(&mut core, "proj-b", "host-b");
    let _ = peer.drain_outbound(); // discard LaunchSession IPCs

    // Set up a workstream.
    core.create_workstream("ws-1".to_string(), "W1".to_string(), String::new(), None)
        .expect("create_workstream");
    core.assign_session(&s_from, "ws-1", true)
        .expect("assign from");
    core.assign_session(&s_to, "ws-1", false)
        .expect("assign to");
    let _ = peer.drain_outbound();

    // Add a directed connection from→to.
    core.add_connection(
        "conn-1".to_string(),
        "ws-1".to_string(),
        s_from.clone(),
        s_to.clone(),
        false, // not bidirectional
    )
    .expect("add_connection");

    let sent = peer.drain_outbound();
    let regs = register_peers(&sent);

    // Exactly one RegisterWaslPeer: `from` (host-a) registered on `to` (host-b).
    assert_eq!(regs.len(), 1, "exactly one RegisterWaslPeer for directed connection");
    assert_eq!(regs[0].0, "host-b", "host_id must be the to-node's host");
    assert_eq!(regs[0].1, "marsad-host-a", "peer_id must be marsad-<from_host>");
    assert_eq!(regs[0].2, "host-a", "peer_host must be the from-node's host");
}

// ---------------------------------------------------------------------------
// Test: bidirectional add_connection emits two RegisterWaslPeer messages
// ---------------------------------------------------------------------------

#[test]
fn add_bidirectional_connection_emits_two_register_wasl_peer() {
    let (mut core, mut peer) = make_core();

    let s_a = launch_on_host(&mut core, "proj-a", "host-a");
    let s_b = launch_on_host(&mut core, "proj-b", "host-b");
    let _ = peer.drain_outbound();

    core.create_workstream("ws-bi".to_string(), "WBi".to_string(), String::new(), None)
        .expect("create");
    core.assign_session(&s_a, "ws-bi", true).expect("assign a");
    core.assign_session(&s_b, "ws-bi", false).expect("assign b");
    let _ = peer.drain_outbound();

    // Bidirectional connection.
    core.add_connection(
        "conn-bi".to_string(),
        "ws-bi".to_string(),
        s_a.clone(),
        s_b.clone(),
        true, // bidirectional
    )
    .expect("add_connection bi");

    let sent = peer.drain_outbound();
    let regs = register_peers(&sent);

    // Two registrations: host-a→host-b and host-b→host-a.
    assert_eq!(regs.len(), 2, "two RegisterWaslPeer for bidirectional connection");
    // The first is from→to direction (host-a registered on host-b).
    let from_to: Vec<_> = regs
        .iter()
        .filter(|(h, _, _)| h == "host-b")
        .collect();
    let to_from: Vec<_> = regs
        .iter()
        .filter(|(h, _, _)| h == "host-a")
        .collect();
    assert_eq!(from_to.len(), 1, "one register on host-b");
    assert_eq!(from_to[0].1, "marsad-host-a");
    assert_eq!(to_from.len(), 1, "one register on host-a");
    assert_eq!(to_from[0].1, "marsad-host-b");
}

// ---------------------------------------------------------------------------
// Test: remove_connection emits UnregisterWaslPeer
// ---------------------------------------------------------------------------

#[test]
fn remove_connection_emits_unregister_wasl_peer() {
    let (mut core, mut peer) = make_core();

    let s_from = launch_on_host(&mut core, "proj-a", "host-a");
    let s_to = launch_on_host(&mut core, "proj-b", "host-b");
    let _ = peer.drain_outbound();

    core.create_workstream("ws-rm".to_string(), "WRm".to_string(), String::new(), None)
        .expect("create");
    core.assign_session(&s_from, "ws-rm", true).expect("assign from");
    core.assign_session(&s_to, "ws-rm", false).expect("assign to");

    core.add_connection(
        "conn-rm".to_string(),
        "ws-rm".to_string(),
        s_from.clone(),
        s_to.clone(),
        false,
    )
    .expect("add_connection");
    let _ = peer.drain_outbound(); // discard the RegisterWaslPeer

    // Now remove the connection.
    core.remove_connection("conn-rm").expect("remove_connection");

    let sent = peer.drain_outbound();
    let unregs = unregister_peers(&sent);

    assert_eq!(unregs.len(), 1, "exactly one UnregisterWaslPeer");
    assert_eq!(unregs[0].0, "host-b", "host_id must be the to-node's host");
    assert_eq!(unregs[0].1, "marsad-host-a", "peer_id must be marsad-<from_host>");
}

// ---------------------------------------------------------------------------
// Test: add_connection still succeeds even when session has no host_id resolved
// (unknown session → best-effort skip, no panic, no error)
// ---------------------------------------------------------------------------

#[test]
fn add_connection_succeeds_and_returns_board_update() {
    let (mut core, mut peer) = make_core();

    let s1 = launch_on_host(&mut core, "proj-1", "local");
    let s2 = launch_on_host(&mut core, "proj-2", "local");
    let _ = peer.drain_outbound();

    core.create_workstream("ws-ok".to_string(), "WOk".to_string(), String::new(), None)
        .expect("create");
    core.assign_session(&s1, "ws-ok", true).expect("assign s1");
    core.assign_session(&s2, "ws-ok", false).expect("assign s2");

    let updates = core
        .add_connection(
            "conn-ok".to_string(),
            "ws-ok".to_string(),
            s1.clone(),
            s2.clone(),
            false,
        )
        .expect("add_connection must not fail");

    // Board VmUpdate must be present.
    assert!(
        updates.iter().any(|u| matches!(u, VmUpdate::Board(_))),
        "add_connection must return Board VmUpdate"
    );
}

// ---------------------------------------------------------------------------
// Test: remove_connection of absent id is a no-op (no panic, no IPC)
// ---------------------------------------------------------------------------

#[test]
fn remove_connection_absent_id_is_no_op() {
    let (mut core, mut peer) = make_core();

    let updates = core
        .remove_connection("does-not-exist")
        .expect("absent id must be no-op Ok");

    assert!(
        updates.iter().any(|u| matches!(u, VmUpdate::Board(_))),
        "remove_connection absent id must return Board VmUpdate"
    );

    let sent = peer.drain_outbound();
    let unregs = unregister_peers(&sent);
    assert!(
        unregs.is_empty(),
        "absent id must emit no UnregisterWaslPeer"
    );
}
