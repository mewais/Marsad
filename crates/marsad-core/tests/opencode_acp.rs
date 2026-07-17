//! opencode_acp.rs — core/reduce-boundary TDD (RED phase) tests for the
//! opencode ACP backend.
//!
//! ALL tests in this file MUST FAIL (red phase). They assert genuinely-new,
//! not-yet-implemented behavior — specifically plan-mode surfacing — that
//! requires fields and reducer arms that do not yet exist.
//!
//! Removed (green → worthless for TDD):
//!   oatp1 – driver field round-trip (driver field already implemented)
//!   oatp2 – ACP gate surfaces Needs + board attention (existing gate path)
//!   oatp3 – answer routes AnswerGate IPC (existing answer_gate path)
//!   oatp5 – compaction bumps counter (existing compact_boundary path)
//!   oatp6 – full lifecycle (existing reduce pipeline)
//!   oatp7 – unknown update tolerated (existing Unknown no-op)
//!
//! Kept + expanded (genuinely red):
//!   oatp4 – plan update surfaces plan_entries on SessionView (field/reducer absent)
//!
//! The JSON round-trip strategy (serde_json::to_value) is used throughout so
//! that asserting on a not-yet-existing field compiles fine and fails at runtime
//! with a clear message — never a compile error.

use marsad_core::{Core, CoreConfig, LaunchSpec, Posture, VmUpdate};
use marsad_events::Phase;
use marsad_store::MemoryBackend;
use marsad_transport::LoopbackFake;

// ---------------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------------

fn make_core_with_peer() -> (Core, LoopbackFake) {
    let (transport_a, transport_b) = LoopbackFake::pair();
    let cfg = CoreConfig {
        agent_endpoint: "loopback://test".to_string(),
        data_root: "/tmp/marsad-opencode-acp-test".to_string(),
    };
    let core = Core::with_deps(
        cfg,
        Box::new(transport_a),
        Box::new(marsad_core::RealGit),
        Box::new(marsad_core::RealTranscript),
        Box::new(MemoryBackend::new()),
    )
    .expect("Core::with_deps");
    (core, transport_b)
}

fn launch_opencode(core: &mut Core) -> String {
    core.launch_session(LaunchSpec {
        project: "/tmp/opencode-project".to_string(),
        host_id: "local".to_string(),
        host_target: None,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        model: "gpt-4o".to_string(),
        posture: Posture::Ask,
        first_prompt: None,
        mode: Default::default(),
        name: None,
        disallowed_tools: vec![],
        wasl: false,
        driver: "opencode".to_string(),
    })
    .expect("launch_session opencode")
}

fn agent_event_system_init(session_id: &str, model: &str) -> String {
    format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"init","session_id":"{session_id}","model":"{model}","cwd":"/tmp/opencode-project","tools":[],"mcp_servers":[],"permissionMode":"default"}}}}"#
    )
}

/// Extract the last `SessionView` from a `VmUpdate` list.
fn last_session_view(updates: &[VmUpdate]) -> marsad_viewmodel::SessionView {
    updates
        .iter()
        .rev()
        .find_map(|u| match u {
            VmUpdate::Session(sv) => Some(sv.clone()),
            _ => None,
        })
        .expect("expected a VmUpdate::Session in the update list")
}

// ---------------------------------------------------------------------------
// Test 4a: Plan update surfaces plan_entries on the session view (RED)
// ---------------------------------------------------------------------------

