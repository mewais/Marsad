//! wasl_session.rs — Wasl **Backend** (Wasl-managed session) agent bucket.
//!
//! Authoritative design: `docs/design/wasl-backend-plan.md` (v3), especially
//! §6.1 (agent-side `WaslSessionProvider` registry + observe threads), §7 (the
//! new `IpcMsg` variants — ALREADY implemented in `marsad-proto`, frozen at the
//! `gate_id`-keyed IPC shape — see the v3-IPC note below),
//! §8/§8.1 (native-gate relay + lock hygiene), and §10.3 (this file's test
//! plan). This is a DIFFERENT feature from the X1 operator connector tested in
//! `tests/wasl.rs`/`tests/wasl_lock_safety.rs`: here the session's process is
//! owned end-to-end by Wasl and Marsad is a thin CLIENT of a Wasl
//! `DeliveryTarget`. It runs ENTIRELY against `marsad_wasl_session`'s in-repo
//! fakes (`FakeWaslSessionClient`/`FakeWaslSessionProvider`) — no socket, no
//! real process — matching this crate's FAKES-only house convention
//! (`tests/happy.rs`'s top-of-file note) and mirroring the X1 `tests/wasl.rs`
//! substrate.
//!
//! ── NOTE(test-agent): INVENTED / ASSUMED AGENT SURFACE ───────────────────────
//! `docs/design/wasl-backend-plan.md` is a DESIGN doc, not a per-crate spec (no
//! `docs/specs/wasl-backend.md` exists yet). §6.1/§7/§8 pin the *behavior* but
//! not the exact Rust seam names on `marsad_agent::Agent`. This file assumes the
//! seams below, chosen to mirror the EXISTING X1 seams EXACTLY (the design doc
//! says to do so verbatim — §6.1 "structurally identical to … the X1
//! `wasl_connectors` registry", §7's `set_wasl_session_provider_factory` is
//! "the direct analogue of `set_wasl_connector_factory`"). If the code-agent
//! lands differently-shaped seams, it should adjust these call sites — the
//! BEHAVIOR pinned here (absent = inert; a launch registers a client via the
//! factory; prompt/gate/interrupt/stop reach the fake's logs; observe events map
//! to typed `WaslSessionEvent`/`WaslSessionStatus` lines; NO registry lock is
//! held across a blocking client call) is the actual contract, not the method
//! names.
//!
//!   1. `Agent::set_wasl_session_provider_factory(&mut self, f: Box<dyn Fn() ->
//!         Box<dyn marsad_wasl_session::WaslSessionProvider> + Send>)`
//!      — the analogue of `set_wasl_connector_factory`. The factory constructs
//!      the provider the agent uses to `get_or_start` clients on
//!      `LaunchWaslSession`. (A zero-arg factory: the Wasl target key travels on
//!      each `LaunchWaslSession`, so unlike the per-operator connector factory
//!      the provider needs no per-call argument.)
//!   2. `Agent::register_wasl_session_provider(&mut self, provider: Box<dyn
//!         marsad_wasl_session::WaslSessionProvider>)` — the always-compiled
//!      DIRECT registration seam (the analogue of `register_wasl_connector`),
//!      used here so tests don't have to drive the `#[cfg(not(coverage))]`
//!      `serve()` path. The registered provider hands out the per-session
//!      clients that `handle_ipc`'s new `*Wasl*` arms drive.
//!   3. Two always-compiled pure mapping helpers, re-exported at the crate root
//!      exactly like X1's `wasl_event_lines` (the observe-thread broadcast is
//!      spawned only in `serve()`, which is `#[cfg(not(coverage))]` and
//!      unreachable from a unit test — so the MAPPING is proven pure, and the
//!      re-pump WIRING is proven via the synchronous `handle_ipc` arms):
//!        `wasl_session_event_lines(session_id: &str, seq: u64,
//!             ev: &marsad_wasl_session::WaslSessionEvent) -> Vec<String>`
//!        `wasl_session_status_line(session_id: &str,
//!             st: &marsad_wasl_session::WaslTargetStatus) -> String`
//!      (The design's `WaslSessionEvent { session_id, seq, event }` and
//!      `WaslSessionStatus { session_id, status }` IPC broadcasts, §7.)
//!
//! This file WILL fail to compile until the code-agent (a) adds
//! `marsad-wasl-session` as a dependency of `marsad-agent`, and (b) lands the
//! provider registry + factory seam, the six `*Wasl*` inbound `handle_ipc` arms,
//! and the two mapping helpers. That is the expected tests-first compile-failure
//! (red-state) signal.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use marsad_agent::{
    wasl_session_event_lines, wasl_session_status_line, Agent, AgentConfig, AgentError, ListenSpec,
};
use marsad_lifecycle::FakeContainer;
use marsad_notify::CaptureNotifier;
use marsad_proto::{encode_ipc, parse_ipc, IpcMsg};
use marsad_store::{MemoryBackend, Store};
use marsad_wasl_session::{
    native_gate_key, FakeWaslSessionClient, FakeWaslSessionProvider, WaslAgentCard, WaslCapabilities,
    WaslEventStream, WaslGateDecision, WaslNativeGateDetail, WaslSessionClient, WaslSessionError,
    WaslSessionEvent, WaslSessionEventKind, WaslSessionProvider, WaslStreamItem, WaslTargetKey,
    WaslTargetSnapshot, WaslTargetStatus, WaslTurnId, WaslTurnReceipt,
};

// ---------------------------------------------------------------------------
// Shared agent constructor (mirrors tests/wasl.rs's `make_agent`).
// ---------------------------------------------------------------------------

fn make_agent() -> Agent {
    Agent::with_fakes(
        AgentConfig {
            listen: ListenSpec::Stdio,
            data_root: "/tmp/marsad-agent-wasl-session-test".to_string(),
        },
        Box::new(FakeContainer::new()),
        Box::new(CaptureNotifier::new()),
        Box::new(Store::new(Box::new(MemoryBackend::new()))),
    )
    .expect("Agent::with_fakes")
}

