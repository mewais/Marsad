//! wasl_peer_ipc.rs — tests for the `RegisterWaslPeer` / `UnregisterWaslPeer`
//! IPC handlers in `marsad-agent`.
//!
//! Verifies that:
//! 1. `RegisterWaslPeer` calls `provider.register_peer` with the correct card.
//! 2. `UnregisterWaslPeer` calls `provider.unregister_peer` with the correct id.
//! 3. Both handlers are best-effort: a missing provider logs an error but does
//!    NOT fail the IPC call (returns `Ok([])`).
//!
//! Uses `FakeWaslSessionProvider` as the seam (the existing `list_peers` API on
//! the fake is used to observe side-effects from `register_peer`/`unregister_peer`).

use marsad_agent::{Agent, AgentConfig, ListenSpec};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::{encode_ipc, IpcMsg};
use marsad_store::{MemoryBackend, Store};
use marsad_wasl_session::{FakeWaslSessionProvider, WaslAgentCard, WaslSessionProvider};

fn make_agent() -> Agent {
    Agent::with_fakes(
        AgentConfig {
            listen: ListenSpec::Stdio,
            data_root: "/tmp/marsad-agent-wasl-peer-test".to_string(),
        },
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes")
}

fn register_line(host_id: &str, peer_id: &str, peer_host: &str, endpoint: Option<&str>) -> String {
    encode_ipc(&IpcMsg::RegisterWaslPeer {
        host_id: host_id.to_string(),
        peer_id: peer_id.to_string(),
        peer_host: peer_host.to_string(),
        peer_endpoint: endpoint.map(|s| s.to_string()),
    })
}

fn unregister_line(host_id: &str, peer_id: &str) -> String {
    encode_ipc(&IpcMsg::UnregisterWaslPeer {
        host_id: host_id.to_string(),
        peer_id: peer_id.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Test: register_peer is called on the provider when RegisterWaslPeer arrives
// ---------------------------------------------------------------------------

#[test]
fn handle_register_wasl_peer_calls_provider_register() {
    let mut agent = make_agent();

    let provider = FakeWaslSessionProvider::new();
    // Wrap in Arc to inspect after the call (the provider is moved into the agent).
    use std::sync::Arc;
    let provider = Arc::new(provider);
    // Register a provider that delegates to the Arc'd fake.
    struct SharedProvider(Arc<FakeWaslSessionProvider>);
    impl WaslSessionProvider for SharedProvider {
        fn get_or_start(
            &self,
            key: &marsad_wasl_session::WaslTargetKey,
        ) -> Result<(Box<dyn marsad_wasl_session::WaslSessionClient>, marsad_wasl_session::WaslTargetSnapshot), marsad_wasl_session::WaslSessionError>
        {
            self.0.get_or_start(key)
        }
        fn list(&self) -> Result<Vec<String>, marsad_wasl_session::WaslSessionError> {
            self.0.list()
        }
        fn register_peer(&self, card: marsad_wasl_session::WaslAgentCard) -> Result<(), marsad_wasl_session::WaslSessionError> {
            self.0.register_peer(card)
        }
        fn unregister_peer(&self, id: &str) -> Result<bool, marsad_wasl_session::WaslSessionError> {
            self.0.unregister_peer(id)
        }
        fn list_peers(&self) -> Result<Vec<marsad_wasl_session::WaslAgentCard>, marsad_wasl_session::WaslSessionError> {
            self.0.list_peers()
        }
    }

    agent.register_wasl_session_provider(Box::new(SharedProvider(Arc::clone(&provider))));

    // Send RegisterWaslPeer.
    let line = register_line(
        "host-b",
        "marsad-host-a",
        "host-a",
        Some("http://127.0.0.1:48000"),
    );
    let out = agent.handle_ipc(&line).expect("handle_ipc must not error");
    // Fire-and-forget: no ack lines expected.
    assert!(out.is_empty(), "RegisterWaslPeer must return no ack lines");

    // The provider should now have the peer registered.
    let peers = provider.list_peers().expect("list_peers");
    assert_eq!(peers.len(), 1, "exactly one peer registered");
    assert_eq!(peers[0].id, "marsad-host-a");
    assert_eq!(peers[0].host, "host-a");
    assert_eq!(
        peers[0].endpoint,
        Some("http://127.0.0.1:48000".to_string())
    );
}

// ---------------------------------------------------------------------------
// Test: unregister_peer is called on the provider when UnregisterWaslPeer arrives
// ---------------------------------------------------------------------------

#[test]
fn handle_unregister_wasl_peer_calls_provider_unregister() {
    let mut agent = make_agent();

    let provider = FakeWaslSessionProvider::new();
    use std::sync::Arc;
    let provider = Arc::new(provider);

    struct SharedProvider(Arc<FakeWaslSessionProvider>);
    impl WaslSessionProvider for SharedProvider {
        fn get_or_start(&self, key: &marsad_wasl_session::WaslTargetKey) -> Result<(Box<dyn marsad_wasl_session::WaslSessionClient>, marsad_wasl_session::WaslTargetSnapshot), marsad_wasl_session::WaslSessionError> { self.0.get_or_start(key) }
        fn list(&self) -> Result<Vec<String>, marsad_wasl_session::WaslSessionError> { self.0.list() }
        fn register_peer(&self, card: marsad_wasl_session::WaslAgentCard) -> Result<(), marsad_wasl_session::WaslSessionError> { self.0.register_peer(card) }
        fn unregister_peer(&self, id: &str) -> Result<bool, marsad_wasl_session::WaslSessionError> { self.0.unregister_peer(id) }
        fn list_peers(&self) -> Result<Vec<marsad_wasl_session::WaslAgentCard>, marsad_wasl_session::WaslSessionError> { self.0.list_peers() }
    }

    agent.register_wasl_session_provider(Box::new(SharedProvider(Arc::clone(&provider))));

    // First register a peer, then unregister it.
    provider
        .register_peer(WaslAgentCard {
            id: "marsad-host-a".to_string(),
            host: "host-a".to_string(),
            endpoint: None,
        })
        .expect("register");

    let peers_before = provider.list_peers().expect("list before");
    assert_eq!(peers_before.len(), 1);

    let line = unregister_line("host-b", "marsad-host-a");
    let out = agent.handle_ipc(&line).expect("handle_ipc must not error");
    assert!(out.is_empty(), "UnregisterWaslPeer must return no ack lines");

    let peers_after = provider.list_peers().expect("list after");
    assert!(
        peers_after.is_empty(),
        "peer must have been removed after unregister"
    );
}

// ---------------------------------------------------------------------------
// Test: no provider — best-effort, returns Ok([])
// ---------------------------------------------------------------------------

#[test]
fn register_wasl_peer_without_provider_is_best_effort_ok() {
    let mut agent = make_agent();
    // No provider registered.
    let line = register_line("host-b", "marsad-host-a", "host-a", None);
    let result = agent.handle_ipc(&line);
    assert!(
        result.is_ok(),
        "RegisterWaslPeer must be best-effort Ok even without a provider"
    );
    assert!(
        result.unwrap().is_empty(),
        "must return no lines when no provider"
    );
}

#[test]
fn unregister_wasl_peer_without_provider_is_best_effort_ok() {
    let mut agent = make_agent();
    // No provider registered.
    let line = unregister_line("host-b", "marsad-host-a");
    let result = agent.handle_ipc(&line);
    assert!(
        result.is_ok(),
        "UnregisterWaslPeer must be best-effort Ok even without a provider"
    );
    assert!(
        result.unwrap().is_empty(),
        "must return no lines when no provider"
    );
}

// ---------------------------------------------------------------------------
// Test: register peer with no endpoint (None field omitted)
// ---------------------------------------------------------------------------

#[test]
fn register_wasl_peer_without_endpoint() {
    let mut agent = make_agent();

    let provider = FakeWaslSessionProvider::new();
    use std::sync::Arc;
    let provider = Arc::new(provider);

    struct SharedProvider(Arc<FakeWaslSessionProvider>);
    impl WaslSessionProvider for SharedProvider {
        fn get_or_start(&self, key: &marsad_wasl_session::WaslTargetKey) -> Result<(Box<dyn marsad_wasl_session::WaslSessionClient>, marsad_wasl_session::WaslTargetSnapshot), marsad_wasl_session::WaslSessionError> { self.0.get_or_start(key) }
        fn list(&self) -> Result<Vec<String>, marsad_wasl_session::WaslSessionError> { self.0.list() }
        fn register_peer(&self, card: marsad_wasl_session::WaslAgentCard) -> Result<(), marsad_wasl_session::WaslSessionError> { self.0.register_peer(card) }
        fn unregister_peer(&self, id: &str) -> Result<bool, marsad_wasl_session::WaslSessionError> { self.0.unregister_peer(id) }
        fn list_peers(&self) -> Result<Vec<marsad_wasl_session::WaslAgentCard>, marsad_wasl_session::WaslSessionError> { self.0.list_peers() }
    }

    agent.register_wasl_session_provider(Box::new(SharedProvider(Arc::clone(&provider))));

    let line = register_line("host-b", "marsad-host-a", "host-a", None);
    agent.handle_ipc(&line).expect("must not error");

    let peers = provider.list_peers().expect("list");
    assert_eq!(peers.len(), 1);
    assert!(peers[0].endpoint.is_none(), "endpoint must be None");
}
