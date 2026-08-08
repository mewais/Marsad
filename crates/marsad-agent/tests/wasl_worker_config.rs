//! wasl_worker_config.rs — unit tests for the Phase-D Wasl worker MCP config
//! pure builders and their injection into the `LaunchWaslSession` launch path.
//!
//! # Coverage
//! 1. `render_wasl_worker_toml` with 0 peers — `[identity]` present, NO
//!    `[[peers]]`, NO `[runtime]`.
//! 2. `render_wasl_worker_toml` with 2 peers — two `[[peers]]` blocks in order,
//!    each with the exact `agent_id`/`host_id`/`url`; `url` starts with `http://`.
//! 3. `wasl_worker_mcp_config` — parses back to JSON with
//!    `mcpServers.wasl.command == "wasl-mcpd"` and `args == ["--config", path]`.
//! 4. A `LaunchWaslSession` whose context carries `peers` injects
//!    `context.mcp_config` (a string containing `"wasl-mcpd"`) into the key
//!    handed to `get_or_start` — proven via the seam in `wasl_session.rs`'s
//!    `SeededProvider` which records the canonical key string it was called with.

use std::sync::{Arc, Mutex};

use marsad_agent::{
    render_wasl_worker_toml, wasl_worker_mcp_config, Agent, AgentConfig, ListenSpec, WaslWorkerPeer,
};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::{encode_ipc, IpcMsg};
use marsad_store::{MemoryBackend, Store};
use marsad_wasl_session::{
    WaslSessionProvider, WaslTargetKey, WaslTargetSnapshot,
    WaslSessionError, WaslSessionClient, WaslCapabilities, WaslTargetStatus, WaslTurnReceipt,
    WaslEventStream, WaslGateDecision, WaslTurnId, WaslBusyPolicy, WaslCancelReason,
    WaslShutdownMode, WaslTurnRequest, FakeWaslSessionClient,
};

// ---------------------------------------------------------------------------
// §1 — `render_wasl_worker_toml` with 0 peers
// ---------------------------------------------------------------------------

