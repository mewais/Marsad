//! marsad-events — pure reducer: rich agent events + hook events → SessionState.
//!
//! See `docs/specs/events.md` for the authoritative contract.
//!
//! Wave 1 of the wasit migration: the reducer now folds `wasit_core::AgentEvent`
//! (the rich, typed, per-CLI-lossless event model) instead of the old
//! claude-stream-json `marsad_proto::StreamMsg`. The driver-specific parsing that
//! used to live in this crate (`describe_tool_request`, elicitation/question
//! parsing, per-message usage accumulation with message-id dedup) moved into the
//! wasit drivers; Marsad now receives already-shaped events and only projects
//! them onto `SessionState`. Every documented `SessionState` behavior is
//! preserved — see the per-variant comments in `reduce_agent_event`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Phase
// ---------------------------------------------------------------------------

/// Lifecycle phase of a Claude Code session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Phase {
    /// Session spawned; no `SessionInit` received yet.
    Starting,
    /// Claude is actively working.
    Working,
    /// Awaiting user input (gate, notification).
    Needs,
    /// Claude returned an error result.
    Error,
    /// Claude finished successfully.
    Done,
    /// Session process appears stale (set by the lifecycle layer).
    Stale,
    /// Session detached from the running container (set by the lifecycle layer).
    Detached,
}

/// Why a turn ended, when that reason is worth surfacing. `None` on the session
/// means "no noteworthy end reason" (a normal successful turn, or still working).
/// This is the human-facing explanation behind an `Error` card, and it also
/// distinguishes a deliberate user cancel (which lands in `Done`, calm) from a
/// genuine failure (`Error`, red).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndReason {
    /// The user interrupted the turn (Escape / Stop). Not a failure → phase Done.
    Cancelled,
    /// Claude hit its max-turns limit (`result` subtype error_max_turns) → Error.
    MaxTurns,
    /// Claude reported an error during execution → Error.
    ExecutionError,
}

impl EndReason {
    /// Short human-facing text for the card / session view.
    pub fn text(self) -> &'static str {
        match self {
            EndReason::Cancelled => "Cancelled by the user",
            EndReason::MaxTurns => "Reached the max turn limit",
            EndReason::ExecutionError => "Ended with an execution error",
        }
    }
}

// ---------------------------------------------------------------------------
// SubagentStatus / Subagent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SubagentStatus {
    Running,
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subagent {
    pub name: String,
    pub model: Option<String>,
    pub status: SubagentStatus,
}

// ---------------------------------------------------------------------------
// TranscriptEntry
// ---------------------------------------------------------------------------

/// One rendered line of a session's conversation.
///
/// Sourced from claude's authoritative session JSONL (see
/// `marsad_core::discovery`) so it includes the user's own prompts and the full
/// history of a resumed conversation — not just what the live stream echoes.
///
/// Serialized with a `role` tag so the UI can switch on it directly:
/// `{"role":"assistant","text":...}`, `{"role":"user","text":...}`,
/// `{"role":"tool","name":...,"input":...}`,
/// `{"role":"compaction","trigger":...,"dropped_tokens":...}`.
/// Epoch-milliseconds timestamp for a transcript entry, when known. Live entries
/// are stamped by core on arrival; resumed/backfilled entries carry the time
/// parsed from the claude JSONL. `None` for entries whose time is unknown (older
/// data / synthetic test entries). The UI uses it for day separators, idle-gap
/// markers, and per-turn times — NOT a timestamp under every line.
///
/// `#[serde(default, skip_serializing_if = "Option::is_none")]` on every variant
/// keeps the wire clean and lets old serialized transcripts parse unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum TranscriptEntry {
    /// Assistant prose (a `text` content block from claude).
    Assistant {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<i64>,
    },
    /// A user/human turn (a `text` content block in a user message).
    User {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<i64>,
    },
    /// A tool invocation by the assistant (`name` + JSON-encoded `input`).
    Tool {
        name: String,
        input: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<i64>,
    },
    /// A context compaction event (`system/compact_boundary`). Inserted in
    /// sequence so the UI shows the compaction at its chronological position
    /// rather than at the bottom of the transcript.
    Compaction {
        trigger: String,
        dropped_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<i64>,
    },
}

