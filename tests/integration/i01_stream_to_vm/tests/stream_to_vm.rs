//! Integration test i01_stream_to_vm
//!
//! Folds the recorded real Claude stream (`fixtures/streams/qa.ndjson`) through
//! the proto → events → viewmodel pipeline and asserts the derived state is
//! correct end-to-end.

use marsad_events::{Event, Phase, SessionState};
use marsad_proto::parse_stream_line;
use marsad_viewmodel::{
    counts, group_overview, session_detail, AppSnapshot, Filter, SessionView, Tab,
};

/// Absolute path to the fixture, resolved via CARGO_MANIFEST_DIR at compile time.
/// CARGO_MANIFEST_DIR for this crate is tests/integration/i01_stream_to_vm/
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/streams/qa.ndjson",
);

#[test]
fn fold_qa_stream_into_session_state_and_viewmodel() {
    // -----------------------------------------------------------------------
    // 1. Parse + fold every line of the fixture.
    // -----------------------------------------------------------------------
    let raw = std::fs::read_to_string(FIXTURE)
        .unwrap_or_else(|e| panic!("cannot read fixture {FIXTURE}: {e}"));

    let mut state = SessionState::initial();
    assert_eq!(state.phase, Phase::Starting);

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg = parse_stream_line(line)
            .unwrap_or_else(|e| panic!("parse_stream_line failed on line {line:?}: {e}"));
        state = marsad_events::reduce(state, Event::Stream(msg));
    }

    // -----------------------------------------------------------------------
    // 2. Assert SessionState fields.
    // -----------------------------------------------------------------------

    // Phase must be Done (result subtype=success, is_error=false).
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
        "model should be set after SystemInit"
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
        cost_usd: state.cost_usd,
        tokens: state.tokens,
        context_pct: state.context_pct,
        gate: state.gate.clone(),
        transcript: vec![],
        changes: vec![],

        model_usage: vec![],
            tools: vec![],
            mcp_servers: vec![],
            slash_commands: vec![],
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