/// A `session/update { type:"plan", plan:{ entries:[...] } }` translated by
/// `acp_translate` into the contracted stream-json payload must surface plan
/// entries on the `SessionView`.
///
/// Per §5, plan mode entries are a live plan/todo list rendered from
/// `session/update{plan}`. The session view must expose them in a
/// `plan_entries` field.
///
/// This test WILL FAIL until:
///   1. `SessionState` / `SessionView` gain a `plan_entries` field (Vec), AND
///   2. the reducer handles the contracted plan-update stream-json shape.
///
/// Uses JSON round-trip so the missing field causes a runtime failure (not a
/// compile error).
#[test]
fn oatp4a_plan_update_surfaces_plan_entries_on_session_view() {
    let (mut core, _peer) = make_core_with_peer();
    let session_id = launch_opencode(&mut core);

    let init_line = agent_event_system_init(&session_id, "gpt-4o");
    core.on_agent_line(&init_line).expect("init");

    // The contracted plan-update stream-json shape that acp_translate must emit
    // for a session/update{plan} ACP event (§5).
    let plan_update_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"plan_update","session_id":"{session_id}","plan_entries":[{{"content":"Implement the feature","priority":"high"}},{{"content":"Write tests","priority":"medium"}},{{"content":"Update docs","priority":"low"}}]}}}}"#
    );
    let updates = core
        .on_agent_line(&plan_update_line)
        .expect("on_agent_line plan_update must succeed without error");

    // Round-trip through JSON to assert on the not-yet-existing field without
    // compile errors. Fails at runtime until the field is implemented.
    let sv = last_session_view(&updates);
    let sv_json = serde_json::to_value(&sv).expect("SessionView must serialize");

    let plan_entries = sv_json
        .get("plan_entries")
        .and_then(|v| v.as_array())
        .expect(
            "SessionView JSON must contain a 'plan_entries' array after plan_update \
             — field missing or null (RED: implement plan_entries on SessionView)",
        );

    assert_eq!(
        plan_entries.len(),
        3,
        "session view must expose 3 plan_entries from the plan update, got: {plan_entries:?}"
    );
    assert!(
        plan_entries.iter().any(|e| e
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("Implement")),
        "plan_entries must include the 'Implement the feature' entry"
    );
    assert!(
        plan_entries
            .iter()
            .any(|e| e.get("priority").and_then(|v| v.as_str()) == Some("high")),
        "plan_entries must include a high-priority entry"
    );
    assert!(
        plan_entries
            .iter()
            .any(|e| e.get("priority").and_then(|v| v.as_str()) == Some("medium")),
        "plan_entries must include a medium-priority entry"
    );
    assert!(
        plan_entries
            .iter()
            .any(|e| e.get("priority").and_then(|v| v.as_str()) == Some("low")),
        "plan_entries must include a low-priority entry"
    );
}

// ---------------------------------------------------------------------------
// Test 4b: Plan entries are ordered (RED)
// ---------------------------------------------------------------------------