fn all_caps() -> WaslCapabilities {
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

/// A `WaslTargetSnapshot` for `target_id`/`key` with a chosen `last_seq`. v3
/// §4.3: `get_or_start` returns the client PLUS this snapshot (NOT a `created`
/// bool) — the caller runs the §6.1 `last_seq` comparison. A fresh target's
/// journal is empty, so a freshly-spawned snapshot uses `last_seq = 0`.
fn snapshot(key: &serde_json::Value, target_id: &str, last_seq: u64) -> WaslTargetSnapshot {
    WaslTargetSnapshot {
        target_id: target_id.to_string(),
        key: WaslTargetKey(key.clone()),
        capabilities: all_caps(),
        status: WaslTargetStatus::Idle {
            since: String::new(),
        },
        last_seq,
        queued: 0,
        native_session_id: None,
    }
}

// ── IPC line builders (GUI/core → agent) ─────────────────────────────────────

fn launch_wasl_line(session_id: &str, host_id: &str, key: serde_json::Value) -> String {
    encode_ipc(&IpcMsg::LaunchWaslSession {
        session_id: session_id.to_string(),
        host_id: host_id.to_string(),
        wasl_target_key: key,
        name: None,
    })
}

fn send_wasl_prompt_line(
    session_id: &str,
    text: &str,
    idempotency_key: &str,
    busy_policy: &str,
    max_wait_ms: u64,
) -> String {
    encode_ipc(&IpcMsg::SendWaslPrompt {
        session_id: session_id.to_string(),
        text: text.to_string(),
        idempotency_key: idempotency_key.to_string(),
        busy_policy: busy_policy.to_string(),
        max_wait_ms,
    })
}

fn answer_native_gate_line(
    session_id: &str,
    gate_id: &str,
    outcome: &str,
    reason: Option<&str>,
) -> String {
    encode_ipc(&IpcMsg::AnswerWaslNativeGate {
        session_id: session_id.to_string(),
        gate_id: gate_id.to_string(),
        outcome: outcome.to_string(),
        reason: reason.map(str::to_string),
    })
}

fn interrupt_wasl_line(session_id: &str) -> String {
    encode_ipc(&IpcMsg::InterruptWaslSession {
        session_id: session_id.to_string(),
    })
}

fn stop_wasl_line(session_id: &str, drain: bool) -> String {
    encode_ipc(&IpcMsg::StopWaslSession {
        session_id: session_id.to_string(),
        drain,
    })
}

// ---------------------------------------------------------------------------
// A scriptable provider that hands out a SPECIFIC, test-owned fake so the test
// can inspect its logs (`prompts`/`resolved_gates`/`interrupts`/`shutdowns`)
// after driving the agent. `FakeWaslSessionProvider` hands out fresh internal
// fakes the test can't reach; this wrapper lets a test seed the exact client it
// wants under a given key and keep a handle to it.
// ---------------------------------------------------------------------------

/// A provider that returns pre-seeded, test-held `FakeWaslSessionClient`s keyed
/// by the canonical string form of the `WaslTargetKey`. `get_or_start` is
/// idempotent per key (later calls reattach to the SAME shared client, never a
/// second target) so the §6.1 reconnect path is provable. It returns the client
/// PLUS a `WaslTargetSnapshot` (v3 §4.3 — no `created` bool). A `Handle` retains
/// the client's shared logs.
struct SeededProvider {
    /// key-string → (client-factory-shared-state, times handed out).
    entries: Mutex<Vec<SeededEntry>>,
    /// In-memory peer store for the three new peer CRUD methods.
    peers: Mutex<BTreeMap<String, WaslAgentCard>>,
}

struct SeededEntry {
    key: String,
    target_id: String,
    caps: WaslCapabilities,
    /// The shared, test-visible client. All hand-outs clone this same Arc so
    /// the agent's registry and the test observe one set of logs.
    client: Arc<SharedClient>,
    handed_out: usize,
}

/// A `WaslSessionClient` that delegates to a shared `FakeWaslSessionClient` so
/// the test keeps a handle to the logs even after the agent takes ownership of
/// a `Box<dyn WaslSessionClient>`.
#[derive(Debug)]
struct SharedClient {
    inner: FakeWaslSessionClient,
}

impl WaslSessionClient for SharedClient {
    fn target_id(&self) -> &str {
        self.inner.target_id()
    }
    fn capabilities(&self) -> WaslCapabilities {
        self.inner.capabilities()
    }
    fn status(&self) -> WaslTargetStatus {
        self.inner.status()
    }
    fn prompt(
        &self,
        req: marsad_wasl_session::WaslTurnRequest,
        policy: marsad_wasl_session::WaslBusyPolicy,
    ) -> WaslTurnReceipt {
        self.inner.prompt(req, policy)
    }
    fn attach(&self, after_seq: u64) -> Box<dyn WaslEventStream> {
        self.inner.attach(after_seq)
    }
    fn resolve_native_gate(
        &self,
        turn_id: &WaslTurnId,
        decision: WaslGateDecision,
    ) -> Result<(), WaslSessionError> {
        self.inner.resolve_native_gate(turn_id, decision)
    }
    fn interrupt(
        &self,
        reason: marsad_wasl_session::WaslCancelReason,
    ) -> Result<(), WaslSessionError> {
        self.inner.interrupt(reason)
    }
    fn shutdown(
        &self,
        mode: marsad_wasl_session::WaslShutdownMode,
    ) -> Result<(), WaslSessionError> {
        self.inner.shutdown(mode)
    }
}

impl SeededProvider {
    fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            peers: Mutex::new(BTreeMap::new()),
        }
    }

    /// Seed a client for `key` and return a handle to its shared logs so a test
    /// can inspect what the agent drove into it.
    fn seed(&self, key: &serde_json::Value, target_id: &str) -> Arc<SharedClient> {
        let client = Arc::new(SharedClient {
            inner: FakeWaslSessionClient::new(target_id, all_caps()),
        });
        self.entries.lock().unwrap().push(SeededEntry {
            key: key_string(key),
            target_id: target_id.to_string(),
            caps: all_caps(),
            client: client.clone(),
            handed_out: 0,
        });
        client
    }

    /// How many times `get_or_start` handed out the client for `key`.
    fn handed_out(&self, key: &serde_json::Value) -> usize {
        let ks = key_string(key);
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.key == ks)
            .map(|e| e.handed_out)
            .unwrap_or(0)
    }
}

/// Canonical key string for `SeededProvider` lookup.
///
/// Strips `context.mcp_config` before serializing — `mcp_config` is a
/// Marsad-internal transport annotation injected by `handle_launch_wasl_session`
/// just before `get_or_start` (Phase D worker-send). It is NOT part of the Wasl
/// target identity that tests seed against; stripping it lets the existing test
/// seeds (which use the original key without `mcp_config`) match the mutated key
/// the agent passes in.
fn key_string(key: &serde_json::Value) -> String {
    let mut v = key.clone();
    // Strip the Marsad-internal mcp_config annotation (injected by
    // handle_launch_wasl_session just before get_or_start — Phase D
    // worker-send). After stripping, if context becomes an empty object
    // (because it only contained mcp_config, e.g. when the original key had
    // no context field at all and the agent created one just for mcp_config),
    // remove the context key entirely so the normalised form matches the
    // original seed key.
    if let Some(obj) = v.as_object_mut() {
        if let Some(ctx) = obj.get_mut("context").and_then(|c| c.as_object_mut()) {
            ctx.remove("mcp_config");
        }
        // Remove context if it is now empty.
        if obj.get("context").map(|c| c.as_object().map(|o| o.is_empty()).unwrap_or(false)).unwrap_or(false) {
            obj.remove("context");
        }
    }
    serde_json::to_string(&v).unwrap_or_default()
}

impl WaslSessionProvider for SeededProvider {
    fn get_or_start(
        &self,
        key: &WaslTargetKey,
    ) -> Result<(Box<dyn WaslSessionClient>, WaslTargetSnapshot), WaslSessionError> {
        let ks = key_string(&key.0);
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.iter_mut().find(|e| e.key == ks) {
            // Reattach to the surviving target — no second target spawned. v3:
            // return the target's snapshot (create-or-reuse ALWAYS returns one),
            // NOT a `created` bool. The fake models a reused target as still
            // fresh-journalled here (last_seq = 0); callers that need a specific
            // reconnect branch script it via the snapshot's last_seq elsewhere.
            entry.handed_out += 1;
            let _ = &entry.caps;
            let snap = snapshot(&key.0, &entry.target_id, 0);
            return Ok((Box::new(entry.client.clone()), snap));
        }
        // Unknown key → auto-seed a fresh client so a launch for an un-seeded
        // key still works (matches FakeWaslSessionProvider's "start one if none
        // exists"). A fresh target's journal is empty → snapshot.last_seq == 0.
        let target_id = format!("wasl-auto-target-{}", entries.len() + 1);
        let client = Arc::new(SharedClient {
            inner: FakeWaslSessionClient::new(&target_id, all_caps()),
        });
        entries.push(SeededEntry {
            key: ks,
            target_id: target_id.clone(),
            caps: all_caps(),
            client: client.clone(),
            handed_out: 1,
        });
        let snap = snapshot(&key.0, &target_id, 0);
        Ok((Box::new(client), snap))
    }

    fn list(&self) -> Result<Vec<String>, WaslSessionError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.target_id.clone())
            .collect())
    }

    fn register_peer(&self, card: WaslAgentCard) -> Result<(), WaslSessionError> {
        self.peers.lock().unwrap().insert(card.id.clone(), card);
        Ok(())
    }

    fn unregister_peer(&self, id: &str) -> Result<bool, WaslSessionError> {
        Ok(self.peers.lock().unwrap().remove(id).is_some())
    }

    fn list_peers(&self) -> Result<Vec<WaslAgentCard>, WaslSessionError> {
        Ok(self.peers.lock().unwrap().values().cloned().collect())
    }
}

// ===========================================================================
// Part 1 — absent = inert (§2 "load-bearing optionality", §10.3)
// ===========================================================================

