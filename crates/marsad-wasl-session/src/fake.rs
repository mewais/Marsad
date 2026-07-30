//! `FakeWaslSessionClient` / `FakeWaslSessionProvider` — the in-memory test
//! double the whole phase runs against (`wasl-backend-plan.md` §5). Mirrors
//! `marsad_wasl::FakeWaslConnector`: scriptable, inspectable, zero IO, no real
//! process anywhere. A scripted state machine — transitions and events are
//! advanced by explicit `script_*` calls, not wall-clock — so tests are
//! deterministic with no timing flakiness.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::client::{WaslEventStream, WaslSessionClient, WaslSessionProvider};
use crate::error::WaslSessionError;
use crate::types::{
    WaslBusyPolicy, WaslCancelReason, WaslCapabilities, WaslEventSeq, WaslGateDecision,
    WaslNativeGateDetail, WaslSessionEvent, WaslSessionEventKind, WaslShutdownMode, WaslStreamItem,
    WaslTargetKey, WaslTargetSnapshot, WaslTargetStatus, WaslTurnId, WaslTurnReceipt,
    WaslTurnRequest,
};

/// One recorded gate resolution: `(turn_id, decision)` — v3: resolution is keyed
/// by the blocked turn's `turn_id` (the real gate has NO gate id).
type ResolvedGate = (String, WaslGateDecision);

/// The mutable, scripted heart of a [`FakeWaslSessionClient`], shared so a
/// cloned handle (e.g. from a provider) observes the same state.
#[derive(Debug)]
struct FakeState {
    status: WaslTargetStatus,
    /// Stream items already assigned (events carry a seq; a `Truncated` is a
    /// control item), in order. `attach` snapshots this.
    items: Vec<WaslStreamItem>,
    /// The next seq to assign (monotonic across ALL enqueue paths).
    next_seq: WaslEventSeq,
    /// Receipts to return from `prompt`, FIFO; default `Started` when drained.
    receipts: VecDeque<WaslTurnReceipt>,
    /// Errors to return from the next recording call, FIFO.
    errors: VecDeque<WaslSessionError>,
}

/// In-memory scriptable Wasl-managed session. Drives the full
/// [`WaslSessionClient`] contract with NO process and NO IO. Inspectable:
/// `prompts` / `resolved_gates` / `interrupts` / `shutdowns` are pub logs
/// (mirrors `FakeWaslConnector.decisions`).
#[derive(Debug)]
pub struct FakeWaslSessionClient {
    target_id: String,
    caps: WaslCapabilities,
    /// Every `prompt` request, verbatim, in call order.
    pub prompts: Arc<Mutex<Vec<WaslTurnRequest>>>,
    /// Every `resolve_native_gate` call as `(turn_id, decision)`.
    pub resolved_gates: Arc<Mutex<Vec<ResolvedGate>>>,
    /// Every `interrupt` reason.
    pub interrupts: Arc<Mutex<Vec<WaslCancelReason>>>,
    /// Every `shutdown` mode.
    pub shutdowns: Arc<Mutex<Vec<WaslShutdownMode>>>,
    state: Arc<Mutex<FakeState>>,
}

impl FakeWaslSessionClient {
    /// Build a fresh fake for `target_id` with the given honest capability set.
    /// Starts `Idle` (advisory) with an empty event stream.
    pub fn new(target_id: &str, caps: WaslCapabilities) -> Self {
        Self {
            target_id: target_id.to_string(),
            caps,
            prompts: Arc::new(Mutex::new(Vec::new())),
            resolved_gates: Arc::new(Mutex::new(Vec::new())),
            interrupts: Arc::new(Mutex::new(Vec::new())),
            shutdowns: Arc::new(Mutex::new(Vec::new())),
            state: Arc::new(Mutex::new(FakeState {
                status: WaslTargetStatus::Idle {
                    since: String::new(),
                },
                items: Vec::new(),
                next_seq: 0,
                receipts: VecDeque::new(),
                errors: VecDeque::new(),
            })),
        }
    }

    // ── scripting knobs (what tests drive) ──────────────────────────────────

    /// Set the status `status()` returns next.
    pub fn script_status(&self, status: WaslTargetStatus) {
        self.state.lock().unwrap().status = status;
    }

    /// Queue the receipt the NEXT `prompt` returns, FIFO; default `Started` when
    /// exhausted.
    pub fn script_receipt(&self, receipt: WaslTurnReceipt) {
        self.state.lock().unwrap().receipts.push_back(receipt);
    }

