//! Shared test helpers: construct `wasit_core::AgentEvent` values and wrap them
//! as `marsad_events::Event::Stream`.
//!
//! Wave 1 moved the raw claude/opencode stream-json parsing out of
//! `marsad-events` and into the wasit drivers, so these tests no longer build
//! `StreamMsg` from JSON via `parse_stream_line`. Instead they construct the
//! rich `AgentEvent` the reducer now folds — the equivalent event the wasit
//! driver would have emitted for the same wire line.

#![allow(dead_code)]

use std::collections::HashMap;

use marsad_events::{reduce, Event, SessionState};
use wasit_core::{
    AgentEvent, ElicitationChoice, GateRequest, McpServerInfo, ModelUsage, PlanEntry,
    QuestionOption, QuestionPrompt, SessionMode, SlashCommandInfo, Usage,
};

/// Wrap an `AgentEvent` as a stream `Event`.
pub fn ev(e: AgentEvent) -> Event {
    Event::Stream(e)
}

/// A default `SessionInit` (claude-shaped) with a version so the reducer's
/// downstream code has a Working session with modes. `available_modes` is left
/// empty here (the wasit driver populates it) unless a test wants them.
pub fn session_init(session_id: &str, model: &str, cwd: &str) -> AgentEvent {
    AgentEvent::SessionInit {
        native_session_id: Some(session_id.to_string()),
        model: Some(model.to_string()),
        cwd: Some(cwd.to_string()),
        tools: vec![],
        mcp_servers: vec![],
        slash_commands: vec![],
        permission_mode: "default".to_string(),
        claude_code_version: String::new(),
        available_modes: vec![],
        current_mode: String::new(),
    }
}

/// A `SessionInit` with explicit permission_mode.
pub fn session_init_mode(
    session_id: &str,
    model: &str,
    cwd: &str,
    permission_mode: &str,
) -> AgentEvent {
    AgentEvent::SessionInit {
        native_session_id: Some(session_id.to_string()),
        model: Some(model.to_string()),
        cwd: Some(cwd.to_string()),
        tools: vec![],
        mcp_servers: vec![],
        slash_commands: vec![],
        permission_mode: permission_mode.to_string(),
        claude_code_version: String::new(),
        available_modes: vec![],
        current_mode: String::new(),
    }
}

/// A `SessionInit` carrying tools / mcp_servers / slash_commands.
pub fn session_init_full(
    session_id: &str,
    model: &str,
    cwd: &str,
    tools: Vec<String>,
    mcp_servers: Vec<McpServerInfo>,
    slash_commands: Vec<SlashCommandInfo>,
) -> AgentEvent {
    AgentEvent::SessionInit {
        native_session_id: Some(session_id.to_string()),
        model: Some(model.to_string()),
        cwd: Some(cwd.to_string()),
        tools,
        mcp_servers,
        slash_commands,
        permission_mode: "default".to_string(),
        claude_code_version: String::new(),
        available_modes: vec![],
        current_mode: String::new(),
    }
}

/// A `SessionInit` with a claude version (→ driver-derived modes) + current mode.
pub fn session_init_versioned(
    model: &str,
    permission_mode: &str,
    version: &str,
    available_modes: Vec<SessionMode>,
) -> AgentEvent {
    AgentEvent::SessionInit {
        native_session_id: Some("s".to_string()),
        model: Some(model.to_string()),
        cwd: Some("/p".to_string()),
        tools: vec![],
        mcp_servers: vec![],
        slash_commands: vec![],
        permission_mode: permission_mode.to_string(),
        claude_code_version: version.to_string(),
        available_modes,
        current_mode: permission_mode.to_string(),
    }
}

/// An assistant prose delta.
pub fn assistant_delta(text: &str) -> AgentEvent {
    AgentEvent::AssistantDelta {
        text: text.to_string(),
        message_id: None,
    }
}

/// A plain (non-subagent) tool call started.
pub fn tool_call(name: &str, call_id: &str, input: serde_json::Value) -> AgentEvent {
    AgentEvent::ToolCallStarted {
        name: name.to_string(),
        call_id: call_id.to_string(),
        input,
        is_subagent: false,
        subagent_model: None,
    }
}

/// A subagent tool call started.
pub fn subagent_call(
    call_id: &str,
    input: serde_json::Value,
    model: Option<String>,
) -> AgentEvent {
    AgentEvent::ToolCallStarted {
        name: "Task".to_string(),
        call_id: call_id.to_string(),
        input,
        is_subagent: true,
        subagent_model: model,
    }
}

/// A tool call finished.
pub fn tool_finished(call_id: &str, name: &str, ok: bool) -> AgentEvent {
    AgentEvent::ToolCallFinished {
        call_id: call_id.to_string(),
        name: name.to_string(),
        ok,
        result: None,
    }
}

