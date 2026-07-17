//! marsad-events — pure reducer: parsed stream messages + hook events → SessionState.
//!
//! See `docs/specs/events.md` for the authoritative contract.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Phase
// ---------------------------------------------------------------------------

/// Lifecycle phase of a Claude Code session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Phase {
    /// Session spawned; no `system/init` received yet.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Stream(marsad_proto::StreamMsg),
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
    /// Per-model token + cost breakdown from the last `result` event.
    /// Cumulative (replaced each time, not accumulated) — consistent with
    /// `cost_usd` and `tokens` which the `result` event authoritatively sets.
    #[serde(default)]
    pub model_usage: std::collections::HashMap<String, marsad_proto::ModelUsage>,
    /// Tools available in this session (from the system/init event).
    #[serde(default)]
    pub tools: Vec<String>,
    /// MCP servers connected in this session (from the system/init event).
    #[serde(default)]
    pub mcp_servers: Vec<marsad_proto::McpServerInfo>,
    /// Slash commands available in this session (from the system/init event).
    #[serde(default)]
    pub slash_commands: Vec<marsad_proto::SlashCommandInfo>,
    /// Permission mode from the system/init event ("default", "acceptEdits",
    /// "bypassPermissions"). `None` before the first system/init.
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Number of context-compaction events (`system/compact_boundary`)
    /// observed this session. Never reset by a reconnect `SystemInit` — this
    /// is session history, not per-connection state.
    #[serde(default)]
    pub compactions: u32,
    /// Metadata from the MOST RECENT compaction, if any (`None` before the
    /// first `compact_boundary`). Replaced (not merged) on each new one.
    #[serde(default)]
    pub last_compaction: Option<marsad_proto::CompactMetadata>,
    /// Latest rate-limit status observed (`rate_limit_event`), if any
    /// (`None` before the first such event this session). Replaced (not
    /// merged) on each new one.
    #[serde(default)]
    pub rate_limit: Option<marsad_proto::RateLimitInfo>,
    /// Message ids already counted for usage DEDUP.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    seen_message_ids: HashSet<String>,
    /// Map from Task tool_use id → subagent index in `subagents`, for resolving
    /// tool_result completions. Keyed by the `id` of the `Task` tool_use block.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    running_task_ids: HashMap<String, usize>,
    /// Why the last turn ended, when noteworthy (see `EndReason`). Drives the
    /// human-facing text on an Error/Cancelled card. `None` for a normal turn or
    /// while working. Cleared when a new turn starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<EndReason>,
    /// Set true by the core layer the moment it sends an `Interrupt` for this
    /// session; read by `reduce_result` so the resulting error turn is classified
    /// as a calm user Cancel (Done) rather than a failure (Error). Cleared once
    /// consumed, or when a new turn starts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupt_requested: bool,
    /// opencode ACP plan-mode: the live plan/todo list entries from the most recent
    /// `system/plan_update` event. Empty before the first plan_update. Replaced
    /// wholesale on each update (not appended — each update is a full snapshot).
    #[serde(default)]
    pub plan_entries: Vec<marsad_proto::PlanEntry>,
    /// opencode ACP: the current session mode id (e.g. "plan", "build") from the
    /// most recent `system/mode_update` event. `None` before the first mode_update.
    #[serde(default)]
    pub current_mode: Option<String>,
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
            model_usage: std::collections::HashMap::new(),
            tools: Vec::new(),
            mcp_servers: Vec::new(),
            slash_commands: Vec::new(),
            permission_mode: None,
            compactions: 0,
            last_compaction: None,
            rate_limit: None,
            seen_message_ids: HashSet::new(),
            running_task_ids: HashMap::new(),
            end_reason: None,
            interrupt_requested: false,
            plan_entries: Vec::new(),
            current_mode: None,
        }
    }

    /// Clear a pending gate the instant the human answers it (optimistic — the
    /// GUI card must disappear immediately, not wait for the next stream message,
    /// which a bare permission-approval may never emit). Flips a lingering
    /// `Needs` phase back to `Working`. The claude stream remains the source of
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
    /// whose `request_id` is already current or queued is ignored (claude re-emit /
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
    /// `SystemInit` when the long-context beta is active for that session
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
        Event::Stream(msg) => reduce_stream(state, msg),
        Event::Hook(hook) => {
            // Stale / Detached preserve their phase for ALL hook events.
            if matches!(state.phase, Phase::Stale | Phase::Detached) {
                return state;
            }
            reduce_hook(state, hook)
        }
    }
}