// ---------------------------------------------------------------------------
// HookEvent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HookEvent {
    Notification { message: String },
    Stop,
    SubagentStop { name: String },
    PreToolUse { tool: String },
    PostToolUse { tool: String },
    SessionStart,
    SessionEnd,
    PreCompact,
}

// ---------------------------------------------------------------------------
// Event
// ---------------------------------------------------------------------------

// `Stream` carries the rich `AgentEvent` (≥240 B) while `Hook` is tiny — but an
// `Event` is constructed and immediately reduced, never stored in bulk, so boxing
// the large variant would add an allocation + indirection for no real benefit and
// would change every `Event::Stream(agent_event)` construction site. Same
// disposition as `marsad_proto::IpcMsg`'s `#[allow(clippy::large_enum_variant)]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    /// A rich, typed agent event from a wasit driver (claude / codex / opencode).
    Stream(wasit_core::AgentEvent),
    Hook(HookEvent),
}

// ---------------------------------------------------------------------------
// SessionState
// ---------------------------------------------------------------------------

// Re-export PlanEntry so callers can use it as `marsad_events::PlanEntry`.
pub use marsad_proto::PlanEntry;

/// Derived state of a single Claude Code session; produced by folding events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: Option<String>,
    pub phase: Phase,
    pub model: Option<String>,
    pub cwd: Option<String>,
    /// Total tokens (input + output + cache) consumed across the session.
    pub tokens: u64,
    pub cost_usd: f64,
    /// Consumed tokens as a percentage of the model's context window (0..=100).
    pub context_pct: f32,
    pub last_action: Option<String>,
    pub subagents: Vec<Subagent>,
    pub gate: Option<marsad_gates::Gate>,
    /// Additional pending gates behind the currently-shown `gate`, in arrival
    /// order. claude can raise several permission requests at once (parallel tool
    /// calls); we surface them ONE AT A TIME (`gate` = current, this = the rest)
    /// and advance on each answer. Previously a second gate while one was open was
    /// DROPPED here while the agent kept the newest — so the answer got misrouted
    /// and the turn hung. Empty in the common single-gate case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate_queue: Vec<marsad_gates::Gate>,
    pub changed_files: u32,
    /// Per-model token + cost breakdown from the last `TurnCompleted` event.
    /// Cumulative (replaced each time, not accumulated) — consistent with
    /// `cost_usd` and `tokens` which `TurnCompleted` authoritatively sets.
    #[serde(default)]
    pub model_usage: HashMap<String, marsad_proto::ModelUsage>,
    /// Tools available in this session (from the `SessionInit` event).
    #[serde(default)]
    pub tools: Vec<String>,
    /// MCP servers connected in this session (from the `SessionInit` event).
    #[serde(default)]
    pub mcp_servers: Vec<marsad_proto::McpServerInfo>,
    /// Slash commands available in this session (from `SessionInit` /
    /// `SlashCommandsUpdated`).
    #[serde(default)]
    pub slash_commands: Vec<marsad_proto::SlashCommandInfo>,
    /// Permission mode from the `SessionInit` event ("default", "acceptEdits",
    /// "bypassPermissions"). `None` before the first `SessionInit`.
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Number of context-compaction events (`Compaction`) observed this session.
    /// Never reset by a reconnect `SessionInit` — this is session history, not
    /// per-connection state.
    #[serde(default)]
    pub compactions: u32,
    /// Metadata from the MOST RECENT compaction, if any (`None` before the
    /// first `Compaction`). Replaced (not merged) on each new one.
    #[serde(default)]
    pub last_compaction: Option<marsad_proto::CompactMetadata>,
    /// Latest rate-limit status observed (`RateLimitNotice`), if any
    /// (`None` before the first such event this session). Replaced (not
    /// merged) on each new one.
    #[serde(default)]
    pub rate_limit: Option<marsad_proto::RateLimitInfo>,
    /// Map from a subagent's tool-call id → subagent index in `subagents`, for
    /// resolving `ToolCallFinished` completions. Keyed by the `call_id` of the
    /// subagent-spawning tool call.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    running_task_ids: HashMap<String, usize>,
    /// Why the last turn ended, when noteworthy (see `EndReason`). Drives the
    /// human-facing text on an Error/Cancelled card. `None` for a normal turn or
    /// while working. Cleared when a new turn starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<EndReason>,
    /// Set true by the core layer the moment it sends an `Interrupt` for this
    /// session; read by the `TurnCompleted` arm so the resulting error turn is
    /// classified as a calm user Cancel (Done) rather than a failure (Error).
    /// Cleared once consumed, or when a new turn starts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupt_requested: bool,
    /// opencode ACP plan-mode: the live plan/todo list entries from the most recent
    /// `PlanUpdated` event. Empty before the first plan update. Replaced wholesale
    /// on each update (not appended — each update is a full snapshot).
    #[serde(default)]
    pub plan_entries: Vec<marsad_proto::PlanEntry>,
    /// opencode ACP: the current session mode id (e.g. "plan", "build") from the
    /// most recent `ModesUpdated` / `SessionInit`. `None` before the first one.
    #[serde(default)]
    pub current_mode: Option<String>,
    /// The AVAILABLE agent modes for this session, agent-reported (never a
    /// hardcoded list). opencode fills it from `configOptions.mode`; claude from a
    /// version-derived table (it only reports the current mode). Empty until the
    /// first init/config. The current selection is `current_mode`.
    #[serde(default)]
    pub available_modes: Vec<marsad_proto::SessionMode>,
}

