//! Caller-side mirror of Wasl's frozen session-control contract
//! (`wasl-backend-plan.md` §4.4, v3). These types are Marsad's view of the values
//! that cross Wasl's `DeliveryTarget` seam — nothing here depends on any Wasl
//! crate; the fake (§5) is their executable form until Wasl ships fixtures.
//!
//! All domain structs/enums derive `Serialize`/`Deserialize` with **no**
//! `deny_unknown_fields` (forward-compat, the X1 §2.1 convention): a v1-additive
//! Wasl may grow fields Marsad hasn't modelled and must still parse. Where
//! Marsad genuinely branches exhaustively (status kind, receipt kind, event
//! kind, stream item) the enum carries an `Unknown(String)` forward-compat arm
//! where applicable.

use serde::{Deserialize, Serialize};
// Re-export the marsad_proto payload types that appear in the new rich variants
// so callers don't need a direct marsad-proto dep just to pattern-match them.
pub use marsad_proto::{CompactionInfo, PlanEntry, SessionMode, SlashCommandInfo};

/// Monotonic event sequence number on the observe stream (Wasl `events`
/// `after_seq`), used for gap/dup-free replay.
pub type WaslEventSeq = u64;

/// Mirror of Wasl `RuntimeCapabilities` (§2). Eight independent booleans;
/// **informational only** — Marsad never replicates a capability and never
/// assumes one implies another, it only enables/disables UI affordances.
/// NOTE (v3): for all shipped drivers today (claude, opencode) `interruptible`
/// and `streaming_output` are genuinely `false` — Marsad's affordance logic
/// should expect these hidden (§3, §11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaslCapabilities {
    pub persistent_process: bool,
    pub persistent_conversation: bool,
    pub structured_turns: bool,
    pub streaming_output: bool,
    pub attachable: bool,
    pub interruptible: bool,
    pub structured_provenance: bool,
    pub resumable: bool,
}

impl WaslCapabilities {
    /// Whether Marsad should show the "interrupt" affordance for this session.
    /// A pure read of the honest capability set — the client is still *callable*
    /// regardless (interrupt is best-effort per the trait contract), this only
    /// governs whether the UI offers the button (§6.3 "affordance" note).
    pub fn can_interrupt(&self) -> bool {
        self.interruptible
    }
}

/// The CLI's own native permission-prompt detail (Wasl `NativeGateDetail`,
/// `wasl-runtime/src/lib.rs` §4.1). The REAL type is ONLY `{summary, preview}` —
/// there is NO `gate_id` and NO `tool` field (v3 correction). Per-occurrence
/// identity comes from the sibling `turn_id` (on the event / status) combined
/// with the event's stream `seq` (§8). Mapped into the existing
/// `marsad_gates::Gate::Permission` variant and rendered by Marsad's existing
/// gate UI (§8); Marsad does not interpret it beyond display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaslNativeGateDetail {
    pub summary: String,
    pub preview: String,
}

/// Mirror of Wasl `TargetStatus` (§4.1). Branched-on by the status projection
/// (§6.3), so it is an enum with a forward-compat `Unknown` arm. Timestamps are
/// RFC-3339 `String`s — Marsad displays them, never does clock math (X1 §2.1).
/// `Interrupting`/`Draining` carry `{active_turn, since}` — they are NOT bare
/// unit variants in real Wasl (v3, re-confirmed against real source).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaslTargetStatus {
    Offline,
    Starting,
    Idle {
        since: String,
    },
    Busy {
        active_turn: String,
        source: String,
        phase: String,
        since: String,
        queued: usize,
    },
    AwaitingGate {
        active_turn: String,
        detail: WaslNativeGateDetail,
        since: String,
        queued: usize,
    },
    Interrupting {
        active_turn: String,
        since: String,
    },
    Draining {
        active_turn: String,
        since: String,
    },
    Faulted {
        error: String,
        recoverable: bool,
    },
    Stopped,
    /// Forward-compat: a status kind a future Wasl introduced. Projected to
    /// Marsad's `Phase` conservatively (§6.3). Carries the raw tag for logging.
    Unknown(String),
}