    /// Enqueue one event onto the `attach` stream (seq assigned monotonically),
    /// wrapped as a [`WaslStreamItem::Event`].
    pub fn script_event(&self, kind: WaslSessionEventKind) {
        let mut st = self.state.lock().unwrap();
        push_event(&mut st, kind);
    }

    /// Enqueue a [`WaslStreamItem::Truncated`]`{earliest, latest}` onto the
    /// `attach` stream (v3 §4.4), so a test proves the caller re-snapshots +
    /// resubscribes from `latest` distinctly from any normal event. A truncation
    /// is a control item, NOT an event — it does not consume a seq.
    pub fn script_truncated(&self, earliest: WaslEventSeq, latest: WaslEventSeq) {
        self.state
            .lock()
            .unwrap()
            .items
            .push(WaslStreamItem::Truncated { earliest, latest });
    }

    /// Convenience: script a full accept → one delta → complete happy turn so the
    /// common case is one call.
    pub fn script_happy_turn(&self, turn_id: &str, reply: &str) {
        let mut st = self.state.lock().unwrap();
        push_event(
            &mut st,
            WaslSessionEventKind::TurnAccepted {
                turn_id: turn_id.to_string(),
                source: "LocalHuman".to_string(),
            },
        );
        push_event(
            &mut st,
            WaslSessionEventKind::AssistantDelta {
                turn_id: turn_id.to_string(),
                text: reply.to_string(),
            },
        );
        push_event(
            &mut st,
            WaslSessionEventKind::TurnCompleted {
                turn_id: turn_id.to_string(),
                reply: reply.to_string(),
                certainty: "committed".to_string(),
            },
        );
    }

    /// Convenience: drive a native gate — sets status `AwaitingGate` AND enqueues
    /// `NativeGateRequested`, so both the status projection and the gate-pipeline
    /// path (§8) fire. v3: the detail is only `{summary, preview}`; per-gate
    /// identity is `(turn_id, seq)`.
    pub fn script_native_gate(&self, detail: WaslNativeGateDetail) {
        let mut st = self.state.lock().unwrap();
        st.status = WaslTargetStatus::AwaitingGate {
            active_turn: "t-gate".to_string(),
            detail: detail.clone(),
            since: String::new(),
            queued: 0,
        };
        push_event(
            &mut st,
            WaslSessionEventKind::NativeGateRequested {
                turn_id: "t-gate".to_string(),
                detail,
            },
        );
    }

    /// Simulate a crash: status → `Faulted{recoverable:false}` AND enqueue a
    /// terminal `SessionExited`.
    pub fn script_crash(&self, code: Option<i32>) {
        let mut st = self.state.lock().unwrap();
        st.status = WaslTargetStatus::Faulted {
            error: "session crashed".to_string(),
            recoverable: false,
        };
        push_event(&mut st, WaslSessionEventKind::SessionExited { code });
    }

    /// Error the next `prompt`/`resolve_native_gate`/`interrupt`/`shutdown`
    /// (scripted `Err`), FIFO. The attempt is still recorded before the error
    /// surfaces.
    pub fn script_error(&self, err: WaslSessionError) {
        self.state.lock().unwrap().errors.push_back(err);
    }
}

/// Assign the next monotonic seq and append the event (as a
/// [`WaslStreamItem::Event`]) to the scripted stream.
fn push_event(st: &mut FakeState, kind: WaslSessionEventKind) {
    st.next_seq += 1;
    let seq = st.next_seq;
    st.items
        .push(WaslStreamItem::Event(WaslSessionEvent { seq, kind }));
}

impl WaslSessionClient for FakeWaslSessionClient {
    fn target_id(&self) -> &str {
        &self.target_id
    }

    fn capabilities(&self) -> WaslCapabilities {
        self.caps.clone()
    }

    fn status(&self) -> WaslTargetStatus {
        self.state.lock().unwrap().status.clone()
    }

    fn prompt(&self, req: WaslTurnRequest, _busy_policy: WaslBusyPolicy) -> WaslTurnReceipt {
        self.prompts.lock().unwrap().push(req);
        let mut st = self.state.lock().unwrap();
        st.receipts.pop_front().unwrap_or(WaslTurnReceipt::Started {
            turn_id: "t-fake".to_string(),
            first_seq: st.next_seq + 1,
        })
    }