impl SessionState {
    /// Return the zero-valued starting state.
    pub fn initial() -> SessionState {
        SessionState {
            session_id: None,
            phase: Phase::Starting,
            model: None,
            cwd: None,
            tokens: 0,
            cost_usd: 0.0,
            context_pct: 0.0,
            last_action: None,
            subagents: Vec::new(),
            gate: None,
            gate_queue: Vec::new(),
            changed_files: 0,
            model_usage: HashMap::new(),
            tools: Vec::new(),
            mcp_servers: Vec::new(),
            slash_commands: Vec::new(),
            permission_mode: None,
            compactions: 0,
            last_compaction: None,
            rate_limit: None,
            running_task_ids: HashMap::new(),
            end_reason: None,
            interrupt_requested: false,
            plan_entries: Vec::new(),
            current_mode: None,
            available_modes: Vec::new(),
        }
    }

    /// Clear a pending gate the instant the human answers it (optimistic — the
    /// GUI card must disappear immediately, not wait for the next stream message,
    /// which a bare permission-approval may never emit). Flips a lingering
    /// `Needs` phase back to `Working`. The agent stream remains the source of
    /// truth: its subsequent messages re-drive phase/gate as usual.
    pub fn clear_gate_on_answer(&mut self) {
        self.advance_gate();
    }

    /// Discard ALL pending gates (current + queue) when the session is PAUSED.
    /// Pausing kills the host process, so any pending permission/question is
    /// wasted — it can't be answered until a cold resume (like closing the Claude
    /// Code TUI mid-prompt). Clearing it here (not just hiding it behind the PAUSED
    /// badge) keeps the data honest and prevents a stale gate resurfacing on resume.
    /// A lingering `Needs` phase flips back to `Working`.
    pub fn drop_gates_on_pause(&mut self) {
        self.gate = None;
        self.gate_queue.clear();
        if matches!(self.phase, Phase::Needs) {
            self.phase = Phase::Working;
            self.end_reason = None;
        }
    }