/// §10.3 "absent = inert": a freshly constructed `Agent` never launched a Wasl
/// session, so an inbound `*Wasl*` COMMAND has nothing to route to and errors
/// — proving no provider/client is pre-registered (the analogue of X1's
/// `fresh_agent_has_no_wasl_connectors_registered`).
#[test]
fn fresh_agent_has_no_wasl_session_registered() {
    let mut agent = make_agent();
    let line = send_wasl_prompt_line("sess-never-launched", "hi", "idem-1", "queue", 0);
    let err = agent
        .handle_ipc(&line)
        .expect_err("SendWaslPrompt for an unknown session must error on a fresh agent");
    assert!(
        matches!(err, AgentError::Protocol(_)),
        "expected AgentError::Protocol, got {err:?}"
    );
}

/// §2/§10.3: an absent Wasl backend must not perturb the container at all — no
/// `*Wasl*` command touches the session container (mirrors X1's
/// `answer_wasl_gate_with_no_connector_has_no_container_side_effects`).
#[test]
fn wasl_session_commands_never_touch_the_session_container() {
    let mut agent = make_agent();
    let before = agent.container_calls().len();
    let _ = agent.handle_ipc(&send_wasl_prompt_line("s", "hi", "i", "queue", 0));
    let _ = agent.handle_ipc(&interrupt_wasl_line("s"));
    let _ = agent.handle_ipc(&stop_wasl_line("s", false));
    let _ = agent.handle_ipc(&answer_native_gate_line("s", "g", "approved", None));
    assert_eq!(
        agent.container_calls().len(),
        before,
        "no Wasl-managed session command may ever touch the session container"
    );
}

/// A registered provider that is never asked to launch anything performs ZERO
/// work — registering the provider alone must not spawn a target or hand out a
/// client (the analogue of X1's `registering_a_connector_alone_never_calls_decide`).
#[test]
fn registering_a_provider_alone_starts_no_target() {
    let mut agent = make_agent();
    let provider = FakeWaslSessionProvider::new();
    agent.register_wasl_session_provider(Box::new(provider));

    // An unrelated command that touches nothing Wasl-specific.
    let _ = agent.handle_ipc(&encode_ipc(&IpcMsg::Subscribe {
        session_id: String::new(),
    }));

    // Registering a provider then never launching a session leaves the agent's
    // list empty — provable via a launch of a DIFFERENT provider below; here we
    // just assert no panic / no spurious target by re-asserting inertness.
    let err = agent
        .handle_ipc(&interrupt_wasl_line("s"))
        .expect_err("no session launched yet");
    assert!(matches!(err, AgentError::Protocol(_)));
}

// ===========================================================================
// Part 2 — launch via LaunchWaslSession (§7, §10.3 "launch")
// ===========================================================================

/// A `LaunchWaslSession` registers a client obtained through the provider
/// factory seam, and `list` reflects the started target (§10.3 "launch").
#[test]
fn launch_wasl_session_registers_a_client_via_the_factory() {
    let mut agent = make_agent();
    let factory_calls = Arc::new(AtomicUsize::new(0));
    let fc = factory_calls.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        fc.fetch_add(1, Ordering::Relaxed);
        Box::new(FakeWaslSessionProvider::new()) as Box<dyn WaslSessionProvider>
    }));

    let out = agent
        .handle_ipc(&launch_wasl_line(
            "sess-1",
            "local",
            serde_json::json!({"protocol": "claude", "ctx": "a"}),
        ))
        .expect("LaunchWaslSession must succeed once a factory is set");
    // The launch must have consulted the factory to build the provider.
    assert!(
        factory_calls.load(Ordering::Relaxed) >= 1,
        "LaunchWaslSession must build its provider via the registered factory"
    );
    // Some ack line is expected (a SessionLaunched-class ack); it must at least
    // parse as a valid IpcMsg.
    for line in &out {
        parse_ipc(line).unwrap_or_else(|e| panic!("launch ack must be valid IpcMsg: {e}"));
    }
}

/// §4.3/§6.1 `get_or_start` idempotency: a DUPLICATE `LaunchWaslSession` for the
/// SAME session_id + target key reuses the existing client rather than spawning
/// a second competing target (the reconnect guarantee, provable via the seeded
/// provider's hand-out counter).
#[test]
fn duplicate_launch_reuses_the_existing_target_not_a_second_one() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"protocol": "claude", "ctx": "reuse"});
    let _handle = provider.seed(&key, "wasl-target-reuse");

    // The factory hands the SAME provider each time (Arc-shared) so its
    // per-key hand-out counter accumulates across launches.
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));

    agent
        .handle_ipc(&launch_wasl_line("sess-dup", "local", key.clone()))
        .expect("first launch");
    agent
        .handle_ipc(&launch_wasl_line("sess-dup", "local", key.clone()))
        .expect("duplicate launch must not error");

    assert!(
        provider.handed_out(&key) >= 2,
        "both launches must resolve through get_or_start for the same key"
    );
    // The second get_or_start reattached to the surviving target (create-or-
    // reuse, no `created` bool) — proven indirectly: only ONE distinct target id
    // exists for this key (a second competing target would show up as a second
    // `list` entry).
    let distinct: std::collections::HashSet<String> = provider
        .entries
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.target_id.clone())
        .collect();
    assert_eq!(
        distinct.len(),
        1,
        "a duplicate launch for the same key must NOT spawn a second competing target"
    );
}

/// Two launches with DIFFERENT session ids are independent — no cross-routing
/// (the i07-style isolation the design calls for in §10.3 "a second launch is
/// independent").
#[test]
fn two_distinct_wasl_sessions_are_independent() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key_a = serde_json::json!({"ctx": "a"});
    let key_b = serde_json::json!({"ctx": "b"});
    let ha = provider.seed(&key_a, "target-a");
    let hb = provider.seed(&key_b, "target-b");
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));

    agent
        .handle_ipc(&launch_wasl_line("sess-a", "local", key_a))
        .expect("launch a");
    agent
        .handle_ipc(&launch_wasl_line("sess-b", "local", key_b))
        .expect("launch b");

    // Prompt session A only; session B's log must stay empty.
    agent
        .handle_ipc(&send_wasl_prompt_line(
            "sess-a", "for A", "idem-a", "queue", 0,
        ))
        .expect("prompt A");
    assert_eq!(
        ha.inner.prompts.lock().unwrap().len(),
        1,
        "A got its prompt"
    );
    assert_eq!(
        hb.inner.prompts.lock().unwrap().len(),
        0,
        "B must not receive A's prompt (no cross-routing)"
    );
}

/// A provider adapter so an `Arc<SeededProvider>` satisfies the boxed-provider
/// factory return type.
struct ArcProvider(Arc<SeededProvider>);
impl WaslSessionProvider for ArcProvider {
    fn get_or_start(
        &self,
        key: &WaslTargetKey,
    ) -> Result<(Box<dyn WaslSessionClient>, WaslTargetSnapshot), WaslSessionError> {
        self.0.get_or_start(key)
    }
    fn list(&self) -> Result<Vec<String>, WaslSessionError> {
        self.0.list()
    }
    fn register_peer(&self, card: WaslAgentCard) -> Result<(), WaslSessionError> {
        self.0.register_peer(card)
    }
    fn unregister_peer(&self, id: &str) -> Result<bool, WaslSessionError> {
        self.0.unregister_peer(id)
    }
    fn list_peers(&self) -> Result<Vec<WaslAgentCard>, WaslSessionError> {
        self.0.list_peers()
    }
}

// ===========================================================================
// Part 3 — prompt round-trip (§10.3 "prompt round-trip")
// ===========================================================================