    fn attach(&self, after_seq: WaslEventSeq) -> Box<dyn WaslEventStream> {
        let st = self.state.lock().unwrap();
        // Replay honors `after_seq` for events (skip earlier seqs, no gaps/dupes).
        // A `Truncated` control item surfaces only when the requested cursor is
        // below the retained floor (`after_seq < earliest`) — i.e. the cursor
        // fell out of the window. Once a caller resubscribes from `latest` (or
        // any point at/above `earliest`), that stale truncation is not replayed.
        let pending: VecDeque<WaslStreamItem> = st
            .items
            .iter()
            .filter(|item| match item {
                WaslStreamItem::Event(ev) => ev.seq > after_seq,
                WaslStreamItem::Truncated { earliest, .. } => after_seq < *earliest,
            })
            .cloned()
            .collect();
        Box::new(FakeWaslEventStream { pending })
    }

    fn resolve_native_gate(
        &self,
        turn_id: &WaslTurnId,
        decision: WaslGateDecision,
    ) -> Result<(), WaslSessionError> {
        // Record the attempt even when the ack is a scripted error. v3: keyed by
        // the blocked turn's `turn_id` (the real gate has no gate id).
        self.resolved_gates
            .lock()
            .unwrap()
            .push((turn_id.0.clone(), decision));
        let mut st = self.state.lock().unwrap();
        if let Some(err) = st.errors.pop_front() {
            return Err(err);
        }
        // Default success behavior: advance AwaitingGate → Busy, enqueue a
        // NativeGateResolved then a follow-on TurnCompleted, so a test proves the
        // full "gate blocks → resolved → completes" loop with no real Wasl actor.
        if matches!(st.status, WaslTargetStatus::AwaitingGate { .. }) {
            st.status = WaslTargetStatus::Busy {
                active_turn: "t-gate".to_string(),
                source: "LocalHuman".to_string(),
                phase: "generating".to_string(),
                since: String::new(),
                queued: 0,
            };
            push_event(
                &mut st,
                WaslSessionEventKind::NativeGateResolved {
                    turn_id: "t-gate".to_string(),
                    decision: "approved".to_string(),
                },
            );
            push_event(
                &mut st,
                WaslSessionEventKind::TurnCompleted {
                    turn_id: "t-gate".to_string(),
                    reply: "resolved".to_string(),
                    certainty: "committed".to_string(),
                },
            );
        }
        Ok(())
    }

    fn interrupt(&self, reason: WaslCancelReason) -> Result<(), WaslSessionError> {
        self.interrupts.lock().unwrap().push(reason);
        if let Some(err) = self.state.lock().unwrap().errors.pop_front() {
            return Err(err);
        }
        Ok(())
    }

    fn shutdown(&self, mode: WaslShutdownMode) -> Result<(), WaslSessionError> {
        self.shutdowns.lock().unwrap().push(mode);
        if let Some(err) = self.state.lock().unwrap().errors.pop_front() {
            return Err(err);
        }
        Ok(())
    }
}

/// The stream `attach` hands out: drains a snapshot of the scripted stream items
/// past `after_seq`, in order, then `None` (clean end-of-stream).
struct FakeWaslEventStream {
    pending: VecDeque<WaslStreamItem>,
}

impl WaslEventStream for FakeWaslEventStream {
    fn next(&mut self) -> Option<WaslStreamItem> {
        self.pending.pop_front()
    }
}

/// One handed-out fake plus the metadata the provider needs to reattach to the
/// SAME underlying target on a later `get_or_start` for the same key.
struct ProviderEntry {
    target_id: String,
    key: WaslTargetKey,
    caps: WaslCapabilities,
    /// The `last_seq` this target's snapshot reports. A genuinely-fresh target's
    /// journal is empty (`0`); [`FakeWaslSessionProvider::script_pre_existing`]
    /// seeds a surviving target with a CHOSEN `last_seq` so both §6.1 reconnect
    /// branches (resume when `>= persisted`, reset when `< persisted`) are
    /// testable in isolation — the fake surfaces this value verbatim, not a
    /// boolean `created` flag (real Wasl has none).
    last_seq: WaslEventSeq,
    state: Arc<Mutex<FakeState>>,
    prompts: Arc<Mutex<Vec<WaslTurnRequest>>>,
    resolved_gates: Arc<Mutex<Vec<ResolvedGate>>>,
    interrupts: Arc<Mutex<Vec<WaslCancelReason>>>,
    shutdowns: Arc<Mutex<Vec<WaslShutdownMode>>>,
}

impl ProviderEntry {
    /// Build a client bound to this entry's shared state — so a reattach observes
    /// the same target, logs, and event stream as the original hand-out.
    fn client(&self) -> FakeWaslSessionClient {
        FakeWaslSessionClient {
            target_id: self.target_id.clone(),
            caps: self.caps.clone(),
            prompts: self.prompts.clone(),
            resolved_gates: self.resolved_gates.clone(),
            interrupts: self.interrupts.clone(),
            shutdowns: self.shutdowns.clone(),
            state: self.state.clone(),
        }
    }