    /// Advance to the next queued gate (the human answered/closed the current one).
    /// If another gate is waiting it becomes current (phase stays `Needs`); if the
    /// queue is empty, clear the gate and flip a lingering `Needs` → `Working`.
    fn advance_gate(&mut self) {
        self.gate = if self.gate_queue.is_empty() {
            None
        } else {
            Some(self.gate_queue.remove(0))
        };
        if self.gate.is_none() && matches!(self.phase, Phase::Needs) {
            self.phase = Phase::Working;
            self.end_reason = None;
        }
    }

    /// Enqueue a gate for the human. If nothing is currently shown it becomes the
    /// current `gate`; otherwise it waits behind it (surfaced one-at-a-time). A gate
    /// whose `request_id` is already current or queued is ignored (agent re-emit /
    /// duplicate reader), so it can't pile up. Sets phase `Needs`.
    fn enqueue_gate(&mut self, gate: marsad_gates::Gate) {
        let rid = gate_request_id(&gate);
        let dup = self
            .gate
            .as_ref()
            .map(|g| gate_request_id(g) == rid)
            .unwrap_or(false)
            || self.gate_queue.iter().any(|g| gate_request_id(g) == rid);
        if !dup {
            if self.gate.is_none() {
                self.gate = Some(gate);
            } else {
                self.gate_queue.push(gate);
            }
        }
        self.phase = Phase::Needs;
    }

    /// Return the context window size (in tokens) for a given model name.
    ///
    /// Claude Code appends a `[1m]` suffix to the model string reported in
    /// `SessionInit` when the long-context beta is active for that session
    /// (see `fixtures/streams/qa.ndjson`, which observes
    /// `"model":"claude-opus-4.8[1m]"` alongside `"contextWindow":1000000`);
    /// that suffix is the only reliable, fixture-verified signal we have for
    /// a non-default window. Every other model string Claude Code has been
    /// observed to report — Sonnet/Opus 4.x and Haiku alike — uses the
    /// standard 200_000-token window. There is no verified per-model table
    /// beyond that, so don't invent one: keep this to the one real signal
    /// plus the 200_000 default.
    pub fn context_window_for(model: &str) -> u64 {
        if model.contains("[1m]") {
            1_000_000
        } else {
            200_000
        }
    }

    /// Recompute `context_pct` from the current token count and model window.
    fn update_context_pct(&mut self) {
        let window = self
            .model
            .as_deref()
            .map(SessionState::context_window_for)
            .unwrap_or(200_000);
        let pct = (self.tokens as f64 / window as f64 * 100.0) as f32;
        self.context_pct = pct.min(100.0);
    }
}

// ---------------------------------------------------------------------------
// reduce — pure, total reducer
// ---------------------------------------------------------------------------

/// Fold one event into the session state, returning the next state.
///
/// This function is total: it never panics, never returns an error, and never
/// mutates anything outside the returned state.
pub fn reduce(state: SessionState, event: Event) -> SessionState {
    match event {
        Event::Stream(ev) => reduce_agent_event(state, ev),
        Event::Hook(hook) => {
            // Stale / Detached preserve their phase for ALL hook events.
            if matches!(state.phase, Phase::Stale | Phase::Detached) {
                return state;
            }
            reduce_hook(state, hook)
        }
    }
}