/// `SendWaslPrompt` reaches the fake's `prompts` log with the exact idempotency
/// key + text; the reply line parses as `WaslPromptReceipt` carrying the
/// scripted receipt.
#[test]
fn send_wasl_prompt_reaches_the_client_and_replies_with_the_receipt() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"ctx": "prompt"});
    let handle = provider.seed(&key, "target-prompt");
    // Script the exact receipt the fake's next `prompt` returns.
    handle.inner.script_receipt(WaslTurnReceipt::Started {
        turn_id: "turn-42".to_string(),
        first_seq: 7,
    });
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line("sess-p", "local", key))
        .expect("launch");

    let out = agent
        .handle_ipc(&send_wasl_prompt_line(
            "sess-p",
            "explain the plan",
            "11111111-1111-4111-8111-111111111111",
            "queue",
            5000,
        ))
        .expect("SendWaslPrompt must succeed");

    // The prompt reached the client verbatim.
    let prompts = handle.inner.prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1, "exactly one prompt reached the client");
    assert_eq!(prompts[0].text, "explain the plan");
    assert_eq!(
        prompts[0].idempotency_key, "11111111-1111-4111-8111-111111111111",
        "the idempotency key must be relayed VERBATIM (a UUID minted per human send)"
    );

    // The reply carries the scripted receipt as a WaslPromptReceipt.
    let receipt_line = out
        .iter()
        .find_map(|l| match parse_ipc(l) {
            Ok(IpcMsg::WaslPromptReceipt {
                session_id,
                receipt,
            }) => Some((session_id, receipt)),
            _ => None,
        })
        .expect("SendWaslPrompt must reply with a WaslPromptReceipt");
    assert_eq!(receipt_line.0, "sess-p");
    let parsed: WaslTurnReceipt =
        serde_json::from_value(receipt_line.1).expect("receipt Value must be a WaslTurnReceipt");
    assert_eq!(
        parsed,
        WaslTurnReceipt::Started {
            turn_id: "turn-42".to_string(),
            first_seq: 7,
        }
    );
}

/// A busy policy other than the default is relayed as a WaslBusyPolicy (proven
/// black-box: a scripted `RejectedBusy` receipt surfaces intact). The exact
/// policy mapping ("reject" → `WaslBusyPolicy::Reject`) is exercised by
/// including it on the wire; the fake ignores the policy value, so the
/// observable contract is "the receipt round-trips regardless of policy".
#[test]
fn send_wasl_prompt_relays_busy_policy_and_surfaces_rejected_busy_receipt() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"ctx": "busy"});
    let handle = provider.seed(&key, "target-busy");
    handle.inner.script_receipt(WaslTurnReceipt::RejectedBusy {
        active_turn: "turn-active".to_string(),
    });
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line("sess-busy", "local", key))
        .expect("launch");

    let out = agent
        .handle_ipc(&send_wasl_prompt_line(
            "sess-busy",
            "late prompt",
            "idem-b",
            "reject",
            0,
        ))
        .expect("SendWaslPrompt must succeed even when the target is busy");

    let receipt = out
        .iter()
        .find_map(|l| match parse_ipc(l) {
            Ok(IpcMsg::WaslPromptReceipt { receipt, .. }) => Some(receipt),
            _ => None,
        })
        .expect("a WaslPromptReceipt must be returned");
    let parsed: WaslTurnReceipt = serde_json::from_value(receipt).unwrap();
    assert_eq!(
        parsed,
        WaslTurnReceipt::RejectedBusy {
            active_turn: "turn-active".to_string()
        },
        "a busy rejection must surface as the receipt, not be silently dropped"
    );
}

// ===========================================================================
// Part 4 — observe broadcast: mapping helpers (§7, §10.3 "observe broadcast")
//
// The observe THREAD is spawned only in `serve()` (real-IO, coverage-excluded),
// exactly like the X1 forwarder thread. So — mirroring how X1 proves its
// broadcast via the pure `wasl_event_lines` — the mapping the observe thread
// applies is proven here as a pure function, and the re-pump WIRING is proven
// by the AttachWaslSession-driven synchronous path in Part 8.
// ===========================================================================

#[test]
fn wasl_session_event_line_maps_assistant_delta_to_typed_event() {
    let ev = WaslSessionEvent {
        seq: 3,
        kind: WaslSessionEventKind::AssistantDelta {
            turn_id: "t1".to_string(),
            text: "hello".to_string(),
        },
    };
    let lines = wasl_session_event_lines("sess-x", 3, &ev);
    assert_eq!(lines.len(), 1, "one broadcast line per event");
    match parse_ipc(&lines[0]).expect("valid IpcMsg") {
        IpcMsg::WaslSessionEvent {
            session_id,
            seq,
            event,
        } => {
            assert_eq!(session_id, "sess-x");
            assert_eq!(seq, 3, "the event's seq must be carried for replay/dedup");
            let re: WaslSessionEvent = serde_json::from_value(event)
                .expect("event Value must re-parse to the typed event");
            assert_eq!(re, ev, "the typed event must round-trip through the Value");
        }
        other => panic!("expected WaslSessionEvent, got {other:?}"),
    }
}

#[test]
fn wasl_session_event_line_round_trips_every_event_kind() {
    let kinds = vec![
        WaslSessionEventKind::SessionReady {
            native: Some("claude".to_string()),
        },
        WaslSessionEventKind::TurnAccepted {
            turn_id: "t1".to_string(),
            source: "LocalHuman".to_string(),
        },
        WaslSessionEventKind::TurnQueued {
            turn_id: "t1".to_string(),
            position: 3,
        },
        WaslSessionEventKind::ToolStarted {
            turn_id: "t1".to_string(),
            name: "Bash".to_string(),
            input: serde_json::json!({ "command": "ls" }),
        },
        WaslSessionEventKind::ToolFinished {
            turn_id: "t1".to_string(),
            name: "Bash".to_string(),
            ok: true,
            result: Some(serde_json::json!("ok")),
        },
        WaslSessionEventKind::NativeGateRequested {
            turn_id: "t1".to_string(),
            detail: WaslNativeGateDetail {
                summary: "run rm -rf".to_string(),
                preview: "$ rm -rf /tmp/x".to_string(),
            },
        },
        WaslSessionEventKind::NativeGateResolved {
            turn_id: "t1".to_string(),
            decision: "approved".to_string(),
        },
        WaslSessionEventKind::TurnCompleted {
            turn_id: "t1".to_string(),
            reply: "done".to_string(),
            certainty: "committed".to_string(),
        },
        WaslSessionEventKind::TurnFailed {
            turn_id: "t1".to_string(),
            error: "boom".to_string(),
            certainty: "unknown".to_string(),
        },
        WaslSessionEventKind::TurnCancelled {
            turn_id: "t1".to_string(),
            reason: "human".to_string(),
        },
        WaslSessionEventKind::TurnExpired {
            turn_id: "t1".to_string(),
        },
        WaslSessionEventKind::StatusChanged {
            status: WaslTargetStatus::Idle {
                since: "t".to_string(),
            },
        },
        WaslSessionEventKind::SessionExited { code: Some(1) },
        WaslSessionEventKind::ObserveLost {
            error: "reset".to_string(),
        },
        WaslSessionEventKind::Unknown("SomeFutureEventKind".to_string()),
    ];
    for (i, kind) in kinds.into_iter().enumerate() {
        let seq = (i as u64) + 1;
        let ev = WaslSessionEvent { seq, kind };
        let lines = wasl_session_event_lines("sess-y", seq, &ev);
        match parse_ipc(&lines[0]).unwrap() {
            IpcMsg::WaslSessionEvent { event, seq: s, .. } => {
                assert_eq!(s, seq);
                let re: WaslSessionEvent = serde_json::from_value(event).unwrap();
                assert_eq!(re, ev, "every event kind must round-trip");
            }
            other => panic!("expected WaslSessionEvent, got {other:?}"),
        }
    }
}

#[test]
fn wasl_session_status_line_maps_each_status_to_typed_broadcast() {
    let statuses = vec![
        WaslTargetStatus::Offline,
        WaslTargetStatus::Starting,
        WaslTargetStatus::Idle {
            since: "t".to_string(),
        },
        WaslTargetStatus::Busy {
            active_turn: "t1".to_string(),
            source: "LocalHuman".to_string(),
            phase: "generating".to_string(),
            since: "t".to_string(),
            queued: 0,
        },
        WaslTargetStatus::Faulted {
            error: "crash".to_string(),
            recoverable: false,
        },
        WaslTargetStatus::Stopped,
    ];
    for st in statuses {
        let line = wasl_session_status_line("sess-z", &st);
        match parse_ipc(&line).expect("valid IpcMsg") {
            IpcMsg::WaslSessionStatus { session_id, status } => {
                assert_eq!(session_id, "sess-z");
                let re: WaslTargetStatus = serde_json::from_value(status)
                    .expect("status Value must re-parse to the typed status");
                assert_eq!(re, st, "the typed status must round-trip through the Value");
            }
            other => panic!("expected WaslSessionStatus, got {other:?}"),
        }
    }
}

