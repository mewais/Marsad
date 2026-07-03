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
    /// Set once, while `gate` is open, a `user` message arrives that carries
    /// ONLY tool_result blocks (no literal human `text` reply) — i.e. proof
    /// that only automation (the CLI echoing an already-approved tool's own
    /// output), never the human, has spoken since the gate opened. Real
    /// approvals travel over the separate control-response channel and never
    /// appear as a stream event (see `encode_control_response`), so once this
    /// is set, later `assistant`/`result` events must not paper over the
    /// still-unanswered gate by flipping phase away from `Needs`. Cleared
    /// (along with `gate`) only by an actual human `text` reply. Starts
    /// `false` so a gate that closes before any tool_result is ever observed
    /// (e.g. a synthetic/idealized stream that jumps straight from the gate
    /// to the next assistant turn) keeps the original, lenient behavior.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    gate_confirmed_automated: bool,
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
            gate_confirmed_automated: false,
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
            state.tools = init.tools;
            state.mcp_servers = init.mcp_servers;
            state.slash_commands = init.slash_commands;
            state.permission_mode = Some(init.permission_mode);
            state.phase = Phase::Working;
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
            // `encode_control_response`), never as a stream `user` turn. Real
            // captures confirm every `user` line while a permission/plan gate
            // is open is a bare tool_result for the now-approved tool call —
            // seeing one proves definitively that only automation (not the
            // human) has spoken since the gate opened, so it flips
            // `gate_confirmed_automated` and the gate now legitimately stays
            // open (Phase::Needs) for the rest of the fold — see
            // `reduce_assistant` / `reduce_result` — until an actual human
            // `text` reply arrives.
            let has_text_reply = msg
                .message
                .content
                .iter()
                .any(|b| matches!(b, marsad_proto::ContentBlock::Text { .. }));
            if has_text_reply {
                state.gate = None;
                state.gate_confirmed_automated = false;
                state.phase = Phase::Working;
            } else if state.gate.is_some() {
                state.gate_confirmed_automated = true;
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
                let idx = state.subagents.len();
                state.subagents.push(Subagent {
                    name: subagent_name,
                    model: None,
                    status: SubagentStatus::Running,
                });
                state.running_task_ids.insert(id.clone(), idx);
            }
        }
    }

    // A pending gate that has been CONFIRMED automated (see
    // `gate_confirmed_automated` / the `StreamMsg::User` arm) stays open
    // until the human's actual answer — an assistant message that shows up
    // in the meantime (e.g. claude continuing to reason/act on
    // already-approved tool output while the gate itself was only ever
    // resolved by automation, never a literal reply) must not silently paper
    // over the open Need. A gate that has NOT yet seen that confirmation
    // (e.g. a caller that drives `reduce` directly from `ControlRequest`
    // straight to `Assistant`, with no intervening stream `user` event at
    // all) keeps the original, lenient behavior.
    if !state.gate_confirmed_automated {
        state.phase = Phase::Working;
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

    // A gate CONFIRMED automated (see `gate_confirmed_automated`) means the
    // human never actually answered it — claude's own turn-level `result` is
    // not the human's decision, so it must not retroactively mark the
    // session Done/Error out from under a pending Needs. Cost/tokens/
    // model_usage above still reflect the real captured usage either way. A
    // gate that never saw that confirmation (no intervening stream `user`
    // event) keeps the original, lenient behavior — `result` is authoritative.
    if state.gate_confirmed_automated {
        return state;
    }

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
    // A gate already awaiting a human answer takes priority: don't let a
    // second, unrelated control_request silently replace it before the human
    // ever weighed in (a bare tool_result — see the `StreamMsg::User` arm —
    // does NOT count as an answer). Real captures confirm this: claude may
    // open a further gate (e.g. a `Write` permission request right after an
    // approved `ExitPlanMode`) while the earlier one was only ever resolved
    // by its own tool executing, never by literal human input — the FIRST
    // still-open gate is what the human actually owes an answer to.
    if state.gate.is_some() {
        return state;
    }

    // Detect special HITL tool names first.
    if req.tool_name == "AskUserQuestion" {
        let questions = parse_question_prompts(&req.input);
        let gate = marsad_gates::Gate::Question {
            request_id: req.request_id.clone(),
            questions,
        };
        state.gate = Some(gate);
        state.phase = Phase::Needs;
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
            summary: plan_md,
            steps: vec![],
        };
        state.gate = Some(gate);
        state.phase = Phase::Needs;
        return state;
    }

    // Build a Permission gate from the control request. The `command` field is
    // what the UI shows as `$ …`, so it must be the ACTUAL invocation, not the
    // bare tool name — a Bash gate used to read `$ Bash` because we copied
    // `tool_name` here instead of reading the command out of `input`.
    let (command, explanation) = if let Some(message) = &req.elicitation_message {
        // MCP elicitation (e.g. a sandboxed-bash server asking to approve a
        // command). The prompt is free-form markdown; pull the command out of its
        // fenced code block for the `$ …` line and keep the full prompt as the
        // explanation so the user sees the server's full justification.
        (
            elicitation_command(message),
            format!("Approval requested by MCP server: {}", req.tool_name),
        )
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
    };
    state.gate = Some(gate);
    state.phase = Phase::Needs;
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

/// Pull the command out of an MCP elicitation prompt for the gate's `$ …` line.
///
/// MCP approval prompts are free-form markdown; servers conventionally put the
/// command in a fenced ```code``` block (the `hares` sandboxed-bash server does).
/// Prefer that block; else fall back to the first non-empty line; else the whole
/// (trimmed) message — so the gate is never blank.
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
    message
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_else(|| message.trim())
        .to_string()
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