/// A permission gate request (can_use_tool style — wasit already computed the
/// display command/explanation).
pub fn permission_gate(request_id: &str, command: &str, explanation: &str) -> AgentEvent {
    AgentEvent::GateRequested {
        gate: GateRequest::Permission {
            request_id: request_id.to_string(),
            tool_name: "Bash".to_string(),
            command: command.to_string(),
            cwd: String::new(),
            explanation: explanation.to_string(),
            is_elicitation: false,
            elicitation_choices: vec![],
            permission_suggestions: vec![],
            raw_input: serde_json::Value::Null,
        },
    }
}

/// A permission gate request with full control over the fields.
#[allow(clippy::too_many_arguments)]
pub fn permission_gate_full(
    request_id: &str,
    tool_name: &str,
    command: &str,
    cwd: &str,
    explanation: &str,
    is_elicitation: bool,
    elicitation_choices: Vec<ElicitationChoice>,
    permission_suggestions: Vec<serde_json::Value>,
) -> AgentEvent {
    AgentEvent::GateRequested {
        gate: GateRequest::Permission {
            request_id: request_id.to_string(),
            tool_name: tool_name.to_string(),
            command: command.to_string(),
            cwd: cwd.to_string(),
            explanation: explanation.to_string(),
            is_elicitation,
            elicitation_choices,
            permission_suggestions,
            raw_input: serde_json::Value::Null,
        },
    }
}

/// A plan gate request.
pub fn plan_gate(request_id: &str, summary: &str) -> AgentEvent {
    AgentEvent::GateRequested {
        gate: GateRequest::Plan {
            request_id: request_id.to_string(),
            summary: summary.to_string(),
        },
    }
}

/// A question gate request.
pub fn question_gate(request_id: &str, questions: Vec<QuestionPrompt>) -> AgentEvent {
    AgentEvent::GateRequested {
        gate: GateRequest::Question {
            request_id: request_id.to_string(),
            questions,
        },
    }
}

/// Build a `QuestionPrompt`.
pub fn qprompt(
    question: &str,
    header: &str,
    multi_select: bool,
    options: Vec<(&str, &str)>,
) -> QuestionPrompt {
    QuestionPrompt {
        question: question.to_string(),
        header: header.to_string(),
        multi_select,
        options: options
            .into_iter()
            .map(|(l, d)| QuestionOption {
                label: l.to_string(),
                description: d.to_string(),
            })
            .collect(),
    }
}

/// A `Usage` value.
pub fn usage(input: u64, output: u64, cache_read: u64, cache_creation: u64, cost: f64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
        cost_usd: cost,
    }
}

/// A mid-turn usage snapshot.
pub fn usage_update(u: Usage, context_pct: Option<f32>) -> AgentEvent {
    AgentEvent::UsageUpdate {
        usage: u,
        context_pct,
    }
}

/// A successful turn completion carrying cumulative usage.
pub fn turn_success(u: Usage) -> AgentEvent {
    AgentEvent::TurnCompleted {
        end_reason: None,
        was_interrupted: false,
        num_turns: 1,
        usage: u,
        per_model: HashMap::new(),
        reply: None,
    }
}

/// A successful turn with a per-model breakdown.
pub fn turn_success_per_model(u: Usage, per_model: HashMap<String, ModelUsage>) -> AgentEvent {
    AgentEvent::TurnCompleted {
        end_reason: None,
        was_interrupted: false,
        num_turns: 1,
        usage: u,
        per_model,
        reply: None,
    }
}

/// A turn completion with an explicit end_reason.
pub fn turn_end(reason: wasit_core::EndReason, u: Usage) -> AgentEvent {
    AgentEvent::TurnCompleted {
        end_reason: Some(reason),
        was_interrupted: false,
        num_turns: 1,
        usage: u,
        per_model: HashMap::new(),
        reply: None,
    }
}

/// A turn completion reporting a user interrupt.
pub fn turn_interrupted(u: Usage) -> AgentEvent {
    AgentEvent::TurnCompleted {
        end_reason: Some(wasit_core::EndReason::Cancelled),
        was_interrupted: true,
        num_turns: 1,
        usage: u,
        per_model: HashMap::new(),
        reply: None,
    }
}

/// A plan (opencode todo list) update.
pub fn plan_update(entries: Vec<PlanEntry>) -> AgentEvent {
    AgentEvent::PlanUpdated { entries }
}

/// A modes update.
pub fn modes_update(available: Vec<SessionMode>, current: Option<String>) -> AgentEvent {
    AgentEvent::ModesUpdated {
        available_modes: available,
        current_mode: current,
    }
}

/// A slash-commands update.
pub fn slash_update(commands: Vec<SlashCommandInfo>) -> AgentEvent {
    AgentEvent::SlashCommandsUpdated { commands }
}

/// Fold a fresh `SessionState` from an init event.
pub fn after_init(init: AgentEvent) -> SessionState {
    reduce(SessionState::initial(), ev(init))
}