/// The AVAILABLE permission modes for a claude session, derived from its version.
/// claude reports only the CURRENT `permissionMode` on the wire — never the full
/// list — and the set is version-dependent (older claude lacks `bypassPermissions`,
/// i.e. "full autonomy"). This version table is the ONLY non-wire mode source in
/// the system, and it exists solely because claude, unlike opencode, does not
/// advertise its modes. Labels use claude's own terminology (truthful, not a
/// re-invented ask/auto/full abstraction).
///
/// NOTE (Wave 1): the wasit-claude driver now populates `SessionInit.available_modes`
/// from the version, so the reducer no longer calls this at init. It stays public
/// because the capability-discovery path elsewhere in the workspace still uses it
/// to seed `Capabilities.available_modes` before a session is live.
pub fn claude_available_modes(version: &str) -> Vec<marsad_proto::SessionMode> {
    let m = |id: &str, label: &str, desc: &str| marsad_proto::SessionMode {
        id: id.to_string(),
        label: label.to_string(),
        description: desc.to_string(),
    };
    let mut modes = vec![
        m("default", "Default", "Prompt before edits and commands."),
        m(
            "acceptEdits",
            "Accept edits",
            "Apply edits automatically; commands still ask.",
        ),
        m("plan", "Plan", "Read-only planning; propose a plan before acting."),
    ];
    if claude_supports_bypass(version) {
        modes.push(m(
            "bypassPermissions",
            "Bypass permissions",
            "Act without asking. Use only in throwaway sandboxes.",
        ));
    }
    modes
}

/// Whether a claude version offers the `bypassPermissions` ("full autonomy") mode.
/// Present across modern claude (1.x+); a parseable major of 0 drops it, and an
/// unrecognized/blank version assumes modern (the common case). Best-effort — the
/// exact historical cutoff is refinable; it never affects opencode.
fn claude_supports_bypass(version: &str) -> bool {
    match version.split('.').next().and_then(|s| s.parse::<u32>().ok()) {
        Some(major) => major >= 1,
        None => true,
    }
}

