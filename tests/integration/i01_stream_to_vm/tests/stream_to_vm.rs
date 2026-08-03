//! Integration test i01_stream_to_vm
//!
//! Constructs the qa scenario as `wasit_core::AgentEvent`s — the same sequence
//! the wasit-claude driver emits for the recorded `fixtures/streams/qa.ndjson`
//! (session_id "59215d34-0f57-4b68-9419-3bb8e27d6fab", model "claude-opus-4.8[1m]",
//! cwd "/work/mewais/Marsad", one assistant delta, result with
//! total_cost_usd=0.796905 / input=2 / cache_creation=42448 / output=13).
//!
//! Wave 1 moved raw stream-json parsing into the wasit drivers; `marsad-events`
//! now folds `AgentEvent`s directly. The fixture file is no longer read here;
//! the qa scenario is fully reconstructed from the documented field values.

use marsad_events::{reduce, Event, Phase, SessionState};
use wasit_core::{AgentEvent, Usage};
use marsad_viewmodel::{
    counts, group_overview, session_detail, AppSnapshot, Filter, SessionView, Tab,
};

// ---------------------------------------------------------------------------
// Local AgentEvent constructor helpers (mirror marsad-events/tests/common/mod.rs)
// ---------------------------------------------------------------------------

fn ev(e: AgentEvent) -> Event {
    Event::Stream(e)
}

fn session_init(session_id: &str, model: &str, cwd: &str) -> AgentEvent {
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

fn assistant_delta(text: &str) -> AgentEvent {
    AgentEvent::AssistantDelta {
        text: text.to_string(),
        message_id: None,
    }
}

fn turn_success(u: Usage) -> AgentEvent {
    AgentEvent::TurnCompleted {
        end_reason: None,
        was_interrupted: false,
        num_turns: 1,
        usage: u,
        per_model: std::collections::HashMap::new(),
        reply: None,
    }
}

fn usage(input: u64, output: u64, cache_read: u64, cache_creation: u64, cost: f64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
        cost_usd: cost,
    }
}

/// Reconstruct the qa scenario session state from AgentEvents — mirrors the
/// recorded fixture: SessionInit → AssistantDelta → TurnCompleted(usage).
fn fold_qa() -> SessionState {
    let mut state = SessionState::initial();
    state = reduce(
        state,
        ev(session_init(
            "59215d34-0f57-4b68-9419-3bb8e27d6fab",
            "claude-opus-4.8[1m]",
            "/work/mewais/Marsad",
        )),
    );
    state = reduce(state, ev(assistant_delta("Here is the answer.")));
    // result/success: input=2, cache_creation=42448, output=13 → 42463 cumulative;
    // total_cost_usd=0.796905.
    state = reduce(state, ev(turn_success(usage(2, 13, 0, 42448, 0.796905))));
    state
}

