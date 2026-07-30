//! X6 — Wasl **Backend** (Wasl-managed session) agent wiring: the
//! `WaslSessionProvider`/`WaslSessionClient` registry + factory test seams, the
//! six inbound `*Wasl*` `handle_ipc` arms, and the two pure observe-thread
//! mapping helpers (`wasl_session_event_lines`/`wasl_session_status_line`).
//!
//! Authoritative design: `docs/design/wasl-backend-plan.md` (v2) §6.1, §7, §8,
//! §8.1. STRUCTURALLY IDENTICAL to the X1 operator wiring in `wasl.rs`: the
//! registry mirrors `wasl_connectors`, the factory/registration seams mirror
//! `set_wasl_connector_factory`/`register_wasl_connector`, and — critically —
//! every arm that calls a potentially-blocking `WaslSessionClient` method
//! follows `handle_answer_wasl_gate`'s lock-hygiene pattern EXACTLY: take the
//! map lock, clone the `Arc<dyn WaslSessionClient>` out, DROP the lock, THEN
//! call. No registry/map lock is ever held across a blocking client call (§8.1,
//! a numbered test requirement).

use marsad_proto::{encode_ipc, IpcMsg};
use marsad_wasl_session::{
    new_idempotency_key, WaslBusyPolicy, WaslCancelReason, WaslCausation, WaslGateDecision,
    WaslSessionClient, WaslSessionEvent, WaslSessionEventKind, WaslShutdownMode, WaslStreamItem,
    WaslTargetKey, WaslTargetStatus, WaslTurnId, WaslTurnRequest,
};

use super::{Agent, AgentError};

// ---------------------------------------------------------------------------
// §7 — the two pure observe-thread mapping helpers (crate-root re-exported)
// ---------------------------------------------------------------------------

/// Pure mapping (unit-testable without sockets / observe threads): one
/// normalized `WaslSessionEvent` -> the `WaslSessionEvent` IPC broadcast line
/// the observe thread emits (§7). Value-carried on purpose (the typed form lives
/// in `marsad-wasl-session`; `marsad-proto` must not depend on it), so core
/// re-parses the `event` Value back to the typed event when folding.
///
/// The analogue of X1's `wasl_event_lines`: the observe THREAD is spawned only
/// in `serve()` (real-IO, `#[cfg(not(coverage))]`), so this pure mapping is what
/// is proven, and the re-pump wiring is proven via the synchronous `handle_ipc`
/// arms.
pub fn wasl_session_event_lines(session_id: &str, seq: u64, ev: &WaslSessionEvent) -> Vec<String> {
    vec![encode_ipc(&IpcMsg::WaslSessionEvent {
        session_id: session_id.to_string(),
        seq,
        event: serde_json::to_value(ev).expect("WaslSessionEvent always serializes"),
    })]
}

/// Pure mapping: a Wasl advisory status -> its `WaslSessionStatus` broadcast line
/// (§7). Value-carried like the event mapping above; core projects the status to
/// `Phase` (§6.3).
pub fn wasl_session_status_line(session_id: &str, st: &WaslTargetStatus) -> String {
    encode_ipc(&IpcMsg::WaslSessionStatus {
        session_id: session_id.to_string(),
        status: serde_json::to_value(st).expect("WaslTargetStatus always serializes"),
    })
}

// ---------------------------------------------------------------------------
// §6.1 — the observe-thread pump (pure, always-compiled, unit-tested) + the
// reconnect-policy / reconnect-after_seq decisions it leans on.
//
// The spawned observe THREAD + real subscriber broadcast list + backoff sleep
// live in the `#[cfg(not(coverage))]` `serve()` layer (exactly like
// `reader.rs`'s `spawn_session_reader`); the LOAD-BEARING pump logic is
// extracted here as a pure, synchronous helper proven against the in-repo
// `FakeWaslSessionClient` (the analogue of `reconnect_backoff_next` /
// `conn_on_drop`). No socket, no real process — the fake IS the substrate.
// ---------------------------------------------------------------------------

/// The verdict `pump_wasl_observe` returns when its stream ends: the resume
/// cursor (highest emitted seq) and whether a TERMINAL event (`SessionExited`)
/// was seen before the end (§6.1). Drives `wasl_observe_should_reconnect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaslObserveEnd {
    /// Highest event seq emitted — the resume cursor for a reconnect, and what a
    /// `WaslSessionBinding.last_seq` would persist (§6.1).
    pub last_seq: u64,
    /// True iff a TERMINAL event (`SessionExited`) was observed before the stream
    /// ended — the session is genuinely gone.
    pub terminal: bool,
}

