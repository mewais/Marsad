//! marsad-events — pure reducer: parsed stream messages + hook events → SessionState.
//!
//! See `docs/specs/events.md` for the authoritative contract.

use std::collections::HashSet;

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
/// `{"role":"tool","name":...,"input":...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum TranscriptEntry {
    /// Assistant prose (a `text` content block from claude).
    Assistant { text: String },
    /// A user/human turn (a `text` content block in a user message).
    User { text: String },
    /// A tool invocation by the assistant (`name` + JSON-encoded `input`).
    Tool { name: String, input: String },
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
    pub changed_files: u32,
    /// Message ids already counted for usage DEDUP.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    seen_message_ids: HashSet<String>,
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
            changed_files: 0,
            seen_message_ids: HashSet::new(),
        }
    }

    /// Return the context window size (in tokens) for a given model name.
    ///
    /// Models whose names contain `[1m]` always return 1_000_000.
    /// Known named models return their documented window; unknown models fall
    /// back to 200_000 (the current Claude 3 default).
    pub fn context_window_for(model: &str) -> u64 {
        // The [1m] suffix signals a 1M-token context window.
        if model.contains("[1m]") {
            return 1_000_000;
        }

        // Named model table; all fall back to 200_000 (current Claude standard).
        match model {
            "claude-opus-4-5"
            | "claude-sonnet-4-5"
            | "claude-sonnet-4-6"
            | "claude-3-5-sonnet-20241022"
            | "claude-3-5-haiku-20241022"
            | "claude-haiku-3-5"
            | "claude-3-opus-20240229"
            | "claude-3-haiku-20240307" => 200_000,
            _ => 200_000,
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
            state.phase = Phase::Working;
            state
        }

        StreamMsg::Assistant(msg) => {
            if stale_or_detached {
                return state;
            }
            reduce_assistant(state, msg)
        }

        StreamMsg::User(_) => {
            if stale_or_detached {
                return state;
            }
            // A user turn answers a pending gate / need.
            state.gate = None;
            state.phase = Phase::Working;
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
    for block in &msg.message.content {
        if let marsad_proto::ContentBlock::ToolUse { name, input, .. } = block {
            let input_str = serde_json::to_string(input).unwrap_or_default();
            state.last_action = Some(format!("{name}({input_str})"));
        }
    }

    state.phase = Phase::Working;
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

    // Determine phase from error flags / subtype.
    use marsad_proto::ResultSubtype;
    state.phase = if result.is_error {
        Phase::Error
    } else {
        match result.subtype {
            ResultSubtype::Success => Phase::Done,
            ResultSubtype::ErrorMaxTurns | ResultSubtype::ErrorDuringExecution => Phase::Error,
        }
    };

    state
}

fn reduce_control_request(
    mut state: SessionState,
    req: marsad_proto::ControlRequest,
) -> SessionState {
    // Build a Permission gate from the control request. The `command` field is
    // what the UI shows as `$ …`, so it must be the ACTUAL tool invocation, not
    // the bare tool name — a Bash gate used to read `$ Bash` because we copied
    // `tool_name` here instead of reading the command out of `input`.
    let command = describe_tool_request(&req.tool_name, &req.input);
    // Prefer claude's own description of the action when present (the Bash tool
    // input carries a `description`); fall back to a generic line.
    let explanation = req
        .input
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Tool use request: {}", req.tool_name));
    let gate = marsad_gates::Gate::Permission {
        request_id: req.request_id.clone(),
        command,
        cwd: state.cwd.clone().unwrap_or_default(),
        risk: String::new(),
        explanation,
    };
    state.gate = Some(gate);
    state.phase = Phase::Needs;
    state
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