#[test]
fn fold_qa_stream_into_session_state_and_viewmodel() {
    // -----------------------------------------------------------------------
    // 1. Fold the qa scenario as AgentEvents (replaces the old NDJSON parse loop).
    // -----------------------------------------------------------------------
    let state = fold_qa();

    // -----------------------------------------------------------------------
    // 2. Assert SessionState fields.
    // -----------------------------------------------------------------------

    // Phase must be Done (TurnCompleted with was_interrupted=false).
    assert_eq!(
        state.phase,
        Phase::Done,
        "phase should be Done after success result"
    );

    // Session ID must match the fixture.
    assert_eq!(
        state.session_id.as_deref(),
        Some("59215d34-0f57-4b68-9419-3bb8e27d6fab"),
        "session_id mismatch",
    );

    // Model must be present.
    assert!(
        state.model.is_some(),
        "model should be set after SessionInit"
    );
    let model = state.model.as_deref().unwrap();
    assert!(
        model.contains("claude"),
        "model should contain 'claude', got {model:?}",
    );

    // Tokens must be positive (result.usage: input=2, cache_creation=42448, output=13).
    assert!(
        state.tokens > 0,
        "tokens should be > 0, got {}",
        state.tokens
    );

    // Cost must match the fixture's total_cost_usd=0.796905 within a small epsilon.
    let expected_cost = 0.796905_f64;
    let epsilon = 1e-6_f64;
    assert!(
        (state.cost_usd - expected_cost).abs() < epsilon,
        "cost_usd expected ≈{expected_cost}, got {}",
        state.cost_usd,
    );

    // context_pct must be in (0, 100].
    // Model "claude-opus-4.8[1m]" has a 1M token window; fixture tokens ≈42463.
    assert!(
        state.context_pct > 0.0,
        "context_pct should be > 0, got {}",
        state.context_pct,
    );
    assert!(
        state.context_pct <= 100.0,
        "context_pct should be ≤ 100, got {}",
        state.context_pct,
    );

    // -----------------------------------------------------------------------
    // 3. Build AppSnapshot from the folded state.
    // -----------------------------------------------------------------------
    let session_id = state.session_id.clone().unwrap();
    let sv = SessionView {
        id: session_id.clone(),
        name: "qa-session".to_string(),
        host_id: "local".to_string(),
        project: "Marsad".to_string(),
        branch: "main".to_string(),
        model: state.model.clone().unwrap_or_default(),
        state: state.phase.clone(),
        changed_files: state.changed_files,
        subagents: state.subagents.len() as u32,
        running_subagents: 0,
        cost_usd: state.cost_usd,
        tokens: state.tokens,
        context_pct: state.context_pct,
        gate: state.gate.clone(),
        transcript: vec![],
        changes: vec![],

        model_usage: vec![],
        cost_estimated: false,
        compactions: state.compactions,
        last_compaction: state.last_compaction.clone(),
        rate_limit: state.rate_limit.clone(),
        tools: vec![],
        mcp_servers: vec![],
        slash_commands: vec![],

        driver: "claude".to_string(),
        live: true,
        conn_health: "Connected".to_string(),
        end_reason: state.end_reason.clone(),
        plan_entries: state.plan_entries.clone(),
        current_mode: state.current_mode.clone(),
        available_modes: state.available_modes.clone(),
        shell_running: false,
        subagent_list: vec![],
    };

    let snapshot = AppSnapshot {
        sessions: vec![sv],
        hosts: vec![],
        workspaces: vec![],
        active_ws: None,
        selected_id: Some(session_id.clone()),
        filter: Filter::All,
        search: String::new(),
        tab: Tab::Activity,
    };

    // -----------------------------------------------------------------------
    // 4. Assert viewmodel outputs.
    // -----------------------------------------------------------------------

    // counts(): 1 total session, 0 needs, 0 errors, 0 working (it is Done).
    let c = counts(&snapshot);
    assert_eq!(c.total, 1, "counts.total should be 1");
    assert_eq!(c.needs, 0, "counts.needs should be 0");
    assert_eq!(c.errors, 0, "counts.errors should be 0");
    assert_eq!(c.working, 0, "counts.working should be 0");
    // Cost string starts with '$' and contains the total.
    assert!(
        c.cost.starts_with('$'),
        "counts.cost should start with '$', got {:?}",
        c.cost
    );

    // group_overview(): the Done session must land in the idle group.
    let groups = group_overview(&snapshot);
    assert!(
        groups.attention.is_empty(),
        "attention should be empty (no needs/error), got {:?}",
        groups.attention,
    );
    assert!(
        groups.active.is_empty(),
        "active should be empty (not working/starting), got {:?}",
        groups.active,
    );
    assert_eq!(
        groups.idle.len(),
        1,
        "idle should contain exactly 1 Done session"
    );
    assert_eq!(
        groups.idle[0].id, session_id,
        "idle[0].id should match our session",
    );
    assert_eq!(
        groups.idle[0].state,
        Phase::Done,
        "idle[0].state should be Done",
    );

    // session_detail(): selected_id is set, so we get Some.
    let detail = session_detail(&snapshot)
        .expect("session_detail should return Some when selected_id is set");
    assert!(!detail.model.is_empty(), "detail.model should not be empty");
    assert!(
        detail.cost_label.starts_with('$'),
        "detail.cost_label should start with '$', got {:?}",
        detail.cost_label,
    );
    // The Done session has context_pct well below 75, so ctx_warn should be false.
    assert!(
        !detail.ctx_warn,
        "ctx_warn should be false for a low context_pct session"
    );
    assert_eq!(
        detail.context_pct, state.context_pct,
        "detail.context_pct should match state"
    );
}