/// Fold one `wasit_core::AgentEvent` into the session state.
///
/// Totality rule (preserved from the old `reduce_stream`): once the lifecycle
/// layer has marked a session `Stale` or `Detached`, ONLY a fresh `SessionInit`
/// may supersede that phase — a dead/detached container cannot legitimately emit
/// any other event without a re-init first. Every other variant is a totality
/// no-op that returns the state with its phase unchanged.
fn reduce_agent_event(mut state: SessionState, ev: wasit_core::AgentEvent) -> SessionState {
    use wasit_core::AgentEvent;

    let stale_or_detached = matches!(state.phase, Phase::Stale | Phase::Detached);

    match ev {
        // ── SessionInit: always supersedes Stale / Detached (and any phase) ──
        AgentEvent::SessionInit {
            native_session_id,
            model,
            cwd,
            tools,
            mcp_servers,
            slash_commands,
            permission_mode,
            claude_code_version: _,
            available_modes,
            current_mode,
        } => {
            // native_session_id is optional — only set when the CLI reports one
            // (opencode/claude do; some paths may not).
            if let Some(sid) = native_session_id {
                state.session_id = Some(sid);
            }
            // model / cwd are optional on the wasit event; only overwrite when set.
            if let Some(m) = model {
                state.model = Some(m);
            }
            if let Some(c) = cwd {
                state.cwd = Some(c);
            }
            state.tools = tools;
            state.mcp_servers = mcp_servers;
            state.slash_commands = slash_commands;
            state.permission_mode = Some(permission_mode);
            // Wasit already computed the available-modes list (version-derived for
            // claude, configOptions-derived for opencode) — use it DIRECTLY rather
            // than re-deriving. Only overwrite when non-empty so an init that
            // carries no modes (e.g. codex) leaves any previously-learned list
            // intact, matching the old "don't clobber" behavior.
            if !available_modes.is_empty() {
                state.available_modes = available_modes;
            }
            if !current_mode.is_empty() {
                state.current_mode = Some(current_mode);
            }
            state.phase = Phase::Working;
            // Fresh init = a new turn is beginning; drop any stale end reason /
            // pending interrupt from a prior turn.
            state.end_reason = None;
            state.interrupt_requested = false;
            state
        }

        // ── Content stream: active-turn phase guard only ──────────────────────
        // AssistantDelta / ThinkingDelta mark an active turn: flip Working and
        // clear a stale end_reason, but ONLY when no gate is open (an open gate is
        // a real Need that further same-turn output must not paper over — the card
        // must stay in the attention bucket until the human answers). No token
        // accounting here — usage now arrives via UsageUpdate / TurnCompleted.
        AgentEvent::AssistantDelta { .. } | AgentEvent::ThinkingDelta { .. } => {
            if stale_or_detached {
                return state;
            }
            if state.gate.is_none() {
                state.phase = Phase::Working;
                state.end_reason = None;
            }
            state
        }

        // ── Tool calls ────────────────────────────────────────────────────────
        AgentEvent::ToolCallStarted {
            name,
            call_id,
            input,
            is_subagent,
            subagent_model,
        } => {
            if stale_or_detached {
                return state;
            }
            let input_str = serde_json::to_string(&input).unwrap_or_default();
            state.last_action = Some(format!("{name}({input_str})"));

            // Trust the driver's `is_subagent` flag — do NOT re-sniff Task/Agent.
            if is_subagent {
                let subagent_name = input
                    .get("subagent_type")
                    .and_then(|v| v.as_str())
                    .or_else(|| input.get("description").and_then(|v| v.as_str()))
                    .unwrap_or("subagent")
                    .to_string();
                let idx = state.subagents.len();
                state.subagents.push(Subagent {
                    name: subagent_name,
                    model: subagent_model,
                    status: SubagentStatus::Running,
                });
                state.running_task_ids.insert(call_id, idx);
            }

            // Same active-turn phase guard as content deltas: a tool call is
            // active output, but must not clobber an open gate.
            if state.gate.is_none() {
                state.phase = Phase::Working;
                state.end_reason = None;
            }
            state
        }

        // ToolCallUpdated is an opencode-only intermediate progress signal — no-op.
        AgentEvent::ToolCallUpdated { .. } => state,

        // A tool call finished: if it was a tracked subagent, mark it Done.
        // (Replaces the old "user tool_result closes subagent" path.)
        AgentEvent::ToolCallFinished { call_id, .. } => {
            if stale_or_detached {
                return state;
            }
            if let Some(idx) = state.running_task_ids.remove(&call_id) {
                // idx is always valid — we push() then record len()-1.
                state.subagents[idx].status = SubagentStatus::Done;
            }
            state
        }

        // ── Gates ─────────────────────────────────────────────────────────────
        // Build a `marsad_gates::Gate` from the typed wasit `GateRequest` and
        // enqueue it (dedups by request_id, sets phase Needs). Wasit already
        // computed command/explanation/choices — we do NOT re-run any tool-request
        // description or elicitation parsing here.
        AgentEvent::GateRequested { gate } => {
            if stale_or_detached {
                return state;
            }
            let g = gate_from_wasit(gate, &state);
            state.enqueue_gate(g);
            state
        }

        // ── Usage / cost (live mid-turn snapshot) ─────────────────────────────
        // Supplements TurnCompleted's authoritative totals. cost + tokens are a
        // snapshot (replace, not accumulate — see the token snapshot shift note in
        // the crate docs). context_pct: use the event's value when present
        // (opencode reports it directly), else derive from tokens.
        AgentEvent::UsageUpdate { usage, context_pct } => {
            if stale_or_detached {
                return state;
            }
            state.cost_usd = usage.cost_usd;
            state.tokens = usage
                .input_tokens
                .saturating_add(usage.output_tokens)
                .saturating_add(usage.cache_read_tokens)
                .saturating_add(usage.cache_creation_tokens);
            if let Some(pct) = context_pct {
                state.context_pct = pct.clamp(0.0, 100.0);
            } else {
                state.update_context_pct();
            }
            state
        }

        // ── Turn lifecycle ────────────────────────────────────────────────────
        AgentEvent::TurnStarted { .. } => {
            if stale_or_detached {
                return state;
            }
            if state.gate.is_none() {
                state.phase = Phase::Working;
            }
            state.end_reason = None;
            state
        }

        // TurnCompleted mirrors the old `reduce_result`: authoritative cumulative
        // cost/tokens/model_usage, then the gate/interrupt classification.
        AgentEvent::TurnCompleted {
            end_reason,
            was_interrupted,
            num_turns: _,
            usage,
            per_model,
            reply: _,
        } => {
            if stale_or_detached {
                return state;
            }
            // Authoritative cumulative cost + token total for the session.
            state.cost_usd = usage.cost_usd;
            state.tokens = usage
                .input_tokens
                .saturating_add(usage.output_tokens)
                .saturating_add(usage.cache_read_tokens)
                .saturating_add(usage.cache_creation_tokens);
            state.update_context_pct();
            // Replace model_usage with the authoritative cumulative breakdown.
            state.model_usage = per_model;

            // A still-pending gate means the agent is BLOCKED awaiting a human
            // answer; a turn-level result is not that answer, so it must not
            // retroactively mark the session Done/Error out from under a pending
            // Needs. EXCEPTION: a user interrupt genuinely ends the turn (the agent
            // declines the pending gate over the control channel) — so if the
            // interrupt was requested OR the result reports it, fall through and
            // classify the calm Cancel below (which drops the abandoned gates).
            if state.gate.is_some() && !state.interrupt_requested && !was_interrupted {
                return state;
            }

            // Classify phase + end_reason. A user-interrupted turn is a calm
            // Cancel (Done), not a failure. CompactionNeeded (opencode-only, no
            // marsad EndReason) is treated as a genuine ExecutionError → Error.
            let cancelled = was_interrupted
                || state.interrupt_requested
                || matches!(end_reason, Some(wasit_core::EndReason::Cancelled));
            if cancelled {
                state.phase = Phase::Done;
                state.end_reason = Some(EndReason::Cancelled);
                // The cancelled turn abandons any pending gate(s) — clear them
                // rather than leaving a stranded Needs card behind a Done turn.
                state.gate = None;
                state.gate_queue.clear();
            } else {
                match end_reason {
                    Some(wasit_core::EndReason::MaxTurns) => {
                        state.phase = Phase::Error;
                        state.end_reason = Some(EndReason::MaxTurns);
                    }
                    // ExecutionError and CompactionNeeded both surface as a genuine
                    // execution error (marsad has no CompactionNeeded EndReason;
                    // mapping it to ExecutionError/Error is the honest choice — the
                    // turn did not complete normally).
                    Some(wasit_core::EndReason::ExecutionError)
                    | Some(wasit_core::EndReason::CompactionNeeded) => {
                        state.phase = Phase::Error;
                        state.end_reason = Some(EndReason::ExecutionError);
                    }
                    // A normal, successful turn.
                    Some(wasit_core::EndReason::Cancelled) | None => {
                        state.phase = Phase::Done;
                        state.end_reason = None;
                    }
                }
            }
            // The interrupt signal is one-shot: consumed by the turn it ended.
            state.interrupt_requested = false;
            state
        }

        // ── Plan / mode / model / slash updates ───────────────────────────────
        AgentEvent::PlanUpdated { entries } => {
            if stale_or_detached {
                return state;
            }
            state.plan_entries = entries;
            state
        }

        // opencode/claude mode change. Empty available_modes = "current changed,
        // list unchanged" — don't wipe. current_mode None = list-only change.
        AgentEvent::ModesUpdated {
            available_modes,
            current_mode,
        } => {
            if stale_or_detached {
                return state;
            }
            if !available_modes.is_empty() {
                state.available_modes = available_modes;
            }
            if let Some(cm) = current_mode {
                state.current_mode = Some(cm);
            }
            state
        }

        AgentEvent::ModelChanged { model } => {
            if stale_or_detached {
                return state;
            }
            state.model = Some(model);
            state
        }

        AgentEvent::SlashCommandsUpdated { commands } => {
            if stale_or_detached {
                return state;
            }
            state.slash_commands = commands;
            state
        }

        // ── Compaction / rate limit (informational; don't touch phase) ────────
        AgentEvent::Compaction { info } => {
            if stale_or_detached {
                return state;
            }
            state.compactions = state.compactions.saturating_add(1);
            state.last_compaction = Some(info);
            state
        }

        AgentEvent::RateLimitNotice { info } => {
            if stale_or_detached {
                return state;
            }
            state.rate_limit = Some(info);
            state
        }

        // Process liveness is owned by the agent/core layer (SessionLiveness),
        // not the reducer — ProcessExited is a no-op here.
        AgentEvent::ProcessExited { .. } => state,

        // Forward-compat: an unrecognized event is a no-op (preserved regardless
        // of phase, including Stale/Detached).
        AgentEvent::Unknown { .. } => state,
    }
}