/// Mirror of Wasl `TurnReceipt` (§3; fixtures `receipt_*.json`) — the atomic
/// admission decision returned by `prompt`. Never races with `status()`.
/// Includes the real `Duplicate` variant (v3): a resend of an already-seen
/// `(TargetKey, IdempotencyKey)` returns the ORIGINAL receipt wrapped verbatim —
/// Wasl does NOT double-inject (fixture `receipt_duplicate.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaslTurnReceipt {
    Started {
        turn_id: String,
        first_seq: WaslEventSeq,
    },
    Queued {
        turn_id: String,
        position: usize,
    },
    RejectedBusy {
        active_turn: String,
    },
    RejectedUnavailable {
        status: WaslTargetStatus,
    },
    /// A resent idempotency key: the wrapped original receipt, no second
    /// injection.
    Duplicate {
        original: Box<WaslTurnReceipt>,
    },
}

/// Mirror of Wasl `BusyPolicy` (§4.2). Marsad picks the policy per prompt
/// (default: `Queue` with a bounded wait for a human `LocalHuman` prompt — §12
/// Q4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaslBusyPolicy {
    Queue { max_wait_ms: u64 },
    Reject,
    QueueRelated { max_wait_ms: u64 },
}

/// One human-originated prompt (Wasl `TurnRequest`, source implicitly
/// `LocalHuman` — Marsad NEVER originates an Agent turn, §11). Carries a fresh
/// single-hop `WaslCausation` (v3) since Marsad only ever originates root turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaslTurnRequest {
    /// Idempotency key so a resend does not double-inject (Wasl returns
    /// `Duplicate` for a repeat). A **UUID** minted once per logical human send
    /// action and reused verbatim on every retry of that same send — see
    /// [`crate::new_idempotency_key`] and §12 Q5 (DECIDED). NOT a per-process
    /// counter (which can collide across a Marsad-core restart while the Wasl
    /// session survives).
    pub idempotency_key: String,
    pub text: String,
    /// The synchronous/queued call-path metadata Wasl requires on every turn
    /// (Wasl `Causation`, §8.2). Marsad only ever originates ROOT, non-A2A-chained
    /// `LocalHuman` turns, so this is ALWAYS a fresh single-hop causation per send
    /// (see [`WaslCausation::fresh`]).
    pub causation: WaslCausation,
}

/// Mirror of Wasl `Causation` (§8.2; wire fields per `runtime-control-api.md`).
/// Marsad always builds a fresh single-hop instance (v3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaslCausation {
    /// The root request that started this call path. For Marsad, a freshly-minted
    /// id per send.
    pub root_request: String,
    /// The immediate parent request. ALWAYS `None` for Marsad (no chaining).
    pub parent_request: Option<String>,
    /// Agents already on this call path. ALWAYS empty for Marsad.
    pub path: Vec<String>,
    /// Distance from the root. ALWAYS 0 for Marsad (root turn).
    pub hop: u32,
}

impl WaslCausation {
    /// The only constructor Marsad uses: a fresh single-hop root causation (no
    /// parent, empty path, hop 0). Documented as the correct default for a fresh
    /// non-chained `LocalHuman` turn (§3, §11).
    pub fn fresh(root_request: String) -> Self {
        Self {
            root_request,
            parent_request: None,
            path: Vec::new(),
            hop: 0,
        }
    }
}

/// A Wasl-side per-turn id (Wasl `TurnId`, `wasl-runtime/src/lib.rs` §1). This is
/// what [`crate::WaslSessionClient::resolve_native_gate`] is keyed by (there is
/// no gate id) and what every `TurnEvent` correlates by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaslTurnId(pub String);

/// The human's decision on a native gate (Wasl `gate approve|deny`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaslGateDecision {
    Approve,
    Deny { reason: String },
}

/// How to shut a session down (Wasl `shutdown(mode)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaslShutdownMode {
    Drain,
    Immediate,
}

/// Why the active turn is being cancelled (Wasl `cancel_turn` reason).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaslCancelReason {
    Human,
    Superseded,
}

/// One normalized observe event Marsad consumes from `attach` (Wasl `TurnEvent`,
/// `wasl-runtime/src/lib.rs` §5). Carries its sequence for replay/resubscribe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaslSessionEvent {
    pub seq: WaslEventSeq,
    pub kind: WaslSessionEventKind,
}

