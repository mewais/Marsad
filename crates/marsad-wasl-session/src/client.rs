//! The caller-side traits (`wasl-backend-plan.md` §4.2/§4.3): `WaslSessionClient`
//! (Marsad's view of ONE Wasl-managed `DeliveryTarget`), `WaslEventStream` (the
//! pull-style observe stream), and `WaslSessionProvider` (the construct/reattach
//! factory seam). All blocking + thread-based to match the house style — there
//! is no async runtime in the workspace.

use crate::error::WaslSessionError;
use crate::types::{
    WaslAgentCard, WaslBusyPolicy, WaslCancelReason, WaslCapabilities, WaslEventSeq,
    WaslGateDecision, WaslShutdownMode, WaslStreamItem, WaslTargetKey, WaslTargetSnapshot,
    WaslTargetStatus, WaslTurnId, WaslTurnReceipt, WaslTurnRequest,
};

/// Pull-style observe stream (house style: std-blocking, thread-pumped — no
/// async). Mirrors Wasl's `TurnEventStream` of `StreamItem` (§3) from the caller
/// side. The agent pumps it on a per-session thread exactly like
/// `spawn_session_reader` pumps a claude NDJSON stream.
pub trait WaslEventStream: Send {
    /// Blocking next stream item; `None` on clean end-of-stream. Returns a
    /// [`WaslStreamItem`] so a `Truncated` control message is surfaced DISTINCTLY
    /// from a normal `Event` (never silently dropped or folded into `Unknown`). A
    /// real impl bounds the read so a dead peer cannot hang the pump forever.
    fn next(&mut self) -> Option<WaslStreamItem>;
}

/// Marsad's caller-side view of ONE Wasl-managed live session (Wasl's
/// `DeliveryTarget`, `wasl-runtime/src/lib.rs` §3). Marsad is the CLIENT: it
/// prompts, observes, resolves native gates, interrupts, and stops — it never
/// owns the process, never writes raw stdin, never implements any
/// `RuntimeCapability`. Transport-abstracted so the in-memory fake and the real
/// HTTP client (Wasl's control server is HTTP/TCP — no UDS; see §3) swap cleanly
/// behind the same trait.
pub trait WaslSessionClient: Send + std::fmt::Debug {
    /// Stable Wasl-side handle for this session (Wasl `target_id()`). This is a
    /// WASL target id, NOT a Marsad session id — core maps between them (§6).
    fn target_id(&self) -> &str;

    /// The session's honest capability set (Wasl `capabilities()`), constant for
    /// its lifetime. INFORMATIONAL — used only to enable/disable UI affordances,
    /// never to gate an admission decision.
    fn capabilities(&self) -> WaslCapabilities;

    /// A best-effort status snapshot (Wasl `status()`). ADVISORY ONLY — may be
    /// stale the instant it returns; NEVER gate a prompt on it (`prompt` is the
    /// atomic admission decision). Used for the observe/status projection (§6).
    fn status(&self) -> WaslTargetStatus;

    /// Submit ONE human-originated prompt (Wasl `submit_turn` with
    /// source = `LocalHuman`, §3). THE ATOMIC ADMISSION DECISION: returns
    /// immediately with a receipt (Started / Queued / RejectedBusy /
    /// RejectedUnavailable / Duplicate) — it does NOT block for the reply, which
    /// arrives via `attach`. `busy_policy` governs the busy case. The
    /// `WaslTurnRequest` carries a fresh single-hop `WaslCausation` (§4.4).
    fn prompt(&self, req: WaslTurnRequest, busy_policy: WaslBusyPolicy) -> WaslTurnReceipt;

    /// Observe-only subscription to the replayable, sequenced event stream (Wasl
    /// `events(after_seq)`, §3). `after_seq` resumes without gaps/dupes. Its
    /// `next()` yields a [`WaslStreamItem`] — either an `Event` or a distinct
    /// `Truncated` control item (§4.4).
    fn attach(&self, after_seq: WaslEventSeq) -> Box<dyn WaslEventStream>;

    /// Resolve a NATIVE gate (Wasl
    /// `NativeGateResolver::resolve_native_gate(turn_id, decision)`, §9 option
    /// (i)). Keyed by the **`turn_id`** of the blocked turn — the real gate has
    /// NO gate id; per-occurrence identity for Marsad's own ledger derives from
    /// `(turn_id, seq)` (§8). Only meaningful when status is `AwaitingGate`;
    /// routed through Wasl's single-writer actor. This is the CLI's OWN permission
    /// prompt — DISTINCT from Wasl's send-side operator gate (X1).
    fn resolve_native_gate(
        &self,
        turn_id: &WaslTurnId,
        decision: WaslGateDecision,
    ) -> Result<(), WaslSessionError>;