fn reduce_stream(mut state: SessionState, msg: marsad_proto::StreamMsg) -> SessionState {
    use marsad_proto::StreamMsg;

    // Stale / Detached are only superseded by a fresh SystemInit.
    // Every other stream message is a totality no-op that preserves the phase.
    let stale_or_detached = matches!(state.phase, Phase::Stale | Phase::Detached);

    match msg {
        // SystemInit always supersedes Stale / Detached (and any other phase).
        StreamMsg::SystemInit(init) => {
            state.session_id = Some(init.session_id);
            state.model = Some(init.model);
            state.cwd = Some(init.cwd);
            state.tools = init.tools;
            state.mcp_servers = init.mcp_servers;
            state.slash_commands = init.slash_commands;
            state.permission_mode = Some(init.permission_mode);
            state.phase = Phase::Working;
            // Fresh init = a new turn is beginning; drop any stale end reason /
            // pending interrupt from a prior turn.
            state.end_reason = None;
            state.interrupt_requested = false;
            state
        }

        StreamMsg::Assistant(msg) => {
            if stale_or_detached {
                return state;
            }
            reduce_assistant(state, msg)
        }

        StreamMsg::User(msg) => {
            if stale_or_detached {
                return state;
            }
            // A literal human reply (a `text` content block) answers a pending
            // gate / need. A `user` message that carries ONLY `tool_result`
            // blocks is the CLI's own automatic echo of an already-approved
            // tool's output — the real approval travels over the separate
            // control-response channel (see `reduce_control_request` /
            // `encode_control_response`), never as a stream `user` turn — so it
            // must NOT resolve the gate. The gate stays open (Phase::Needs) until
            // an actual human `text` reply arrives or the gate is answered via
            // the control channel (`clear_gate_on_answer`); `reduce_assistant` /
            // `reduce_result` now key their "don't clobber an open gate" guard on
            // `state.gate` directly, so no separate confirmation flag is needed.
            let has_text_reply = msg
                .message
                .content
                .iter()
                .any(|b| matches!(b, marsad_proto::ContentBlock::Text { .. }));
            if has_text_reply {
                // Advance the gate queue (show the next pending gate if any; else
                // clear + flip to Working). A human reply resolves the current gate.
                state.advance_gate();
                // A human reply starts a new turn — clear any prior end reason.
                state.end_reason = None;
            }
            // Check for tool_result blocks that complete a running Task subagent.
            for block in &msg.message.content {
                if let marsad_proto::ContentBlock::ToolResult { tool_use_id, .. } = block {
                    if let Some(idx) = state.running_task_ids.remove(tool_use_id) {
                        // idx is always valid — we push() then record len()-1.
                        state.subagents[idx].status = SubagentStatus::Done;
                    }
                }
            }
            state
        }

        StreamMsg::Result(result) => {
            if stale_or_detached {
                return state;
            }
            reduce_result(state, result)
        }

        StreamMsg::ControlRequest(req) => {
            if stale_or_detached {
                return state;
            }
            reduce_control_request(state, req)
        }

        // U3b: a completed context compaction. Informational only — bumps
        // the running count and records the latest metadata; does NOT touch
        // phase/tokens/context_pct (those stay authoritative from
        // Assistant/Result usage — see docs/specs/events.md). Same
        // Stale/Detached totality-no-op rule as Assistant/User/Result/
        // control above: it cannot arrive from a dead/detached container
        // without a re-init first.
        StreamMsg::CompactBoundary(cb) => {
            if stale_or_detached {
                return state;
            }
            state.compactions = state.compactions.saturating_add(1);
            state.last_compaction = Some(cb.compact_metadata);
            state
        }

        // U3b: a rate-limit status notice. Informational only — replaces the
        // latest observed status; does not touch phase. Same Stale/Detached
        // totality-no-op rule as above.
        StreamMsg::RateLimitEvent(rl) => {
            if stale_or_detached {
                return state;
            }
            state.rate_limit = Some(rl.rate_limit_info);
            state
        }

        // opencode ACP: live plan/todo list update. Informational — replaces
        // plan_entries wholesale (each event is a full snapshot, not a patch).
        // Does not touch phase or tokens. Same Stale/Detached totality-no-op rule
        // as CompactBoundary.
        StreamMsg::PlanUpdate(pu) => {
            if stale_or_detached {
                return state;
            }
            state.plan_entries = pu.plan_entries;
            state
        }

        // opencode ACP: current session mode changed. Informational — sets
        // current_mode to the new id. Does not touch phase.
        StreamMsg::ModeUpdate(mu) => {
            if stale_or_detached {
                return state;
            }
            state.current_mode = Some(mu.current_mode);
            state
        }

        // opencode ACP: mid-session slash command catalog update. Replaces
        // slash_commands wholesale (each event is a full catalog snapshot).
        // Does not touch phase.
        StreamMsg::SlashUpdate(su) => {
            if stale_or_detached {
                return state;
            }
            state.slash_commands = su.slash_commands;
            state
        }

        // opencode ACP: cost + context window usage update. Purely informational —
        // sets cost_usd and context_pct from the latest values. Replaces (not
        // accumulates): ACP emits cumulative totals, so the latest is authoritative.
        // Does NOT change phase. Same Stale/Detached totality-no-op rule as others.
        StreamMsg::UsageUpdate(uu) => {
            if stale_or_detached {
                return state;
            }
            state.cost_usd = uu.cost_usd;
            // Compute context_pct from used/size; guard against divide-by-zero.
            if uu.context_size > 0 {
                let pct = (uu.context_used as f64 / uu.context_size as f64 * 100.0) as f32;
                state.context_pct = pct.min(100.0);
            }
            state
        }

        // ControlResponse is outbound (no-op); Unknown is forward-compat (no-op).
        // Both are preserved as-is regardless of phase (including Stale/Detached).
        StreamMsg::ControlResponse(_) | StreamMsg::Unknown(_) => state,
    }
}