/// Pump ONE observe subscription end to end (§6.1): call `client.attach(after_seq)`,
/// loop its `next()`, and for each item:
///
/// * `WaslStreamItem::Event(ev)` → emit every line `wasl_session_event_lines`
///   produces (so a launched session's reply / transcript / native-gate / status
///   change reaches core), advance the high-water seq to `ev.seq`, and note a
///   TERMINAL if the event is a `SessionExited`.
/// * `WaslStreamItem::Truncated{latest, ..}` → NOT emitted as an event line;
///   instead RE-ATTACH from `latest` (discarding the old stream) and keep
///   pumping — a retention-window truncation is transparently recovered (§4.4).
///   A stale truncation is never re-surfaced (the fake/real stream only yields a
///   truncation whose window is below the current cursor), so the recovery
///   converges.
/// * `None` (clean end-of-stream) → return the high-water seq + terminal verdict.
///
/// Pure + synchronous: the caller (the `serve()` observe thread) drives it in a
/// loop, broadcasting each `emit`ted line to subscribers and reconnecting per
/// `wasl_observe_should_reconnect` when it returns non-terminal.
pub fn pump_wasl_observe(
    session_id: &str,
    client: &dyn WaslSessionClient,
    after_seq: u64,
    emit: &mut dyn FnMut(String),
) -> WaslObserveEnd {
    let mut last_seq = after_seq;
    let mut terminal = false;
    let mut stream = client.attach(after_seq);
    loop {
        match stream.next() {
            Some(WaslStreamItem::Event(ev)) => {
                let seq = ev.seq;
                if matches!(ev.kind, WaslSessionEventKind::SessionExited { .. }) {
                    terminal = true;
                }
                for line in wasl_session_event_lines(session_id, seq, &ev) {
                    emit(line);
                }
                last_seq = seq;
            }
            // A truncation is a control signal, not an event: re-attach from
            // `latest` and keep delivering the events that live above it.
            Some(WaslStreamItem::Truncated { latest, .. }) => {
                stream = client.attach(latest);
            }
            None => {
                return WaslObserveEnd { last_seq, terminal };
            }
        }
    }
}

/// The reconnect-on-disconnect policy (§6.1): reconnect ⇔ the pump ended
/// NON-terminally (a bare transport disconnect while the Wasl session may still
/// be alive → re-`attach(last_seq)` to resume with no gap/dup). A stream that
/// ended AFTER a terminal `SessionExited` means the session is truly gone —
/// Marsad does not own the process, so reconnecting would attach to nothing.
/// The Wasl analogue of `reader.rs::conn_on_probe`'s "Dead never reconnects".
pub fn wasl_observe_should_reconnect(end: &WaslObserveEnd) -> bool {
    !end.terminal
}