// ===========================================================================
// Part 5 — native gate relay (§8, §10.3 "native gate relay")
// ===========================================================================

/// `AnswerWaslNativeGate` (approve) reaches the fake's `resolved_gates` log
/// keyed by the blocked turn's `turn_id` (v3: `resolve_native_gate` takes a
/// `WaslTurnId`, NOT a gate id — the real gate has none). The wire still carries
/// the per-occurrence `gate_key` in the `gate_id` IPC field; the agent decomposes
/// it back to the bare `turn_id` before calling the client. The reply parses as
/// `WaslNativeGateAnswered` echoing the same `gate_id` (gate_key).
#[test]
fn answer_native_gate_approve_reaches_client_and_acks() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"ctx": "gate"});
    let handle = provider.seed(&key, "target-gate");
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line("sess-g", "local", key))
        .expect("launch");
    // Put the fake in AwaitingGate so resolve advances the state machine. v3: the
    // detail is only {summary, preview}; the fake's blocked turn is "t-gate".
    handle.inner.script_native_gate(WaslNativeGateDetail {
        summary: "run tests".to_string(),
        preview: "$ cargo test".to_string(),
    });

    // The wire gate_id is the per-occurrence (turn_id, seq) key; the agent must
    // resolve the bare turn_id ("t-gate") out of it before calling the client.
    let gate_key = native_gate_key("t-gate", 1);
    let out = agent
        .handle_ipc(&answer_native_gate_line(
            "sess-g", &gate_key, "approved", None,
        ))
        .expect("AnswerWaslNativeGate must succeed");

    let resolved = handle.inner.resolved_gates.lock().unwrap();
    assert_eq!(
        resolved.len(),
        1,
        "exactly one gate resolution reached the client"
    );
    assert_eq!(
        resolved[0].0, "t-gate",
        "resolve_native_gate must be keyed by the bare turn_id decomposed from the \
         (turn_id, seq) gate_key — the real gate has no gate id (v3)"
    );
    assert_eq!(
        resolved[0].1,
        WaslGateDecision::Approve,
        "an 'approved' outcome must relay as WaslGateDecision::Approve"
    );

    assert_eq!(
        out.iter()
            .filter_map(|l| parse_ipc(l).ok())
            .find(|m| matches!(m, IpcMsg::WaslNativeGateAnswered { .. })),
        Some(IpcMsg::WaslNativeGateAnswered {
            session_id: "sess-g".to_string(),
            gate_id: gate_key.clone(),
        })
    );
}

/// `AnswerWaslNativeGate` (deny + reason) relays `Deny { reason }`.
#[test]
fn answer_native_gate_deny_relays_reason() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"ctx": "gate-deny"});
    let handle = provider.seed(&key, "target-gate-deny");
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line("sess-gd", "local", key))
        .expect("launch");
    handle.inner.script_native_gate(WaslNativeGateDetail {
        summary: "danger".to_string(),
        preview: "$ rm -rf /".to_string(),
    });

    let gate_key = native_gate_key("t-gate", 1);
    agent
        .handle_ipc(&answer_native_gate_line(
            "sess-gd",
            &gate_key,
            "denied",
            Some("too risky"),
        ))
        .expect("deny must succeed");

    let resolved = handle.inner.resolved_gates.lock().unwrap();
    assert_eq!(
        resolved[0].0, "t-gate",
        "the bare turn_id must be relayed (v3), decomposed from the gate_key"
    );
    assert_eq!(
        resolved[0].1,
        WaslGateDecision::Deny {
            reason: "too risky".to_string()
        },
        "a 'denied' outcome + reason must relay as WaslGateDecision::Deny{{reason}}"
    );
}

/// §10.3: a scripted client error on `resolve_native_gate` surfaces as a
/// `Protocol` agent error (the serve loop would reply `Error`), AND — critically
/// — must not poison the provider/registry lock (the X1
/// `wasl_lock_safety.rs` analogue: a failing resolve can't wedge the registry).
#[test]
fn answer_native_gate_client_error_surfaces_as_protocol_error() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"ctx": "gate-err"});
    let handle = provider.seed(&key, "target-gate-err");
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line("sess-ge", "local", key))
        .expect("launch");
    handle.inner.script_native_gate(WaslNativeGateDetail {
        summary: "s".to_string(),
        preview: "p".to_string(),
    });
    handle
        .inner
        .script_error(WaslSessionError::Protocol("resolve failed".to_string()));

    let gate_key = native_gate_key("t-gate", 1);
    let err = agent
        .handle_ipc(&answer_native_gate_line(
            "sess-ge", &gate_key, "approved", None,
        ))
        .expect_err("a scripted resolve error must surface as an agent error");
    assert!(
        matches!(err, AgentError::Protocol(_)),
        "expected AgentError::Protocol, got {err:?}"
    );

    // The registry must still be usable after the error: a prompt on the same
    // (or another) session must still route (a poisoned lock would error/panic).
    let out = agent
        .handle_ipc(&send_wasl_prompt_line(
            "sess-ge",
            "still works",
            "i2",
            "queue",
            0,
        ))
        .expect("the registry must remain usable after a resolve error");
    assert!(out
        .iter()
        .any(|l| matches!(parse_ipc(l), Ok(IpcMsg::WaslPromptReceipt { .. }))));
}

// ===========================================================================
// Part 6 — LOCK HYGIENE (§8.1, §10.3 numbered requirement)
//
// The `Agent` is deliberately NOT shareable across threads (see serve.rs's
// "Concurrency model" doc comment), so — exactly as the X1
// `wasl_lock_safety.rs` does — this is proven WITHOUT threads via Mutex
// POISONING: if a client method PANICS while the provider/registry lock is
// held, that lock is left poisoned and every later registry operation panics
// on `.lock().unwrap()`. If the lock is correctly dropped BEFORE the client
// call ("clone the Arc out, drop the lock, then call"), a panic inside the
// client cannot poison it and the registry stays fully usable — the ONLY way
// this test can pass.
// ===========================================================================

/// A client whose every blocking method PANICS — stands in for "the blocking
/// Wasl call blew up while a lock was (wrongly) held". A real slow/blocking peer
/// would time out rather than panic, but a panic is the sharpest black-box probe
/// for whether a `Mutex` was held across the call.
#[derive(Debug)]
struct PanickingClient;
impl WaslSessionClient for PanickingClient {
    fn target_id(&self) -> &str {
        "panicking-target"
    }
    fn capabilities(&self) -> WaslCapabilities {
        all_caps()
    }
    fn status(&self) -> WaslTargetStatus {
        WaslTargetStatus::Idle {
            since: String::new(),
        }
    }
    fn prompt(
        &self,
        _req: marsad_wasl_session::WaslTurnRequest,
        _policy: marsad_wasl_session::WaslBusyPolicy,
    ) -> WaslTurnReceipt {
        panic!("simulated catastrophic prompt() failure");
    }
    fn attach(&self, _after_seq: u64) -> Box<dyn WaslEventStream> {
        panic!("simulated catastrophic attach() failure");
    }
    fn resolve_native_gate(
        &self,
        _turn_id: &WaslTurnId,
        _decision: WaslGateDecision,
    ) -> Result<(), WaslSessionError> {
        panic!("simulated catastrophic resolve_native_gate() failure");
    }
    fn interrupt(
        &self,
        _reason: marsad_wasl_session::WaslCancelReason,
    ) -> Result<(), WaslSessionError> {
        panic!("simulated catastrophic interrupt() failure");
    }
    fn shutdown(
        &self,
        _mode: marsad_wasl_session::WaslShutdownMode,
    ) -> Result<(), WaslSessionError> {
        panic!("simulated catastrophic shutdown() failure");
    }
}