/// Build a `marsad_gates::Gate` from a typed `wasit_core::GateRequest`.
///
/// Wasit already computed the display fields (`command`, `explanation`,
/// `elicitation_choices`) — this only shapes the typed request into the gate
/// domain's enum. The Permission `cwd` falls back to the session's cwd when the
/// request carries none (preserves the old `state.cwd.unwrap_or_default()`).
fn gate_from_wasit(req: wasit_core::GateRequest, state: &SessionState) -> marsad_gates::Gate {
    use wasit_core::GateRequest;
    match req {
        GateRequest::Question {
            request_id,
            questions,
        } => marsad_gates::Gate::Question {
            request_id,
            questions: questions
                .into_iter()
                .map(|q| marsad_gates::QuestionPrompt {
                    question: q.question,
                    header: q.header,
                    multi_select: q.multi_select,
                    options: q
                        .options
                        .into_iter()
                        .map(|o| marsad_gates::QuestionOption {
                            label: o.label,
                            description: o.description,
                        })
                        .collect(),
                })
                .collect(),
        },
        GateRequest::Plan {
            request_id,
            summary,
        } => marsad_gates::Gate::Plan {
            request_id,
            summary,
            steps: vec![],
        },
        GateRequest::Permission {
            request_id,
            tool_name: _,
            command,
            cwd,
            explanation,
            is_elicitation,
            elicitation_choices,
            permission_suggestions,
            raw_input: _,
        } => marsad_gates::Gate::Permission {
            request_id,
            command,
            // Prefer the request's cwd; fall back to the session's when empty.
            cwd: if cwd.is_empty() {
                state.cwd.clone().unwrap_or_default()
            } else {
                cwd
            },
            risk: String::new(),
            explanation,
            is_elicitation,
            // wasit's ElicitationChoice IS marsad_proto::ElicitationChoice (re-export).
            elicitation_choices,
            permission_suggestions,
        },
    }
}