    /// The caller-side [`WaslTargetSnapshot`] this entry surfaces from
    /// `get_or_start` (§4.3): the target id, its key, capabilities, the current
    /// advisory status, and the `last_seq` the §6.1 comparison runs against.
    fn snapshot(&self) -> WaslTargetSnapshot {
        WaslTargetSnapshot {
            target_id: self.target_id.clone(),
            key: self.key.clone(),
            capabilities: self.caps.clone(),
            status: self.state.lock().unwrap().status.clone(),
            last_seq: self.last_seq,
            queued: 0,
            native_session_id: None,
        }
    }
}

/// In-memory scriptable provider that hands out [`FakeWaslSessionClient`]s
/// (§5). `get_or_start` is idempotent per key: the first call for a key spawns a
/// target (`created = true`); a later call for the SAME key reattaches to it
/// (`created = false`, no second target). Fully-capable clients are handed out.
pub struct FakeWaslSessionProvider {
    entries: Mutex<HashMap<String, ProviderEntry>>,
    next_target: Mutex<u64>,
}

impl FakeWaslSessionProvider {
    #[allow(clippy::new_without_default)] // test fake: no test constructs it via `Default`.
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            next_target: Mutex::new(0),
        }
    }

    /// Seed a surviving target for `key` under `target_id` with a chosen
    /// `last_seq` WITHOUT a prior `get_or_start` (§5), so a §6.1 reconnect can be
    /// tested in isolation: the next `get_or_start(key)` reattaches to it and its
    /// returned [`WaslTargetSnapshot`] surfaces this `last_seq` (v3 — no `created`
    /// bool). A `last_seq >= persisted` drives the resume branch; a
    /// `last_seq < persisted` drives the replaced/reset branch.
    pub fn script_pre_existing(&self, key: &WaslTargetKey, target_id: &str, last_seq: WaslEventSeq) {
        let mut entries = self.entries.lock().unwrap();
        entries.insert(key_string(key), new_entry(target_id, key.clone(), last_seq));
    }
}

/// The canonical string form of a target key, used as the provider map key.
/// Serialized so structurally-equal `Value`s (the tests build them by hand) map
/// to the same entry.
fn key_string(key: &WaslTargetKey) -> String {
    serde_json::to_string(&key.0).unwrap_or_default()
}

/// A fresh provider entry (its own shared state + logs) for `target_id`, handed
/// out fully-capable with the given `key` + snapshot `last_seq`.
fn new_entry(target_id: &str, key: WaslTargetKey, last_seq: WaslEventSeq) -> ProviderEntry {
    ProviderEntry {
        target_id: target_id.to_string(),
        key,
        caps: all_caps(),
        last_seq,
        state: Arc::new(Mutex::new(FakeState {
            status: WaslTargetStatus::Idle {
                since: String::new(),
            },
            items: Vec::new(),
            next_seq: 0,
            receipts: VecDeque::new(),
            errors: VecDeque::new(),
        })),
        prompts: Arc::new(Mutex::new(Vec::new())),
        resolved_gates: Arc::new(Mutex::new(Vec::new())),
        interrupts: Arc::new(Mutex::new(Vec::new())),
        shutdowns: Arc::new(Mutex::new(Vec::new())),
    }
}

/// A fully-capable set (every `RuntimeCapability` true) for provider-handed
/// clients — capabilities are informational, so the fake hands out a complete
/// set.
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

impl WaslSessionProvider for FakeWaslSessionProvider {
    fn get_or_start(
        &self,
        key: &WaslTargetKey,
    ) -> Result<(Box<dyn WaslSessionClient>, WaslTargetSnapshot), WaslSessionError> {
        let ks = key_string(key);
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get(&ks) {
            // Reattach to the surviving target — no second target spawned. The
            // snapshot surfaces the target's accumulated `last_seq` so the caller
            // runs the §6.1 comparison (there is no `created` bool).
            return Ok((Box::new(entry.client()), entry.snapshot()));
        }
        let target_id = {
            let mut n = self.next_target.lock().unwrap();
            *n += 1;
            format!("wasl-fake-target-{n}")
        };
        // A genuinely-fresh target's journal is empty → snapshot.last_seq == 0.
        let entry = new_entry(&target_id, key.clone(), 0);
        let client = entry.client();
        let snapshot = entry.snapshot();
        entries.insert(ks, entry);
        Ok((Box::new(client), snapshot))
    }

    fn list(&self) -> Result<Vec<String>, WaslSessionError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .values()
            .map(|e| e.target_id.clone())
            .collect())
    }
}