/// A provider that hands out a `PanickingClient` for key "wedge" and a healthy
/// `FakeWaslSessionClient` for every other key. Its OWN internal lock (if any)
/// is what the test proves stays unpoisoned.
struct MixedProvider {
    started: Mutex<Vec<String>>,
    peers: Mutex<BTreeMap<String, WaslAgentCard>>,
}
impl MixedProvider {
    fn new() -> Self {
        Self {
            started: Mutex::new(Vec::new()),
            peers: Mutex::new(BTreeMap::new()),
        }
    }
}
impl WaslSessionProvider for MixedProvider {
    fn get_or_start(
        &self,
        key: &WaslTargetKey,
    ) -> Result<(Box<dyn WaslSessionClient>, WaslTargetSnapshot), WaslSessionError> {
        let ks = key_string(&key.0);
        self.started.lock().unwrap().push(ks.clone());
        if ks.contains("wedge") {
            Ok((
                Box::new(PanickingClient),
                snapshot(&key.0, "panicking-target", 0),
            ))
        } else {
            Ok((
                Box::new(FakeWaslSessionClient::new("healthy", all_caps())),
                snapshot(&key.0, "healthy", 0),
            ))
        }
    }
    fn list(&self) -> Result<Vec<String>, WaslSessionError> {
        Ok(self.started.lock().unwrap().clone())
    }
    fn register_peer(&self, card: WaslAgentCard) -> Result<(), WaslSessionError> {
        self.peers.lock().unwrap().insert(card.id.clone(), card);
        Ok(())
    }
    fn unregister_peer(&self, id: &str) -> Result<bool, WaslSessionError> {
        Ok(self.peers.lock().unwrap().remove(id).is_some())
    }
    fn list_peers(&self) -> Result<Vec<WaslAgentCard>, WaslSessionError> {
        Ok(self.peers.lock().unwrap().values().cloned().collect())
    }
}

/// §8.1: a blocking (here: panicking) client call on session A must NOT be made
/// while the agent holds the provider-registry / session-client-map lock. Proven
/// by Mutex poisoning: after A's `prompt()` panics, an unrelated operation on a
/// DIFFERENT session B must still complete. A held-across-the-call lock would be
/// left poisoned by the unwind and B's registry access would itself panic.
#[test]
fn prompt_blocking_call_on_one_session_does_not_wedge_the_registry_for_another() {
    let mut agent = make_agent();
    let provider = Arc::new(MixedProvider::new());
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcMixed(p.clone())) as Box<dyn WaslSessionProvider>
    }));

    agent
        .handle_ipc(&launch_wasl_line(
            "sess-wedge",
            "local",
            serde_json::json!({"ctx": "wedge"}),
        ))
        .expect("launch wedged session");
    agent
        .handle_ipc(&launch_wasl_line(
            "sess-ok",
            "local",
            serde_json::json!({"ctx": "healthy"}),
        ))
        .expect("launch healthy session");

    // A's prompt panics inside the client.
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        agent.handle_ipc(&send_wasl_prompt_line(
            "sess-wedge",
            "boom",
            "i",
            "queue",
            0,
        ))
    }));
    assert!(
        panicked.is_err(),
        "sanity: the panicking client must propagate out of handle_ipc"
    );

    // B's prompt must still succeed — a poisoned registry lock here would prove
    // the agent held it across A's client call.
    let out = agent
        .handle_ipc(&send_wasl_prompt_line("sess-ok", "hi", "i2", "queue", 0))
        .unwrap_or_else(|e| {
            panic!(
                "the session registry must stay usable after a client panic on an \
                 unrelated session — a poisoned lock here proves a registry/map lock was \
                 held across the blocking client call. Got: {e}"
            )
        });
    assert!(
        out.iter()
            .any(|l| matches!(parse_ipc(l), Ok(IpcMsg::WaslPromptReceipt { .. }))),
        "the healthy session's prompt must round-trip normally"
    );
}

/// The same lock-hygiene guarantee for `resolve_native_gate` (§8.1 lists it
/// among the blocking methods that must never be called under a held lock).
#[test]
fn resolve_native_gate_blocking_call_does_not_wedge_the_registry() {
    let mut agent = make_agent();
    let provider = Arc::new(MixedProvider::new());
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcMixed(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line(
            "sess-wedge2",
            "local",
            serde_json::json!({"ctx": "wedge"}),
        ))
        .expect("launch wedged");
    agent
        .handle_ipc(&launch_wasl_line(
            "sess-ok2",
            "local",
            serde_json::json!({"ctx": "healthy"}),
        ))
        .expect("launch healthy");

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        agent.handle_ipc(&answer_native_gate_line(
            "sess-wedge2",
            "g",
            "approved",
            None,
        ))
    }));
    assert!(panicked.is_err(), "sanity: resolve panic must propagate");

    let out = agent
        .handle_ipc(&send_wasl_prompt_line("sess-ok2", "hi", "i", "queue", 0))
        .unwrap_or_else(|e| {
            panic!("registry must survive a resolve_native_gate panic on another session: {e}")
        });
    assert!(out
        .iter()
        .any(|l| matches!(parse_ipc(l), Ok(IpcMsg::WaslPromptReceipt { .. }))));
}

/// The same guarantee for `interrupt` and `shutdown` (both listed in §8.1),
/// checked together for economy — each must not be held under the lock.
#[test]
fn interrupt_and_shutdown_blocking_calls_do_not_wedge_the_registry() {
    for (verb_line, ctx) in [
        (interrupt_wasl_line("sess-wedge3"), "interrupt"),
        (stop_wasl_line("sess-wedge3", false), "shutdown"),
    ] {
        let mut agent = make_agent();
        let provider = Arc::new(MixedProvider::new());
        let p = provider.clone();
        agent.set_wasl_session_provider_factory(Box::new(move || {
            Box::new(ArcMixed(p.clone())) as Box<dyn WaslSessionProvider>
        }));
        agent
            .handle_ipc(&launch_wasl_line(
                "sess-wedge3",
                "local",
                serde_json::json!({"ctx": "wedge"}),
            ))
            .expect("launch wedged");
        agent
            .handle_ipc(&launch_wasl_line(
                "sess-ok3",
                "local",
                serde_json::json!({"ctx": "healthy"}),
            ))
            .expect("launch healthy");

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            agent.handle_ipc(&verb_line)
        }));
        assert!(panicked.is_err(), "sanity: {ctx} panic must propagate");

        let out = agent
            .handle_ipc(&send_wasl_prompt_line("sess-ok3", "hi", "i", "queue", 0))
            .unwrap_or_else(|e| {
                panic!("registry must survive a {ctx} panic on another session: {e}")
            });
        assert!(out
            .iter()
            .any(|l| matches!(parse_ipc(l), Ok(IpcMsg::WaslPromptReceipt { .. }))));
    }
}

/// Adapter so an `Arc<MixedProvider>` satisfies the boxed-provider factory type.
struct ArcMixed(Arc<MixedProvider>);
impl WaslSessionProvider for ArcMixed {
    fn get_or_start(
        &self,
        key: &WaslTargetKey,
    ) -> Result<(Box<dyn WaslSessionClient>, WaslTargetSnapshot), WaslSessionError> {
        self.0.get_or_start(key)
    }
    fn list(&self) -> Result<Vec<String>, WaslSessionError> {
        self.0.list()
    }
    fn register_peer(&self, card: WaslAgentCard) -> Result<(), WaslSessionError> {
        self.0.register_peer(card)
    }
    fn unregister_peer(&self, id: &str) -> Result<bool, WaslSessionError> {
        self.0.unregister_peer(id)
    }
    fn list_peers(&self) -> Result<Vec<WaslAgentCard>, WaslSessionError> {
        self.0.list_peers()
    }
}

// ===========================================================================
// Part 7 — interrupt / stop (§10.3 "interrupt / stop")
// ===========================================================================