/// Decide the `after_seq` the observe pump must (re)attach from on a §6.1
/// reconnect, given the `snapshot.last_seq` `get_or_start` returned and the seq
/// Marsad persisted for this binding. There is NO `created` flag in real Wasl:
///
/// * `snapshot_last_seq >= persisted` → the SAME still-alive session → resume
///   from `persisted` (Wasl's `events(after_seq)` replay is gap/dup-free);
/// * `snapshot_last_seq  < persisted` → the target was REPLACED/restarted (its
///   journal started over below what Marsad had folded) → treat it as fresh:
///   reset local seq tracking to `0` and re-attach from `0`.
pub fn wasl_reconnect_after_seq(snapshot_last_seq: u64, persisted_last_seq: u64) -> u64 {
    if snapshot_last_seq >= persisted_last_seq {
        persisted_last_seq
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// §6.1/§7 — provider registry + factory test seams (mirror wasl.rs's X1 seams)
// ---------------------------------------------------------------------------

impl Agent {
    /// Additive test seam (also the direct registration seam used by `serve()`'s
    /// real-IO wiring): register the `WaslSessionProvider` the agent uses to
    /// `get_or_start` clients on `LaunchWaslSession`. The analogue of
    /// `register_wasl_connector`. Registering a provider does NOT start any target
    /// or hand out a client — that only happens on a `LaunchWaslSession`.
    pub fn register_wasl_session_provider(
        &mut self,
        provider: Box<dyn marsad_wasl_session::WaslSessionProvider>,
    ) {
        self.wasl_session_provider.0 = Some(provider);
    }

    /// Additive test seam: set the zero-arg factory the agent uses to build its
    /// `WaslSessionProvider` on first launch. The direct analogue of
    /// `set_wasl_connector_factory`. Setter so no constructor changes. Setting a
    /// factory alone starts nothing — the provider is built lazily on the first
    /// `LaunchWaslSession` that needs it.
    pub fn set_wasl_session_provider_factory(&mut self, f: super::WaslSessionProviderFactoryFn) {
        self.wasl_session_provider_factory.0 = Some(f);
    }

    /// Obtain the session's registered client handle (an `Arc` clone), dropping
    /// the map lock before returning it — so callers NEVER hold the map lock
    /// across a blocking client call (§8.1). Absent session -> `Protocol` error
    /// (the whole no-session-launched surface: a fresh agent has an empty map).
    fn wasl_session_client(
        &self,
        session_id: &str,
    ) -> Result<std::sync::Arc<dyn WaslSessionClient>, AgentError> {
        let clients = self.wasl_session_clients.0.lock().unwrap();
        clients
            .get(session_id)
            .cloned()
            .ok_or_else(|| AgentError::Protocol(format!("no wasl-managed session '{session_id}'")))
    }

    /// Obtain an INDEPENDENTLY-OWNED client `Box` (which is `Send`, unlike the
    /// registry's `Arc<dyn WaslSessionClient>`) for the per-session observe thread,
    /// plus the `after_seq` that thread's first `pump_wasl_observe` must attach
    /// from. Create-or-reuse: `get_or_start` reattaches to the SAME target the
    /// preceding `LaunchWaslSession` registered (never a second competing target),
    /// so the observe thread pumps the same live session the synchronous IPC arms
    /// drive. The start seq is the §6.1 decision over the returned
    /// `snapshot.last_seq` and the seq core persisted for this binding.
    ///
    /// `serve()`-only (real IO): the observe thread it feeds is spawned from
    /// `serve()`, which is `#[cfg(not(coverage))]`. The pump/reconnect LOGIC it
    /// runs (`pump_wasl_observe` / `wasl_observe_should_reconnect` /
    /// `wasl_reconnect_after_seq`) is the pure, always-compiled, unit-tested core.
    #[cfg(not(coverage))]
    pub(crate) fn wasl_observe_client_and_after_seq(
        &self,
        wasl_target_key: serde_json::Value,
        persisted_last_seq: u64,
    ) -> Result<(Box<dyn WaslSessionClient>, u64), AgentError> {
        let key = WaslTargetKey(wasl_target_key);
        let (client, snapshot) = self
            .wasl_session_provider
            .0
            .as_ref()
            .ok_or_else(|| {
                AgentError::Protocol("no wasl session provider for observe".to_string())
            })?
            .get_or_start(&key)
            .map_err(|e| AgentError::Protocol(format!("observe get_or_start failed: {e}")))?;
        let after_seq = wasl_reconnect_after_seq(snapshot.last_seq, persisted_last_seq);
        Ok((client, after_seq))
    }

    // -----------------------------------------------------------------------
    // §7 — the six inbound `*Wasl*` handle_ipc arms
    // -----------------------------------------------------------------------

    /// `LaunchWaslSession`: build the provider (via the registered one, or lazily
    /// via the factory), `get_or_start` a client for `wasl_target_key`, and
    /// register it under `session_id`. No argv, no container — the process is
    /// Wasl's. Idempotent per (session, key): a duplicate launch reuses the
    /// existing target rather than spawning a second competing one (the reconnect
    /// guarantee, §4.3/§6.1) — `get_or_start` is create-or-reuse (it returns a
    /// `WaslTargetSnapshot`, never a `created` bool), and the map is keyed by
    /// `session_id` so no second entry appears. The per-session observe THREAD
    /// that pumps `attach(after_seq)` and broadcasts the mapped event/status
    /// lines is spawned by `serve()`'s real-IO layer (which owns the subscriber
    /// broadcast list); it decides its start seq via
    /// [`crate::wasl_reconnect_after_seq`]`(snapshot.last_seq, persisted)` and
    /// loops [`crate::pump_wasl_observe`] with [`crate::wasl_observe_should_reconnect`].
    pub(crate) fn handle_launch_wasl_session(
        &mut self,
        session_id: String,
        host_id: String,
        wasl_target_key: serde_json::Value,
        name: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        let _ = (host_id, name);
        // Ensure a provider exists: prefer the directly-registered one, else build
        // one from the factory (once) and keep it. Absent both -> Protocol error.
        if self.wasl_session_provider.0.is_none() {
            match self.wasl_session_provider_factory.0.as_ref() {
                Some(factory) => self.wasl_session_provider.0 = Some(factory()),
                None => {
                    return Err(AgentError::Protocol(format!(
                        "LaunchWaslSession {session_id}: no wasl session provider registered"
                    )))
                }
            }
        }

        // Resolve (or reattach to) the client. `get_or_start` is the potentially
        // blocking call, but the provider is `&mut self`-borrowed here (not under
        // any registry lock), so lock hygiene holds trivially.
        let key = WaslTargetKey(wasl_target_key);
        let (client, _snapshot) = self
            .wasl_session_provider
            .0
            .as_ref()
            .expect("provider present after the guard above")
            .get_or_start(&key)
            .map_err(|e| AgentError::Protocol(format!("get_or_start failed: {e}")))?;

        // Box<dyn WaslSessionClient> -> Arc<dyn WaslSessionClient> at the registry
        // boundary (mirrors `register_wasl_connector`'s `Arc::from`) so later arms
        // can clone the handle out and drop the map lock before the blocking call.
        let client: std::sync::Arc<dyn WaslSessionClient> = std::sync::Arc::from(client);
        self.wasl_session_clients
            .0
            .lock()
            .unwrap()
            .insert(session_id.clone(), client);

        Ok(vec![encode_ipc(&IpcMsg::SessionLaunched { session_id })])
    }

    /// `SendWaslPrompt`: build a `WaslTurnRequest` + `WaslBusyPolicy` and relay it
    /// to the session's client via `prompt`; reply with the `TurnReceipt` as a
    /// `WaslPromptReceipt` (the atomic admission decision; the reply itself
    /// arrives later as a `WaslSessionEvent::TurnCompleted`). Lock hygiene: the
    /// client `Arc` is cloned out and the map lock dropped BEFORE `prompt`.
    pub(crate) fn handle_send_wasl_prompt(
        &mut self,
        session_id: String,
        text: String,
        idempotency_key: String,
        busy_policy: String,
        max_wait_ms: u64,
    ) -> Result<Vec<String>, AgentError> {
        let client = self.wasl_session_client(&session_id)?;
        // The agent mints the fresh single-hop `WaslCausation` (§7): it is NOT
        // carried on the IPC, since Marsad ALWAYS originates a fresh ROOT
        // `LocalHuman` turn (no chaining, no parent, empty path, hop 0). A fresh
        // `root_request` per send — distinct across sends.
        let req = WaslTurnRequest {
            idempotency_key,
            text,
            causation: WaslCausation::fresh(new_idempotency_key()),
        };
        let policy = busy_policy_from_wire(&busy_policy, max_wait_ms);
        let receipt = client.prompt(req, policy);
        Ok(vec![encode_ipc(&IpcMsg::WaslPromptReceipt {
            session_id,
            receipt: serde_json::to_value(&receipt).expect("WaslTurnReceipt always serializes"),
        })])
    }

    /// `AttachWaslSession`: (re)subscribe the observe stream from `after_seq`.
    /// The per-session observe THREAD is spawned in `serve()` (real IO); here the
    /// synchronous path validates the session exists (so the reconnect re-pump
    /// wiring is exercised) and acks — the observe events themselves broadcast
    /// through `wasl_session_event_lines` on the thread. Lock hygiene: the client
    /// `Arc` is cloned out and the map lock dropped before touching it.
    pub(crate) fn handle_attach_wasl_session(
        &mut self,
        session_id: String,
        after_seq: u64,
    ) -> Result<Vec<String>, AgentError> {
        let _client = self.wasl_session_client(&session_id)?;
        // The real observe re-pump (spawning the per-session observe thread that
        // pumps `attach(after_seq)` and broadcasts `wasl_session_event_lines`)
        // lives in serve.rs, which owns the subscriber broadcast list. Here the
        // ack is the observable signal that the (re)attach was accepted and routed
        // to the right session's client; the fake's replay-from-`after_seq`
        // correctness is proven directly against the client in the test suite.
        let _ = after_seq;
        Ok(vec![encode_ipc(&IpcMsg::SessionAttached { session_id })])
    }

    /// `AnswerWaslNativeGate`: relay the human's decision to the session's client
    /// via `resolve_native_gate`, then ack with `WaslNativeGateAnswered`
    /// REGARDLESS of the client ack (the follow-on status/event is authoritative,
    /// mirroring X1's `WaslGateAnswered` semantics). A scripted client error
    /// surfaces as a `Protocol` agent error. Lock hygiene (§8.1): the client
    /// `Arc` is cloned out and the map lock DROPPED before `resolve_native_gate`
    /// — a wedged/panicking client cannot poison the registry lock.
    pub(crate) fn handle_answer_wasl_native_gate(
        &mut self,
        session_id: String,
        gate_id: String,
        outcome: String,
        reason: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        let client = self.wasl_session_client(&session_id)?;
        let decision = gate_decision_from_wire(&outcome, reason);
        // v3: the wire `gate_id` field carries core's per-occurrence
        // `native_gate_key(turn_id, seq)` (format `"{turn_id}#{seq}"`), Marsad's
        // ledger identity. Wasl's `resolve_native_gate` is keyed by the bare
        // `turn_id` (the real gate has no gate id), so decompose the key back to
        // the turn_id before calling the client — split on the LAST `#` so a
        // turn_id that itself contains a `#` is preserved.
        let turn_id = turn_id_from_gate_key(&gate_id);
        client
            .resolve_native_gate(&WaslTurnId(turn_id), decision)
            .map_err(|e| {
                AgentError::Protocol(format!(
                    "resolve_native_gate failed for {session_id}/{gate_id}: {e}"
                ))
            })?;
        Ok(vec![encode_ipc(&IpcMsg::WaslNativeGateAnswered {
            session_id,
            gate_id,
        })])
    }

    /// `InterruptWaslSession`: best-effort `cancel_turn` via the client. Lock
    /// hygiene: client `Arc` cloned out, map lock dropped before `interrupt`.
    pub(crate) fn handle_interrupt_wasl_session(
        &mut self,
        session_id: String,
    ) -> Result<Vec<String>, AgentError> {
        let client = self.wasl_session_client(&session_id)?;
        client
            .interrupt(WaslCancelReason::Human)
            .map_err(|e| AgentError::Protocol(format!("interrupt failed for {session_id}: {e}")))?;
        Ok(vec![encode_ipc(&IpcMsg::Interrupted { session_id })])
    }

    /// `StopWaslSession`: `shutdown(Drain|Immediate)` via the client and drop the
    /// registry entry. Lock hygiene: client `Arc` cloned out, map lock dropped
    /// before `shutdown`.
    pub(crate) fn handle_stop_wasl_session(
        &mut self,
        session_id: String,
        drain: bool,
    ) -> Result<Vec<String>, AgentError> {
        let client = self.wasl_session_client(&session_id)?;
        let mode = if drain {
            WaslShutdownMode::Drain
        } else {
            WaslShutdownMode::Immediate
        };
        client
            .shutdown(mode)
            .map_err(|e| AgentError::Protocol(format!("shutdown failed for {session_id}: {e}")))?;
        self.wasl_session_clients
            .0
            .lock()
            .unwrap()
            .remove(&session_id);
        Ok(vec![encode_ipc(&IpcMsg::SessionStopped { session_id })])
    }
}

/// Map the wire busy-policy string (+ `max_wait_ms`) to a typed `WaslBusyPolicy`
/// (§7). "reject" -> `Reject`; "queue_related" -> `QueueRelated`; anything else
/// (incl. the default "queue" and empty) -> `Queue` (§12 Q4: a human's late
/// prompt is usually still wanted).
fn busy_policy_from_wire(policy: &str, max_wait_ms: u64) -> WaslBusyPolicy {
    match policy {
        "reject" => WaslBusyPolicy::Reject,
        "queue_related" => WaslBusyPolicy::QueueRelated { max_wait_ms },
        _ => WaslBusyPolicy::Queue { max_wait_ms },
    }
}

/// Decompose core's per-occurrence gate key `native_gate_key(turn_id, seq)`
/// (format `"{turn_id}#{seq}"`, `marsad-wasl-session`'s projection module) back
/// into the bare Wasl `turn_id` that `resolve_native_gate` is keyed by (§8, v3).
/// Splits on the LAST `#` so a turn_id containing a `#` survives; a key with no
/// `#` (e.g. a raw turn_id, or an unrecognized shape) is returned verbatim.
fn turn_id_from_gate_key(gate_key: &str) -> String {
    match gate_key.rsplit_once('#') {
        Some((turn_id, _seq)) => turn_id.to_string(),
        None => gate_key.to_string(),
    }
}

/// Map the wire outcome string (+ optional reason) to a typed `WaslGateDecision`
/// (§7/§8). "denied" -> `Deny { reason }`; anything else (incl. "approved") ->
/// `Approve`.
fn gate_decision_from_wire(outcome: &str, reason: Option<String>) -> WaslGateDecision {
    if outcome == "denied" {
        WaslGateDecision::Deny {
            reason: reason.unwrap_or_default(),
        }
    } else {
        WaslGateDecision::Approve
    }
}