fn reduce_assistant(mut state: SessionState, msg: marsad_proto::AssistantMsg) -> SessionState {
    let id = &msg.message.id;

    // DEDUP: skip counting if we've already processed this message id.
    if !state.seen_message_ids.contains(id) {
        state.seen_message_ids.insert(id.clone());

        let u = &msg.message.usage;
        let new_tokens = u
            .input_tokens
            .saturating_add(u.output_tokens)
            .saturating_add(u.cache_read_input_tokens)
            .saturating_add(u.cache_creation_input_tokens);
        state.tokens = state.tokens.saturating_add(new_tokens);
        state.update_context_pct();
    }

    // Extract last_action from the first tool-use content block (if any).
    // Also track Task tool_use blocks to mark subagents as Running.
    for block in &msg.message.content {
        if let marsad_proto::ContentBlock::ToolUse { name, input, id } = block {
            let input_str = serde_json::to_string(input).unwrap_or_default();
            state.last_action = Some(format!("{name}({input_str})"));

            // A Task tool_use starts a running subagent. Real claude
            // environments may advertise the subagent-spawning tool as
            // "Agent" instead of "Task" (same `subagent_type`-carrying input
            // shape) — accept both names.
            if name == "Task" || name == "Agent" {
                let subagent_name = input
                    .get("subagent_type")
                    .and_then(|v| v.as_str())
                    .or_else(|| input.get("description").and_then(|v| v.as_str()))
                    .unwrap_or("subagent")
                    .to_string();
                // The Task/Agent tool input carries an optional `model` override;
                // capture it so the UI shows the subagent's real model. Absent =
                // inherits the session model, which the UI leaves blank (never
                // shows the parent's model — that would mislead).
                let subagent_model = input
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let idx = state.subagents.len();
                state.subagents.push(Subagent {
                    name: subagent_name,
                    model: subagent_model,
                    status: SubagentStatus::Running,
                });
                state.running_task_ids.insert(id.clone(), idx);
            }
        }
    }

    // A pending gate means claude is BLOCKED on a `can_use_tool` /
    // AskUserQuestion / ExitPlanMode control_request that has not yet been
    // answered (the agent captured it and no control_response / human reply has
    // cleared it — see `advance_gate` / `clear_gate_on_answer`). An assistant
    // message that arrives while the gate is still open (e.g. claude reasoning
    // or firing further parallel tool calls in the same turn) must NOT paper
    // over that open Need by flipping the phase back to Working — otherwise the
    // gate's card silently leaves the dashboard's attention bucket while the
    // human still owes an answer. We key this on the gate itself, not a stream
    // heuristic: real MCP/opencode turns emit `control_request → assistant`
    // with no intervening `user` tool_result, so the old
    // `gate_confirmed_automated` flag stayed false and the gate got clobbered.
    if state.gate.is_none() {
        state.phase = Phase::Working;
        // Assistant output = an active turn; a prior end reason is stale.
        state.end_reason = None;
    }
    state
}