/// The public event vocabulary Marsad consumes from `attach` (Wasl `TurnEvent`,
/// `wasl-runtime/src/lib.rs` §5; wire fixtures `turn_event_*.json`).
/// Field-for-field with the real enum (v3): EVERY kind correlates by `turn_id`
/// (Wasl's rule: correlate by TurnId, never by seq), and
/// `TurnCompleted`/`TurnFailed` carry a `certainty` (`DeliveryCertainty`, kept as
/// a `String` Marsad only relays/renders). Marsad adds two client-synthesized
/// kinds (`ObserveLost`/`Unknown`) with no real-Wasl counterpart, by design.
///
/// `Eq` is manually provided via the `PartialEq` derive (bit-equality) so that
/// variants carrying `f64` fields (`UsageUpdate::cost_usd`) and types that don't
/// derive `Eq` (`PlanEntry`/`SessionMode`/`SlashCommandInfo`) still satisfy `Eq`
/// for use in `HashSet` and `BTreeSet` keys (matching how `wasl-runtime`'s own
/// `UsageSnapshot` provides a manual `Eq`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WaslSessionEventKind {
    SessionReady {
        native: Option<String>,
    },
    TurnAccepted {
        turn_id: String,
        source: String,
    },
    TurnQueued {
        turn_id: String,
        position: usize,
    },
    AssistantDelta {
        turn_id: String,
        text: String,
    },
    ToolStarted {
        turn_id: String,
        name: String,
        /// The tool's JSON input as the driver reported it (Wasl
        /// `TurnEvent::ToolStarted.input`). `Null` when the driver does not
        /// surface args (e.g. codex). Folded into `TranscriptEntry::Tool.input`
        /// (JSON-stringified) so a Wasl-managed session's tool block shows the
        /// invocation arguments, exactly like a native session.
        #[serde(default)]
        input: serde_json::Value,
    },
    ToolFinished {
        turn_id: String,
        name: String,
        ok: bool,
        /// The tool's result content, if the driver surfaced it (Wasl
        /// `TurnEvent::ToolFinished.result`). `None` when absent (e.g. codex,
        /// which reports only an exit code).
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    NativeGateRequested {
        turn_id: String,
        detail: WaslNativeGateDetail,
    },
    NativeGateResolved {
        turn_id: String,
        decision: String,
    },
    TurnCompleted {
        turn_id: String,
        reply: String,
        certainty: String,
    },
    TurnFailed {
        turn_id: String,
        error: String,
        certainty: String,
    },
    TurnCancelled {
        turn_id: String,
        reason: String,
    },
    TurnExpired {
        turn_id: String,
    },
    StatusChanged {
        status: WaslTargetStatus,
    },
    /// The underlying Wasl session process exited (Wasl `ProcessExited`).
    /// Terminal.
    SessionExited {
        code: Option<i32>,
    },
    /// Marsad-synthesized (NO real-Wasl counterpart, by design): the client lost
    /// its observe connection and will resubscribe. Informational — drives the
    /// `Detached` projection, analogous to `marsad-wasl`'s
    /// `WaslEvent::Disconnected`.
    ObserveLost {
        error: String,
    },
    /// Forward-compat (Marsad-synthesized): an event kind a future Wasl
    /// introduced. Logged + ignored by the folder (X1 tolerant-skip). NOTE: a
    /// stream `Truncated` is NOT an `Unknown` — it is a distinct
    /// [`WaslStreamItem::Truncated`], an unmissable signal, never folded here.
    Unknown(String),

    // ── Rich informational variants (grown from wasit-adapter work) ──────────
    // These mirror the additive `TurnEvent` variants in `wasl-runtime/src/lib.rs`
    // that were added after the lean set above. They carry no `turn_id` — they
    // are session-global observability signals, not per-turn.

    /// Incremental token/cost snapshot from the session. Mapped from the Wasl
    /// wire `usage_update` variant. `tokens` is the sum of all token kinds
    /// (input + output + cache_read + cache_creation). `context_pct` is 0–100
    /// when the driver reports it, `None` when absent.
    UsageUpdate {
        cost_usd: f64,
        tokens: u64,
        context_pct: Option<u8>,
    },
    /// Full replacement of the opencode plan/todo list. Mapped from `plan_updated`.
    PlanUpdated {
        entries: Vec<PlanEntry>,
    },
    /// Available mode list or current mode changed. Mapped from `modes_updated`.
    /// `available_modes` is empty when only `current_mode` changed.
    ModesUpdated {
        available_modes: Vec<SessionMode>,
        current_mode: Option<String>,
    },
    /// Active model changed mid-session. Mapped from `model_changed`.
    ModelChanged {
        model: String,
    },
    /// Slash-command catalog replaced. Mapped from `slash_commands_updated`.
    SlashCommandsUpdated {
        commands: Vec<SlashCommandInfo>,
    },
    /// Context compaction occurred. Mapped from `compaction`.
    Compaction {
        info: CompactionInfo,
    },
}