/// Plan entries must appear in the order the ACP event delivers them.
/// Content and priority must round-trip in insertion order.
///
/// RED: `plan_entries` field doesn't exist yet on `SessionView`.
#[test]
fn oatp4b_plan_entries_are_ordered() {
    let (mut core, _peer) = make_core_with_peer();
    let session_id = launch_opencode(&mut core);

    let init_line = agent_event_system_init(&session_id, "gpt-4o");
    core.on_agent_line(&init_line).expect("init");

    // Feed 5-entry plan: priority sequence is high, low, medium, high, low.
    let plan_update_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"plan_update","session_id":"{session_id}","plan_entries":[{{"content":"Step A","priority":"high"}},{{"content":"Step B","priority":"low"}},{{"content":"Step C","priority":"medium"}},{{"content":"Step D","priority":"high"}},{{"content":"Step E","priority":"low"}}]}}}}"#
    );
    let updates = core
        .on_agent_line(&plan_update_line)
        .expect("plan_update must succeed");

    let sv = last_session_view(&updates);
    let sv_json = serde_json::to_value(&sv).expect("serialize");

    let plan_entries = sv_json
        .get("plan_entries")
        .and_then(|v| v.as_array())
        .expect("plan_entries must exist on SessionView (RED: field not yet implemented)");

    assert_eq!(plan_entries.len(), 5, "must preserve all 5 entries in order");

    // Check order preservation by content.
    let contents: Vec<&str> = plan_entries
        .iter()
        .map(|e| e.get("content").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert_eq!(
        contents,
        vec!["Step A", "Step B", "Step C", "Step D", "Step E"],
        "plan_entries must appear in insertion order"
    );

    // Check priorities in order.
    let priorities: Vec<&str> = plan_entries
        .iter()
        .map(|e| e.get("priority").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert_eq!(
        priorities,
        vec!["high", "low", "medium", "high", "low"],
        "priorities must be preserved in insertion order"
    );
}

// ---------------------------------------------------------------------------
// Test 4c: Second plan update REPLACES the first (RED)
// ---------------------------------------------------------------------------

/// A second `plan_update` must REPLACE the previous plan_entries, not append.
/// The session view must reflect only the latest plan.
///
/// RED: field/reducer absent.
#[test]
fn oatp4c_second_plan_update_replaces_first() {
    let (mut core, _peer) = make_core_with_peer();
    let session_id = launch_opencode(&mut core);

    let init_line = agent_event_system_init(&session_id, "gpt-4o");
    core.on_agent_line(&init_line).expect("init");

    // First plan: 3 entries.
    let first_plan = format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"plan_update","session_id":"{session_id}","plan_entries":[{{"content":"Old step 1","priority":"high"}},{{"content":"Old step 2","priority":"medium"}},{{"content":"Old step 3","priority":"low"}}]}}}}"#
    );
    core.on_agent_line(&first_plan).expect("first plan_update");

    // Second plan: 2 new entries (completely different content).
    let second_plan = format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"plan_update","session_id":"{session_id}","plan_entries":[{{"content":"New step 1","priority":"high"}},{{"content":"New step 2","priority":"low"}}]}}}}"#
    );
    let updates = core
        .on_agent_line(&second_plan)
        .expect("second plan_update");

    let sv = last_session_view(&updates);
    let sv_json = serde_json::to_value(&sv).expect("serialize");

    let plan_entries = sv_json
        .get("plan_entries")
        .and_then(|v| v.as_array())
        .expect("plan_entries must exist (RED: field not yet implemented)");

    assert_eq!(
        plan_entries.len(),
        2,
        "second plan_update must REPLACE first: expected 2 entries, got {}: {plan_entries:?}",
        plan_entries.len()
    );

    // Entries must be the NEW ones, not the old ones.
    let contents: Vec<&str> = plan_entries
        .iter()
        .map(|e| e.get("content").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(
        !contents.contains(&"Old step 1"),
        "old plan entries must be gone after second plan_update, got: {contents:?}"
    );
    assert!(
        contents.contains(&"New step 1"),
        "new plan entries must appear after second plan_update, got: {contents:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 4d: Plan entries visible in snapshot after plan_update (RED)
// ---------------------------------------------------------------------------

/// After a plan_update the plan_entries must be visible in `core.snapshot()`
/// (not just in the immediate VmUpdate stream). This pins that the reducer
/// persists state correctly across a snapshot boundary.
///
/// RED: field/reducer absent.
#[test]
fn oatp4d_plan_entries_visible_in_snapshot() {
    let (mut core, _peer) = make_core_with_peer();
    let session_id = launch_opencode(&mut core);

    let init_line = agent_event_system_init(&session_id, "gpt-4o");
    core.on_agent_line(&init_line).expect("init");

    let plan_update_line = format!(
        r#"{{"type":"AgentEvent","session_id":"{session_id}","event":{{"type":"system","subtype":"plan_update","session_id":"{session_id}","plan_entries":[{{"content":"Snapshot entry","priority":"high"}}]}}}}"#
    );
    core.on_agent_line(&plan_update_line)
        .expect("plan_update must succeed");

    // Pull from snapshot (not from the immediate update list).
    let snap = core.snapshot();
    let sv = snap
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .expect("session must exist in snapshot");

    let sv_json = serde_json::to_value(sv).expect("serialize");
    let plan_entries = sv_json
        .get("plan_entries")
        .and_then(|v| v.as_array())
        .expect("plan_entries must be visible in core.snapshot() (RED: field not yet implemented)");

    assert_eq!(
        plan_entries.len(),
        1,
        "snapshot must carry 1 plan entry, got: {plan_entries:?}"
    );
    assert_eq!(
        plan_entries[0]
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "Snapshot entry",
        "plan entry content must round-trip through snapshot"
    );
}