fn reduce_result(mut state: SessionState, result: marsad_proto::ResultMsg) -> SessionState {
    // Result provides the authoritative cumulative cost for the session.
    state.cost_usd = result.total_cost_usd;

    // Result usage is the authoritative cumulative token total for the session.
    let u = &result.usage;
    state.tokens = u
        .input_tokens
        .saturating_add(u.output_tokens)
        .saturating_add(u.cache_read_input_tokens)
        .saturating_add(u.cache_creation_input_tokens);
    state.update_context_pct();

    // Replace model_usage with the authoritative cumulative per-model breakdown.
    state.model_usage = result.model_usage;

    // A still-pending gate means claude is BLOCKED awaiting a human answer;
    // claude's own turn-level `result` is not that answer, so it must not
    // retroactively mark the session Done/Error out from under a pending Needs.
    // Cost/tokens/model_usage above still reflect the real captured usage either
    // way. Keyed on the gate itself (not the old `gate_confirmed_automated`
    // heuristic) so it holds even when no intervening `user` tool_result arrived.
    //
    // EXCEPTION: a user-initiated interrupt (Escape) genuinely ends the turn —
    // the agent declines the pending gate(s) over the control channel — so we
    // must NOT hold at Needs; fall through to classify the calm Cancel below,
    // which also drops the abandoned gate(s).
    if state.gate.is_some() && !state.interrupt_requested {
        return state;
    }

    // Determine phase + end_reason from error flags / subtype.
    //
    // A turn the USER interrupted (Escape/Stop) comes back as an error result
    // (claude ends the turn with error_during_execution), but it is NOT a
    // failure — classify it as a calm Cancel: phase Done (groups under Done),
    // end_reason Cancelled ("Cancelled by the user"). A genuine error keeps
    // phase Error and carries a real reason so the card is never blank.
    use marsad_proto::ResultSubtype;
    let is_error = result.is_error
        || matches!(
            result.subtype,
            ResultSubtype::ErrorMaxTurns | ResultSubtype::ErrorDuringExecution
        );
    if is_error && state.interrupt_requested {
        state.phase = Phase::Done;
        state.end_reason = Some(EndReason::Cancelled);
        // The cancelled turn abandons any pending gate(s) — the agent declines
        // them on interrupt — so clear them here rather than leaving a stranded
        // Needs card behind a Done turn.
        state.gate = None;
        state.gate_queue.clear();
    } else if is_error {
        state.phase = Phase::Error;
        state.end_reason = Some(match result.subtype {
            ResultSubtype::ErrorMaxTurns => EndReason::MaxTurns,
            _ => EndReason::ExecutionError,
        });
    } else {
        state.phase = Phase::Done;
        state.end_reason = None;
    }
    // The interrupt signal is one-shot: consumed by the turn it ended.
    state.interrupt_requested = false;

    state
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

fn reduce_control_request(
    mut state: SessionState,
    req: marsad_proto::ControlRequest,
) -> SessionState {
    // A gate already awaiting a human answer takes priority — but a second,
    // unrelated control_request is NOT dropped (claude can raise several at once
    // via parallel tool calls, and it waits on ALL of them). It is QUEUED behind
    // the current one via `enqueue_gate` and surfaced after the human answers the
    // first. Previously this early-returned and dropped the new gate while the
    // agent kept the newest, so the answer got misrouted and the turn hung.

    // Detect special HITL tool names first.
    if req.tool_name == "AskUserQuestion" {
        let questions = parse_question_prompts(&req.input);
        let gate = marsad_gates::Gate::Question {
            request_id: req.request_id.clone(),
            questions,
        };
        state.enqueue_gate(gate);
        return state;
    }

    if req.tool_name == "ExitPlanMode" {
        let plan_md = req
            .input
            .get("plan")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let gate = marsad_gates::Gate::Plan {
            request_id: req.request_id.clone(),
            summary: plan_md,
            steps: vec![],
        };
        state.enqueue_gate(gate);
        return state;
    }

    // Build a Permission gate from the control request. The `command` field is
    // what the UI shows as `$ …`, so it must be the ACTUAL invocation, not the
    // bare tool name — a Bash gate used to read `$ Bash` because we copied
    // `tool_name` here instead of reading the command out of `input`.
    let (command, explanation) = if let Some(message) = &req.elicitation_message {
        // MCP elicitation (e.g. hares asking to approve a shell command, or a
        // request_path_access prompt). Show the FULL prompt as the explanation so
        // nothing important is hidden: the path-access prompt carries the path,
        // mode, and reason in its BODY, and the old code discarded it (it showed a
        // generic "Approval requested by MCP server: …" line and, for the `$ …`
        // line, only the first line of the message — so the requested path was
        // never displayed). The `$ …` command line is reserved for a fenced
        // ```code``` block a server put a shell command in; when there is none
        // (e.g. a path request), `elicitation_command` returns "" and the UI hides
        // the command block.
        (elicitation_command(message), message.trim().to_string())
    } else {
        let command = describe_tool_request(&req.tool_name, &req.input);
        // Prefer claude's own description of the action when present (the Bash
        // tool input carries a `description`); fall back to a generic line.
        let explanation = req
            .input
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Tool use request: {}", req.tool_name));
        (command, explanation)
    };
    let gate = marsad_gates::Gate::Permission {
        request_id: req.request_id.clone(),
        command,
        cwd: state.cwd.clone().unwrap_or_default(),
        risk: String::new(),
        explanation,
        is_elicitation: req.elicitation_message.is_some(),
        // Scope-of-allowance choices from the elicitation's requested_schema enum
        // (empty for can_use_tool gates and schemaless elicitations).
        elicitation_choices: marsad_proto::parse_elicitation_choices(
            req.elicitation_schema.as_ref(),
        ),
        // claude's real "always allow …" options for a can_use_tool gate, VERBATIM
        // (empty for elicitations / when claude offers none).
        permission_suggestions: req.permission_suggestions.clone(),
    };
    state.enqueue_gate(gate);
    state
}

/// Parse the `questions` array from an `AskUserQuestion` tool input.
fn parse_question_prompts(input: &serde_json::Value) -> Vec<marsad_gates::QuestionPrompt> {
    let arr = match input.get("questions").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return vec![],
    };
    arr.iter()
        .map(|q| {
            let question = q
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let header = q
                .get("header")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let multi_select = q
                .get("multiSelect")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let options = q
                .get("options")
                .and_then(|v| v.as_array())
                .map(|opts| {
                    opts.iter()
                        .map(|o| marsad_gates::QuestionOption {
                            label: o
                                .get("label")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            description: o
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            marsad_gates::QuestionPrompt {
                question,
                header,
                multi_select,
                options,
            }
        })
        .collect()
}

/// Pull the shell command out of an MCP elicitation prompt for the gate's `$ …`
/// line — but ONLY when the server put one in a fenced ```code``` block (hares'
/// bash-approval prompts do). Returns an empty string when there is no such block
/// (e.g. a `request_path_access` prompt, which has no command). The caller shows
/// the full prompt as the explanation and the UI hides the empty command block —
/// so we never mistake the prompt's first prose line for a command (that dropped
/// the requested path off the path-access gate entirely).
fn elicitation_command(message: &str) -> String {
    if let Some(open) = message.find("```") {
        // Skip the opening fence and its (optional) language tag up to the newline.
        if let Some(nl) = message[open + 3..].find('\n') {
            let after = &message[open + 3 + nl + 1..];
            if let Some(close) = after.find("```") {
                let block = after[..close].trim();
                if !block.is_empty() {
                    return block.to_string();
                }
            }
        }
    }
    String::new()
}

/// Render a human-readable one-line command for a `can_use_tool` permission gate.
///
/// claude nests the real arguments under `input`; the most useful summary depends
/// on the tool. Bash puts the shell command in `input.command` — that's what the
/// user needs to see to make an allow/deny decision. File/search/web tools expose
/// a primary path/pattern/url. Anything unrecognised falls back to the tool name
/// so the gate is never blank.
fn describe_tool_request(tool_name: &str, input: &serde_json::Value) -> String {
    // Bash (and any tool that names its action `command`): show it verbatim.
    if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
        if !cmd.is_empty() {
            return cmd.to_string();
        }
    }
    // File- and search-oriented tools: show "<Tool> <primary-arg>".
    for key in ["file_path", "path", "notebook_path", "pattern", "url"] {
        if let Some(arg) = input.get(key).and_then(|v| v.as_str()) {
            if !arg.is_empty() {
                return format!("{tool_name} {arg}");
            }
        }
    }
    tool_name.to_string()
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