#[test]
fn interrupt_wasl_session_reaches_the_client() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"ctx": "interrupt"});
    let handle = provider.seed(&key, "target-int");
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line("sess-i", "local", key))
        .expect("launch");

    agent
        .handle_ipc(&interrupt_wasl_line("sess-i"))
        .expect("InterruptWaslSession must succeed");
    assert_eq!(
        handle.inner.interrupts.lock().unwrap().len(),
        1,
        "the interrupt must reach the client's interrupts log"
    );
}

#[test]
fn stop_wasl_session_reaches_the_client_with_drain_mode() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"ctx": "stop"});
    let handle = provider.seed(&key, "target-stop");
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line("sess-s", "local", key))
        .expect("launch");

    agent
        .handle_ipc(&stop_wasl_line("sess-s", true))
        .expect("StopWaslSession(drain) must succeed");
    let shutdowns = handle.inner.shutdowns.lock().unwrap();
    assert_eq!(shutdowns.len(), 1, "the shutdown must reach the client");
    assert_eq!(
        shutdowns[0],
        marsad_wasl_session::WaslShutdownMode::Drain,
        "drain:true must map to WaslShutdownMode::Drain"
    );
}

#[test]
fn stop_wasl_session_immediate_when_not_draining() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"ctx": "stop-imm"});
    let handle = provider.seed(&key, "target-stop-imm");
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line("sess-si", "local", key))
        .expect("launch");

    agent
        .handle_ipc(&stop_wasl_line("sess-si", false))
        .expect("StopWaslSession(immediate) must succeed");
    assert_eq!(
        handle.inner.shutdowns.lock().unwrap()[0],
        marsad_wasl_session::WaslShutdownMode::Immediate,
        "drain:false must map to WaslShutdownMode::Immediate"
    );
}

// ===========================================================================
// Part 8 — crash reflected + reconnect re-pump (§10.3 "crash reflected",
// "reconnect after core restart")
// ===========================================================================

/// A scripted crash (`SessionExited` + `Faulted` status) maps to a
/// `WaslSessionStatus` broadcast line that core will project to Error/Detached.
/// Proven via the pure status-mapping helper (the observe thread's mapping).
#[test]
fn crash_maps_to_a_faulted_status_line() {
    let st = WaslTargetStatus::Faulted {
        error: "session crashed".to_string(),
        recoverable: false,
    };
    let line = wasl_session_status_line("sess-crash", &st);
    match parse_ipc(&line).unwrap() {
        IpcMsg::WaslSessionStatus { session_id, status } => {
            assert_eq!(session_id, "sess-crash");
            let re: WaslTargetStatus = serde_json::from_value(status).unwrap();
            assert_eq!(re, st);
        }
        other => panic!("expected WaslSessionStatus, got {other:?}"),
    }
    // A SessionExited event maps to a terminal event line too.
    let ev = WaslSessionEvent {
        seq: 9,
        kind: WaslSessionEventKind::SessionExited { code: Some(137) },
    };
    let ev_lines = wasl_session_event_lines("sess-crash", 9, &ev);
    assert!(matches!(
        parse_ipc(&ev_lines[0]),
        Ok(IpcMsg::WaslSessionEvent { .. })
    ));
}

/// §10.3 reconnect: an `AttachWaslSession { after_seq }` re-pumps the observe
/// stream from the last known sequence. Proven via the fake's replay contract:
/// events past `after_seq` are the only ones the re-attached stream yields, with
/// no gap and no dup. (This exercises `WaslSessionClient::attach`'s `after_seq`
/// semantics that the agent's observe thread relies on.)
#[test]
fn attach_after_seq_replays_only_newer_events_no_gap_no_dup() {
    let client = FakeWaslSessionClient::new("target-replay", all_caps());
    for i in 1..=5 {
        client.script_event(WaslSessionEventKind::AssistantDelta {
            turn_id: "t1".to_string(),
            text: format!("chunk-{i}"),
        });
    }
    // Re-attach from seq 2 → must yield exactly seqs 3,4,5, in order, once each.
    // v3: `next()` yields a `WaslStreamItem` — normal events come through
    // `Event(..)`, a `Truncated` is a distinct control item.
    let mut stream = client.attach(2);
    let mut seen = Vec::new();
    while let Some(item) = stream.next() {
        match item {
            WaslStreamItem::Event(ev) => seen.push(ev.seq),
            WaslStreamItem::Truncated { .. } => {
                panic!("no truncation was scripted; expected only Event items")
            }
        }
    }
    assert_eq!(
        seen,
        vec![3, 4, 5],
        "attach(after_seq=2) must replay exactly the newer events, no gap, no dup"
    );
}

/// §6.1 reconnect idempotency at the provider level: a "core restart" (drop the
/// client-side handle, then re-`get_or_start` the SAME key) reattaches to the
/// surviving target — SAME target_id, never a second one — and returns a
/// `WaslTargetSnapshot` (v3 §4.3: create-or-reuse ALWAYS returns a snapshot;
/// there is NO `created` bool).
#[test]
fn get_or_start_reconnect_returns_same_target_snapshot_and_no_second_target() {
    let provider = FakeWaslSessionProvider::new();
    let key = WaslTargetKey(serde_json::json!({"protocol": "claude", "ctx": "survivor"}));

    let (c1, snap1) = provider.get_or_start(&key).expect("first start");
    let target1 = c1.target_id().to_string();
    assert_eq!(
        snap1.last_seq, 0,
        "a fresh target's journal is empty → snapshot.last_seq == 0"
    );

    // Simulate a core restart: the client handle is dropped; the surviving
    // provider is asked again for the same key.
    drop(c1);
    let (c2, snap2) = provider.get_or_start(&key).expect("reattach");
    assert_eq!(
        c2.target_id(),
        target1,
        "reattaching must bind to the SAME surviving target (no second one)"
    );
    assert_eq!(
        snap2.target_id, target1,
        "the reattach snapshot names the same surviving target"
    );
    assert_eq!(
        provider.list().unwrap().len(),
        1,
        "a reconnect must NOT spawn a second competing target for the same key"
    );
}

/// §5's `script_pre_existing(key, target_id, last_seq)` seam: a reconnect can be
/// tested in isolation (no prior `get_or_start` in the same test) — the first
/// `get_or_start` after seeding reattaches to the surviving target and its
/// snapshot surfaces the seeded `last_seq` (what the caller compares in §6.1),
/// not a `created` bool.
#[test]
fn script_pre_existing_target_reattaches_on_first_get_or_start() {
    let provider = FakeWaslSessionProvider::new();
    let key = WaslTargetKey(serde_json::json!({"ctx": "pre-existing"}));
    provider.script_pre_existing(&key, "surviving-target", 7);

    let (client, snapshot) = provider
        .get_or_start(&key)
        .expect("reattach to seeded target");
    assert_eq!(
        client.target_id(),
        "surviving-target",
        "a pre-existing (surviving) target must reattach, not be re-created"
    );
    assert_eq!(
        snapshot.last_seq, 7,
        "the reattach snapshot surfaces the seeded surviving target's last_seq (7)"
    );
}

// ===========================================================================
// Part 9 — v3 spec-correction coverage: stream `Truncated` handling at the
// observe pump, `WaslCausation` on the built `WaslTurnRequest`, and the
// `WaslTurnId`-keyed native-gate relay naming. NEW tests for the v3 contract.
//
// The observe THREAD is spawned only in `serve()` (real-IO, coverage-excluded),
// so — as Part 4 does for the event/status mapping — these prove the CONTRACT
// the pump relies on (the `WaslEventStream` yielding `Truncated` distinctly, and
// the re-snapshot+resubscribe replay semantics) at the synchronous client level.
// ===========================================================================