// Manual `Eq` for `WaslSessionEventKind`: `PartialEq` is derived above; since
// `PartialEq` is structural (field-by-field) and reflexive for all our field
// types, `Eq` holds trivially (the only non-Eq field is `f64` in `UsageUpdate`,
// which we treat as bit-equal, matching `wasl-runtime`'s `UsageSnapshot` pattern).
impl Eq for WaslSessionEventKind {}

// `WaslSessionEvent` wraps `WaslSessionEventKind`. Propagate the manual Eq.
impl Eq for WaslSessionEvent {}

/// What [`crate::WaslEventStream::next`] yields — the caller-side mirror of
/// Wasl's `StreamItem` (`wasl-runtime/src/lib.rs` §3). It is EITHER a normal
/// sequenced event OR a distinct `Truncated` control item; the two must never be
/// conflated (a truncation is NOT an `Unknown` event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WaslStreamItem {
    /// A journalled event at its sequence.
    Event(WaslSessionEvent),
    /// The replay cursor (`after_seq`) fell below the retained ring floor
    /// (default capacity 4096 events, `wasl-runtime/src/config.rs`; SSE event name
    /// `"truncated"`; fixture `truncated.json`). The caller MUST re-fetch a
    /// snapshot (`get_or_start`/`status`) and resubscribe from `latest`. This is
    /// an UNMISSABLE signal, distinct from any event — never folded into
    /// `Unknown`.
    Truncated {
        earliest: WaslEventSeq,
        latest: WaslEventSeq,
    },
}

// Manual `Eq` for `WaslStreamItem`: propagate from the manual `Eq` on
// `WaslSessionEvent`/`WaslSessionEventKind`.
impl Eq for WaslStreamItem {}

/// The opaque Wasl target key (protocol id + context) used by
/// [`crate::WaslSessionProvider::get_or_start`] to obtain or reattach to a
/// client. Value-carried on purpose (the wire form is a `serde_json::Value`, the
/// same rule that keeps `marsad-proto` free of a `marsad-wasl-session`
/// dependency, §4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaslTargetKey(pub serde_json::Value);

/// Marsad's caller-side view of a Wasl peer node entry (Wasl
/// `GET /v1/peers` element and `POST /v1/peers` body). Describes one agent node
/// that the Wasl runtime knows about for peer-to-peer routing. `endpoint` is
/// optional: `None` when the peer's HTTP control address is not yet known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaslAgentCard {
    /// The peer node's stable id.
    pub id: String,
    /// The peer node's hostname or address label.
    pub host: String,
    /// The peer node's HTTP control endpoint (e.g. `"http://10.0.0.2:8787"`),
    /// or `None` when not yet known.
    pub endpoint: Option<String>,
}

/// Caller-side mirror of Wasl's `TargetSnapshot` (`wasl-runtime-control/src/wire.rs`;
/// fixture `target.json`), returned by [`crate::WaslSessionProvider::get_or_start`]
/// (§4.3). There is NO `created` flag in real Wasl — `POST /v1/targets` is
/// create-or-reuse and ALWAYS returns this shape — so fresh-vs-resumed is decided
/// by comparing `last_seq` against Marsad's persisted seq (§6.1), never a boolean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaslTargetSnapshot {
    /// Wasl-side stable target id (`target_id()`), the handle to reattach to.
    pub target_id: String,
    /// The opaque target key this snapshot was created/reused for.
    pub key: WaslTargetKey,
    /// The target's honest capability set (constant for its lifetime).
    pub capabilities: WaslCapabilities,
    /// The advisory status the snapshot carried.
    pub status: WaslTargetStatus,
    /// The highest journalled event sequence the target has emitted. Compared
    /// against Marsad's persisted last-known seq to detect a fresh/replaced
    /// target (§6.1): `snapshot.last_seq < persisted ⇒ replaced ⇒ reset to 0`.
    pub last_seq: WaslEventSeq,
    /// Turns currently queued behind the active turn.
    pub queued: usize,
    /// The underlying native CLI session id, if the target has one.
    pub native_session_id: Option<String>,
}