/// A gate's identity for dedup/queue purposes: `request_id` for the claude gate
/// kinds, `gate_id` for a connector Handoff.
fn gate_request_id(gate: &marsad_gates::Gate) -> &str {
    match gate {
        marsad_gates::Gate::Permission { request_id, .. }
        | marsad_gates::Gate::Plan { request_id, .. }
        | marsad_gates::Gate::Edit { request_id, .. }
        | marsad_gates::Gate::Question { request_id, .. } => request_id,
        marsad_gates::Gate::Handoff { gate_id, .. } => gate_id,
    }
}

fn reduce_hook(mut state: SessionState, hook: HookEvent) -> SessionState {
    match hook {
        HookEvent::Notification { .. } => {
            state.phase = Phase::Needs;
            state
        }
        HookEvent::Stop => {
            state.phase = Phase::Done;
            state
        }
        HookEvent::SubagentStop { name } => {
            // Record this subagent as Done. Each SubagentStop event appends a
            // Done record; the lifecycle layer deduplicates by name if needed.
            state.subagents.push(Subagent {
                name,
                model: None,
                status: SubagentStatus::Done,
            });
            state
        }
        HookEvent::PreToolUse { .. } | HookEvent::PostToolUse { .. } => state,
        HookEvent::SessionStart | HookEvent::SessionEnd | HookEvent::PreCompact => state,
    }
}