/// v3 §4.4/§10.1: a `WaslStreamItem::Truncated{earliest, latest}` is surfaced by
/// `attach`'s stream DISTINCTLY from any normal `Event` — never silently dropped,
/// never folded into an `Unknown` event. The observe pump reacts to it by
/// re-fetching a snapshot and resubscribing from `latest`; here we prove the
/// stream hands the pump the unmissable control item so it CAN react.
#[test]
fn observe_stream_surfaces_truncated_distinctly_from_events() {
    let client = FakeWaslSessionClient::new("target-trunc", all_caps());
    // The replay cursor fell below the retained floor: earliest=100, latest=140.
    client.script_truncated(100, 140);

    // Subscribing from a cursor below `earliest` yields the Truncated control
    // item (the cursor fell out of the retained window).
    let mut stream = client.attach(0);
    let first = stream
        .next()
        .expect("a truncation must be surfaced, not dropped");
    match first {
        WaslStreamItem::Truncated { earliest, latest } => {
            assert_eq!(earliest, 100);
            assert_eq!(
                latest, 140,
                "the pump must resubscribe from `latest` after a truncation"
            );
        }
        WaslStreamItem::Event(ev) => panic!(
            "a truncation must be a distinct WaslStreamItem::Truncated, never an \
             Event (would be silently mis-folded); got Event {ev:?}"
        ),
    }
}

/// v3 §4.4: after the pump reacts to a `Truncated` by resubscribing from
/// `latest`, the stale truncation is NOT replayed — proving the "re-snapshot +
/// resubscribe from `latest`" recovery converges (no infinite re-truncation
/// loop). A resubscribe at/above `earliest` sees a clean end-of-stream.
#[test]
fn resubscribe_from_latest_after_truncation_does_not_replay_the_truncation() {
    let client = FakeWaslSessionClient::new("target-trunc2", all_caps());
    client.script_truncated(100, 140);

    // The pump resubscribes from `latest` (140), i.e. at/above `earliest` (100):
    // the stale truncation must not surface again.
    let mut resumed = client.attach(140);
    assert!(
        resumed.next().is_none(),
        "after resubscribing from `latest`, the stale truncation must NOT replay — \
         otherwise the pump would re-snapshot forever"
    );
}

/// v3 §7: a `SendWaslPrompt` the agent turns into a `WaslTurnRequest` carries a
/// FRESH single-hop `WaslCausation` (`parent_request: None`, empty `path`,
/// `hop: 0`, a fresh `root_request`) — the agent mints it (it is NOT on the wire,
/// since Marsad only ever originates root LocalHuman turns). Two distinct sends
/// mint two distinct `root_request`s.
#[test]
fn send_wasl_prompt_builds_a_fresh_single_hop_causation() {
    let mut agent = make_agent();
    let provider = Arc::new(SeededProvider::new());
    let key = serde_json::json!({"ctx": "causation"});
    let handle = provider.seed(&key, "target-causation");
    let p = provider.clone();
    agent.set_wasl_session_provider_factory(Box::new(move || {
        Box::new(ArcProvider(p.clone())) as Box<dyn WaslSessionProvider>
    }));
    agent
        .handle_ipc(&launch_wasl_line("sess-c", "local", key))
        .expect("launch");

    agent
        .handle_ipc(&send_wasl_prompt_line(
            "sess-c", "first", "idem-1", "queue", 0,
        ))
        .expect("first prompt");
    agent
        .handle_ipc(&send_wasl_prompt_line(
            "sess-c", "second", "idem-2", "queue", 0,
        ))
        .expect("second prompt");

    let prompts = handle.inner.prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2, "both prompts must reach the client");
    for req in prompts.iter() {
        assert_eq!(
            req.causation.parent_request, None,
            "Marsad only originates ROOT turns — parent_request must be None"
        );
        assert!(
            req.causation.path.is_empty(),
            "a fresh single-hop causation has an empty path"
        );
        assert_eq!(req.causation.hop, 0, "a root turn is hop 0");
        assert!(
            !req.causation.root_request.is_empty(),
            "each send must carry a fresh, non-empty root_request"
        );
    }
    assert_ne!(
        prompts[0].causation.root_request, prompts[1].causation.root_request,
        "two distinct human sends must mint two distinct root_request ids"
    );
}

// ===========================================================================
// Part 10 — reconnect fresh-vs-resume decision from the snapshot's last_seq
// (§4.3/§6.1, v3 spec-correction cascade).
//
// v3 removed the fictional `created: bool` from `get_or_start`: real Wasl has NO
// create-vs-reuse wire signal (`POST /v1/targets` is create-or-reuse and ALWAYS
// returns a `TargetSnapshot`). Fresh-vs-resume is decided by comparing the
// returned `snapshot.last_seq` against Marsad's persisted `binding.last_seq`
// (the `after_seq` core requested on `AttachWaslSession`):
//   • snapshot.last_seq >= persisted → SAME still-alive session → resume from
//     the persisted seq (attach(after_seq = persisted));
//   • snapshot.last_seq  < persisted → the target was REPLACED/restarted (a
//     fresh target's journal starts over below what Marsad has on file) → reset
//     local seq tracking to 0 and re-attach(0).
//
// The observe THREAD that runs the pump is `serve()`-only (coverage-excluded,
// exactly like `reader.rs`'s thread), so — as Part 4 does for the mapping and
// `wasl_session_observe.rs` does for the pump/reconnect POLICY — this proves the
// load-bearing DECISION as a pure, synchronous helper. It computes the
// `after_seq` the pump must use given the snapshot the agent got back from
// `get_or_start` and the seq core persisted. The whole target payload lives in
// the returned `WaslTargetSnapshot`, so the helper takes the two seqs the
// comparison is over.
//
// INVENTED / ASSUMED SURFACE (test-first): a pure helper re-exported at the
// crate root next to `wasl_session_event_lines` / `wasl_observe_should_reconnect`:
//
//     pub fn wasl_reconnect_after_seq(snapshot_last_seq: u64, persisted_last_seq: u64) -> u64
//
// returning the `after_seq` the observe pump must (re)attach from. If the
// code-agent lands a differently-shaped seam it should adjust these call sites;
// the pinned BEHAVIOR (the two §6.1 branches) is the contract.
// ===========================================================================

/// §6.1 branch (a) RESUME: when the returned `snapshot.last_seq` is >= the
/// persisted seq, the target is the same, still-alive session — the pump resumes
/// from the persisted seq (no gap, no dup via Wasl's `events(after_seq)` replay).
#[test]
fn reconnect_after_seq_resumes_from_persisted_when_snapshot_ge_persisted() {
    // snapshot ahead of us (9) and exactly at us (5) both resume from persisted.
    assert_eq!(
        marsad_agent::wasl_reconnect_after_seq(9, 5),
        5,
        "snapshot.last_seq (9) >= persisted (5) → resume from the persisted seq (5)"
    );
    assert_eq!(
        marsad_agent::wasl_reconnect_after_seq(5, 5),
        5,
        "snapshot.last_seq == persisted → still the same session → resume from persisted"
    );
}

/// §6.1 branch (b) REPLACED/RESET: when the returned `snapshot.last_seq` is LESS
/// than the persisted seq, the real target was replaced/restarted (its journal
/// started over below what Marsad had folded) — Marsad treats it as fresh and
/// resets local seq tracking to 0, re-attaching from 0.
#[test]
fn reconnect_after_seq_resets_to_zero_when_snapshot_lt_persisted() {
    assert_eq!(
        marsad_agent::wasl_reconnect_after_seq(2, 40),
        0,
        "snapshot.last_seq (2) < persisted (40) → target was replaced → reset to 0"
    );
    // A brand-new target (snapshot.last_seq == 0) with ANY prior persisted
    // progress is a replaced/fresh target → reset to 0.
    assert_eq!(
        marsad_agent::wasl_reconnect_after_seq(0, 12),
        0,
        "a fresh target (snapshot.last_seq == 0) below our persisted seq resets to 0"
    );
}

/// The boundary/first-connect case: nothing persisted yet (persisted == 0) and a
/// fresh target (snapshot == 0) attaches from 0 — resume and reset coincide, no
/// spurious reset, no negative/underflow.
#[test]
fn reconnect_after_seq_first_connect_is_zero() {
    assert_eq!(
        marsad_agent::wasl_reconnect_after_seq(0, 0),
        0,
        "first connect (nothing persisted, fresh target) attaches from 0"
    );
}