    /// Best-effort interrupt of the active turn (Wasl `cancel_turn`, §3.1). Only
    /// meaningful if `capabilities().interruptible`; reflected in the event
    /// stream + status.
    fn interrupt(&self, reason: WaslCancelReason) -> Result<(), WaslSessionError>;

    /// Shut the session down (Wasl `shutdown(mode)`). `Drain` finishes the active
    /// turn then stops; `Immediate` stops now. Marsad does NOT own the process,
    /// so this is a REQUEST to Wasl, not a kill.
    fn shutdown(&self, mode: WaslShutdownMode) -> Result<(), WaslSessionError>;
}

/// Blanket forwarding impl so a shared handle (`Arc<T>`) is itself a
/// [`WaslSessionClient`]. Every method delegates to the inner `T`. This lets
/// callers hold one session behind an `Arc` (e.g. shared across the agent's
/// per-session pump thread and the request path) while still satisfying the
/// trait bound. `?Sized` admits `Arc<dyn WaslSessionClient>` as well.
impl<T: WaslSessionClient + Send + Sync + ?Sized> WaslSessionClient for std::sync::Arc<T> {
    fn target_id(&self) -> &str {
        (**self).target_id()
    }

    fn capabilities(&self) -> WaslCapabilities {
        (**self).capabilities()
    }

    fn status(&self) -> WaslTargetStatus {
        (**self).status()
    }

    fn prompt(&self, req: WaslTurnRequest, busy_policy: WaslBusyPolicy) -> WaslTurnReceipt {
        (**self).prompt(req, busy_policy)
    }

    fn attach(&self, after_seq: WaslEventSeq) -> Box<dyn WaslEventStream> {
        (**self).attach(after_seq)
    }

    fn resolve_native_gate(
        &self,
        turn_id: &WaslTurnId,
        decision: WaslGateDecision,
    ) -> Result<(), WaslSessionError> {
        (**self).resolve_native_gate(turn_id, decision)
    }

    fn interrupt(&self, reason: WaslCancelReason) -> Result<(), WaslSessionError> {
        (**self).interrupt(reason)
    }

    fn shutdown(&self, mode: WaslShutdownMode) -> Result<(), WaslSessionError> {
        (**self).shutdown(mode)
    }
}

/// Constructs / recovers [`WaslSessionClient`]s. Config selects the impl (fake
/// in tests; real HTTP client later). Mirrors Wasl's `RuntimeProvider` (§10) and
/// Marsad's connector-factory seam. `get_or_start` corresponds to Wasl's
/// `POST /v1/targets` (create-or-reuse); `list` to `GET /v1/targets`.
pub trait WaslSessionProvider: Send {
    /// Idempotently obtain a client for the given Wasl target key. If no live
    /// target exists for the key, start one; if a live target already exists
    /// (e.g. Marsad-core restarted while the Wasl session survived — §6.1
    /// reconnect), return a client bound to the EXISTING target WITHOUT starting
    /// a second one (create-or-reuse — never a second competing target).
    ///
    /// Returns the client PLUS the target's current [`WaslTargetSnapshot`]
    /// (§4.3). Real Wasl exposes NO `created` flag — `POST /v1/targets` always
    /// returns a `TargetSnapshot` regardless of create-vs-reuse — so the caller
    /// decides fresh-vs-resumed by comparing `snapshot.last_seq` against its
    /// persisted seq (§6.1), never a boolean racing the wire.
    fn get_or_start(
        &self,
        key: &WaslTargetKey,
    ) -> Result<(Box<dyn WaslSessionClient>, WaslTargetSnapshot), WaslSessionError>;

    /// List currently-known Wasl target ids on this node (Wasl
    /// `GET /v1/targets`).
    fn list(&self) -> Result<Vec<String>, WaslSessionError>;

    /// Register or overwrite a peer node entry on this Wasl node (Wasl
    /// `POST /v1/peers`). A `204 No Content` response maps to `Ok(())`; any
    /// other non-2xx maps to the appropriate [`WaslSessionError`]. Idempotent:
    /// a second call for the same `card.id` overwrites the previous entry.
    fn register_peer(&self, card: WaslAgentCard) -> Result<(), WaslSessionError>;

    /// Deregister a peer node by id (Wasl `DELETE /v1/peers/:id`).
    /// `Ok(true)` when the peer existed and was removed (204); `Ok(false)`
    /// when the peer was already absent (404 with `error:"unknown_peer"`) — a
    /// 404 is NOT an error condition. Any other non-2xx maps to
    /// [`WaslSessionError`].
    fn unregister_peer(&self, id: &str) -> Result<bool, WaslSessionError>;

    /// List all currently-known peer nodes on this Wasl node (Wasl
    /// `GET /v1/peers`). Returns the list sorted by `id` ascending, mirroring
    /// the Wasl wire contract.
    fn list_peers(&self) -> Result<Vec<WaslAgentCard>, WaslSessionError>;
}