/// With zero peers the TOML must have `[identity]` with the exact agent_id and
/// host_id, no `[[peers]]` blocks, and no `[runtime]` block.
#[test]
fn render_toml_zero_peers_has_identity_no_peers_no_runtime() {
    let toml = render_wasl_worker_toml("marsad-local", "local", &[]);

    // Must contain [identity] block.
    assert!(
        toml.contains("[identity]"),
        "TOML must have an [identity] block; got:\n{toml}"
    );
    // Must have the exact agent_id and host_id.
    assert!(
        toml.contains(r#"agent_id = "marsad-local""#),
        "identity.agent_id must be present; got:\n{toml}"
    );
    assert!(
        toml.contains(r#"host_id = "local""#),
        "identity.host_id must be present; got:\n{toml}"
    );
    // Must NOT have any [[peers]] block.
    assert!(
        !toml.contains("[[peers]]"),
        "zero peers must produce no [[peers]] block; got:\n{toml}"
    );
    // Must NOT have a [runtime] block.
    assert!(
        !toml.contains("[runtime]"),
        "a tools client must NOT have a [runtime] block; got:\n{toml}"
    );
}

// ---------------------------------------------------------------------------
// §2 — `render_wasl_worker_toml` with 2 peers
// ---------------------------------------------------------------------------

/// With two peers the TOML must have two `[[peers]]` blocks in the given
/// order, each carrying the exact `agent_id`, `host_id`, and `url`. The url
/// must start with `http://`.
#[test]
fn render_toml_two_peers_in_order_with_correct_fields() {
    let peers = vec![
        WaslWorkerPeer {
            agent_id: "agent-a".to_string(),
            host_id: "host-a".to_string(),
            url: "http://127.0.0.1:48001".to_string(),
        },
        WaslWorkerPeer {
            agent_id: "agent-b".to_string(),
            host_id: "host-b".to_string(),
            url: "http://127.0.0.1:48002".to_string(),
        },
    ];

    let toml = render_wasl_worker_toml("marsad-local", "local", &peers);

    // Exactly two [[peers]] blocks.
    let peer_block_count = toml.matches("[[peers]]").count();
    assert_eq!(
        peer_block_count, 2,
        "two peers must produce exactly two [[peers]] blocks; got:\n{toml}"
    );

    // Peer A fields are present.
    assert!(
        toml.contains(r#"agent_id = "agent-a""#),
        "peer A agent_id must be present; got:\n{toml}"
    );
    assert!(
        toml.contains(r#"host_id = "host-a""#),
        "peer A host_id must be present; got:\n{toml}"
    );
    assert!(
        toml.contains(r#"url = "http://127.0.0.1:48001""#),
        "peer A url must be present; got:\n{toml}"
    );

    // Peer B fields are present.
    assert!(
        toml.contains(r#"agent_id = "agent-b""#),
        "peer B agent_id must be present; got:\n{toml}"
    );
    assert!(
        toml.contains(r#"host_id = "host-b""#),
        "peer B host_id must be present; got:\n{toml}"
    );
    assert!(
        toml.contains(r#"url = "http://127.0.0.1:48002""#),
        "peer B url must be present; got:\n{toml}"
    );

    // All URLs start with http://.
    for line in toml.lines().filter(|l| l.trim_start().starts_with("url")) {
        let url_val = line
            .splitn(2, '=')
            .nth(1)
            .unwrap_or("")
            .trim()
            .trim_matches('"');
        assert!(
            url_val.starts_with("http://"),
            "peer url must start with http://; got: {url_val}"
        );
    }

    // Order: A must appear before B in the output.
    let pos_a = toml
        .find(r#"agent_id = "agent-a""#)
        .expect("agent-a must be present");
    let pos_b = toml
        .find(r#"agent_id = "agent-b""#)
        .expect("agent-b must be present");
    assert!(
        pos_a < pos_b,
        "peers must appear in the given order (A before B); got:\n{toml}"
    );
}

/// No `[runtime]` block even with peers.
#[test]
fn render_toml_with_peers_has_no_runtime_block() {
    let peers = vec![WaslWorkerPeer {
        agent_id: "agent-x".to_string(),
        host_id: "host-x".to_string(),
        url: "http://127.0.0.1:48100".to_string(),
    }];
    let toml = render_wasl_worker_toml("marsad-x", "x", &peers);
    assert!(
        !toml.contains("[runtime]"),
        "a tools client must NOT have a [runtime] block even with peers; got:\n{toml}"
    );
}

// ---------------------------------------------------------------------------
// §3 — `wasl_worker_mcp_config` JSON round-trip
// ---------------------------------------------------------------------------

/// `wasl_worker_mcp_config("/x/y.toml")` must parse back to JSON with
/// `mcpServers.wasl.command == "wasl-mcpd"`, EMPTY `args`, and the config path in
/// `env.WASL_CONFIG` — wasl-mcpd reads its config from the `WASL_CONFIG` env var,
/// NOT a `--config` flag (which it ignores → default config → useless send-tools).
#[test]
fn wasl_worker_mcp_config_parses_back_correctly() {
    let content = "[identity]\nagent_id = \"a\"\nhost_id = \"h\"\n";
    let json_str = wasl_worker_mcp_config(content);

    let v: serde_json::Value =
        serde_json::from_str(&json_str).expect("wasl_worker_mcp_config must return valid JSON");

    assert_eq!(
        v["mcpServers"]["wasl"]["command"].as_str(),
        Some("wasl-mcpd"),
        "command must be wasl-mcpd"
    );

    let args = v["mcpServers"]["wasl"]["args"]
        .as_array()
        .expect("args must be an array");
    assert!(
        args.is_empty(),
        "args must be empty — the config is delivered via the WASL_CONFIG_CONTENT env var, \
         not a --config flag (wasl-mcpd ignores --config)"
    );
    assert_eq!(
        v["mcpServers"]["wasl"]["env"]["WASL_CONFIG_CONTENT"].as_str(),
        Some(content),
        "the worker TOML must be delivered INLINE via WASL_CONFIG_CONTENT (machine-independent)"
    );
    assert!(
        v["mcpServers"]["wasl"]["env"].get("WASL_CONFIG").is_none(),
        "no filesystem path is embedded — delivery is by content, not a cross-host path"
    );
}

/// Multi-line TOML content (newlines + quotes) must be correctly JSON-escaped by
/// `serde_json` and round-trip through the `env.WASL_CONFIG_CONTENT` value.
#[test]
fn wasl_worker_mcp_config_escapes_multiline_toml_content() {
    let content = "[identity]\nagent_id = \"draft-node-x\"\n\n[[peers]]\nagent_id = \"p\"\nurl = \"http://127.0.0.1:48161\"\n";
    let json_str = wasl_worker_mcp_config(content);
    let v: serde_json::Value = serde_json::from_str(&json_str)
        .expect("multi-line TOML content must still produce valid JSON");
    assert_eq!(
        v["mcpServers"]["wasl"]["env"]["WASL_CONFIG_CONTENT"].as_str(),
        Some(content),
        "multi-line TOML content must round-trip through JSON via WASL_CONFIG_CONTENT"
    );
}

// ---------------------------------------------------------------------------
// §4 — `LaunchWaslSession` with peers injects `context.mcp_config` into the
//       key handed to `get_or_start`.
//
// There IS a provider-recording seam: `wasl_session.rs`'s test infrastructure
// (the `SeededProvider` / `ArcProvider` wrappers) records the canonical JSON
// of every key handed to `get_or_start`. We replicate a minimal version of
// that pattern here (a `RecordingProvider` that captures the full key) so we
// can assert `context.mcp_config` is set and contains `"wasl-mcpd"`.
// ---------------------------------------------------------------------------

/// A `WaslSessionProvider` that records every key passed to `get_or_start`.
struct RecordingProvider {
    keys: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl RecordingProvider {
    fn new() -> (Self, Arc<Mutex<Vec<serde_json::Value>>>) {
        let keys = Arc::new(Mutex::new(Vec::new()));
        (Self { keys: keys.clone() }, keys)
    }
}

/// Minimal fake client returned by `RecordingProvider::get_or_start`.
#[derive(Debug)]
struct MinimalClient;

impl WaslSessionClient for MinimalClient {
    fn target_id(&self) -> &str { "recording-target" }
    fn capabilities(&self) -> WaslCapabilities {
        WaslCapabilities {
            persistent_process: true,
            persistent_conversation: true,
            structured_turns: true,
            streaming_output: true,
            attachable: true,
            interruptible: true,
            structured_provenance: true,
            resumable: true,
        }
    }
    fn status(&self) -> WaslTargetStatus {
        WaslTargetStatus::Idle { since: String::new() }
    }
    fn prompt(&self, _req: WaslTurnRequest, _policy: WaslBusyPolicy) -> WaslTurnReceipt {
        WaslTurnReceipt::Started { turn_id: "t".to_string(), first_seq: 0 }
    }
    fn attach(&self, _after_seq: u64) -> Box<dyn WaslEventStream> {
        FakeWaslSessionClient::new("recording-target", self.capabilities()).attach(0)
    }
    fn resolve_native_gate(&self, _: &WaslTurnId, _: WaslGateDecision) -> Result<(), WaslSessionError> {
        Ok(())
    }
    fn interrupt(&self, _: WaslCancelReason) -> Result<(), WaslSessionError> { Ok(()) }
    fn shutdown(&self, _: WaslShutdownMode) -> Result<(), WaslSessionError> { Ok(()) }
}

impl WaslSessionProvider for RecordingProvider {
    fn get_or_start(
        &self,
        key: &WaslTargetKey,
    ) -> Result<(Box<dyn WaslSessionClient>, WaslTargetSnapshot), WaslSessionError> {
        self.keys.lock().unwrap().push(key.0.clone());
        let snap = WaslTargetSnapshot {
            target_id: "recording-target".to_string(),
            key: WaslTargetKey(key.0.clone()),
            capabilities: MinimalClient.capabilities(),
            status: WaslTargetStatus::Idle { since: String::new() },
            last_seq: 0,
            queued: 0,
            native_session_id: None,
        };
        Ok((Box::new(MinimalClient), snap))
    }
    fn list(&self) -> Result<Vec<String>, WaslSessionError> {
        Ok(vec!["recording-target".to_string()])
    }
    fn register_peer(&self, _: marsad_wasl_session::WaslAgentCard) -> Result<(), WaslSessionError> {
        Ok(())
    }
    fn unregister_peer(&self, _: &str) -> Result<bool, WaslSessionError> { Ok(false) }
    fn list_peers(&self) -> Result<Vec<marsad_wasl_session::WaslAgentCard>, WaslSessionError> {
        Ok(vec![])
    }
}

fn make_agent() -> Agent {
    Agent::with_fakes(
        AgentConfig {
            listen: ListenSpec::Stdio,
            data_root: "/tmp/marsad-agent-wasl-worker-config-test".to_string(),
        },
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes")
}

/// A `LaunchWaslSession` with `context.peers` must inject `context.mcp_config`
/// (a JSON string containing `"wasl-mcpd"`) into the key handed to `get_or_start`.
#[test]
fn launch_with_peers_injects_mcp_config_into_get_or_start_key() {
    let mut agent = make_agent();
    let (provider, keys) = RecordingProvider::new();
    agent.register_wasl_session_provider(Box::new(provider));

    // Build a LaunchWaslSession key with two peers.
    let target_key = serde_json::json!({
        "protocol": "claude",
        "context": {
            "cwd": "/work/proj",
            "model": "claude-opus-4-5",
            "session_mode": "",
            "peers": [
                {"agent_id": "agent-peer1", "host_id": "peer-host-1"},
                {"agent_id": "agent-peer2", "host_id": "peer-host-2"}
            ]
        }
    });

    let line = encode_ipc(&IpcMsg::LaunchWaslSession {
        session_id: "sess-worker-test".to_string(),
        host_id: "local".to_string(),
        wasl_target_key: target_key,
        name: None,
    });

    let result = agent.handle_ipc(&line);
    // The launch may fail (no live node in FakeContainer) or succeed; what
    // matters is that get_or_start was called with the enriched key.
    // Either way, the recording provider must have seen the key.
    let _ = result; // don't assert Ok/Err — just check the recorded key

    let recorded = keys.lock().unwrap();
    assert!(
        !recorded.is_empty(),
        "get_or_start must be called; key was not recorded (provider never invoked)"
    );

    let received_key = &recorded[0];

    // `context` must be an object.
    let context = received_key
        .get("context")
        .expect("the mutated key must have a 'context' field");
    assert!(
        context.is_object(),
        "context must be a JSON object in the key handed to get_or_start; got: {context:?}"
    );

    // `context.mcp_config` must be a string.
    let mcp_config = context
        .get("mcp_config")
        .expect("context.mcp_config must be set before get_or_start")
        .as_str()
        .expect("context.mcp_config must be a JSON string");

    // The string must contain "wasl-mcpd" (it is the compact JSON for the MCP
    // server config — `{"mcpServers":{"wasl":{"command":"wasl-mcpd",...}}}`).
    assert!(
        mcp_config.contains("wasl-mcpd"),
        "context.mcp_config must reference wasl-mcpd; got: {mcp_config}"
    );

    // The string must be valid JSON parseable as an mcpServers config.
    let cfg: serde_json::Value =
        serde_json::from_str(mcp_config).expect("context.mcp_config must be valid JSON");
    assert_eq!(
        cfg["mcpServers"]["wasl"]["command"].as_str(),
        Some("wasl-mcpd"),
        "mcpServers.wasl.command must be wasl-mcpd"
    );
}

/// A `LaunchWaslSession` with NO peers still injects `context.mcp_config` —
/// even with zero peers the worker gets the wasl send-tools MCP.
#[test]
fn launch_with_no_peers_still_injects_mcp_config() {
    let mut agent = make_agent();
    let (provider, keys) = RecordingProvider::new();
    agent.register_wasl_session_provider(Box::new(provider));

    // Key with context but no peers.
    let target_key = serde_json::json!({
        "protocol": "claude",
        "context": {
            "cwd": "/work/proj",
            "model": "claude-opus-4-5",
            "session_mode": ""
        }
    });

    let line = encode_ipc(&IpcMsg::LaunchWaslSession {
        session_id: "sess-worker-nopeer".to_string(),
        host_id: "local".to_string(),
        wasl_target_key: target_key,
        name: None,
    });

    let _ = agent.handle_ipc(&line);

    let recorded = keys.lock().unwrap();
    assert!(!recorded.is_empty(), "get_or_start must be called");

    let received_key = &recorded[0];
    let mcp_config = received_key["context"]["mcp_config"]
        .as_str()
        .expect("context.mcp_config must be set even with zero peers");
    assert!(
        mcp_config.contains("wasl-mcpd"),
        "context.mcp_config must reference wasl-mcpd even with zero peers; got: {mcp_config}"
    );
}

/// A `LaunchWaslSession` with NO context field at all still injects
/// `context.mcp_config` (the handler creates a `{}` context if absent).
#[test]
fn launch_with_no_context_field_creates_context_with_mcp_config() {
    let mut agent = make_agent();
    let (provider, keys) = RecordingProvider::new();
    agent.register_wasl_session_provider(Box::new(provider));

    // Key without any context field.
    let target_key = serde_json::json!({ "protocol": "claude" });

    let line = encode_ipc(&IpcMsg::LaunchWaslSession {
        session_id: "sess-worker-nocontext".to_string(),
        host_id: "local".to_string(),
        wasl_target_key: target_key,
        name: None,
    });

    let _ = agent.handle_ipc(&line);

    let recorded = keys.lock().unwrap();
    assert!(!recorded.is_empty(), "get_or_start must be called");

    let received_key = &recorded[0];
    let mcp_config = received_key
        .get("context")
        .and_then(|c| c.get("mcp_config"))
        .and_then(|v| v.as_str())
        .expect("context.mcp_config must be set even when the key had no context field");
    assert!(
        mcp_config.contains("wasl-mcpd"),
        "context.mcp_config must reference wasl-mcpd; got: {mcp_config}"
    );
}
