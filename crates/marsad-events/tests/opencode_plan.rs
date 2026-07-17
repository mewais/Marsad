//! opencode_plan.rs — events-level TDD (RED phase) tests for opencode plan-mode.
//!
//! ALL tests in this file MUST FAIL (red phase). They assert that feeding the
//! contracted `system/plan_update` stream-json shape into the events reducer
//! populates `SessionState.plan_entries` — a field that does not yet exist.
//!
//! The strategy: parse the plan_update stream-json line, fold it with `reduce`,
//! then round-trip the resulting `SessionState` through `serde_json::to_value`
//! and assert on `plan_entries`. This compiles cleanly (no reference to the
//! missing Rust field) and fails at runtime with a clear message until the
//! field and reducer arm are implemented.
//!
//! Implementation gate (all must be done before tests go green):
//!   1. `SessionState` gains `pub plan_entries: Vec<PlanEntry>` (or similar)
//!      with `#[serde(default)]`.
//!   2. `marsad_proto::StreamMsg` gains a `PlanUpdate` variant (or the
//!      existing `Unknown` arm routes it) so `parse_stream_line` accepts the
//!      `system/plan_update` shape.
//!   3. `reduce_stream` handles the new variant: replaces `state.plan_entries`
//!      with the incoming entries.

use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_proto::parse_stream_line;

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

fn stream(json: &str) -> Event {
    let msg = parse_stream_line(json)
        .unwrap_or_else(|e| panic!("test JSON failed to parse: {e:?}\njson: {json}"));
    Event::Stream(msg)
}

fn system_init_json() -> &'static str {
    r#"{"type":"system","subtype":"init","session_id":"sess-plan","model":"gpt-4o","cwd":"/tmp/work","tools":[],"mcp_servers":[],"permissionMode":"default"}"#
}

/// Build a plan_update stream-json line (the contracted output shape that
/// acp_translate must emit for session/update{plan} ACP events).
fn plan_update_json(session_id: &str, entries: &[(&str, &str)]) -> String {
    let entries_json: String = entries
        .iter()
        .map(|(content, priority)| {
            format!(r#"{{"content":"{content}","priority":"{priority}"}}"#)
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"type":"system","subtype":"plan_update","session_id":"{session_id}","plan_entries":[{entries_json}]}}"#
    )
}

fn init_state() -> SessionState {
    reduce(SessionState::initial(), stream(system_init_json()))
}

// ---------------------------------------------------------------------------
// Helper: assert plan_entries from a serialized SessionState
// ---------------------------------------------------------------------------

/// Round-trip `state` through JSON, assert the `plan_entries` key exists and
/// return the array for further assertions.
///
/// Panics with a clear RED message if the field is absent (not yet implemented).
fn get_plan_entries(state: &SessionState) -> Vec<serde_json::Value> {
    let json = serde_json::to_value(state).expect("SessionState must serialize");
    json.get("plan_entries")
        .and_then(|v| v.as_array())
        .cloned()
        .expect(
            "SessionState JSON must contain a 'plan_entries' array after plan_update \
             — field missing or null (RED: implement plan_entries on SessionState)",
        )
}

// ---------------------------------------------------------------------------
// ep1: plan_update populates plan_entries on SessionState
// ---------------------------------------------------------------------------

/// Feeding a `system/plan_update` stream event into the events reducer must
/// populate `SessionState.plan_entries` with the delivered entries.
///
/// RED: `plan_entries` field and reducer arm do not yet exist.
#[test]
fn ep1_plan_update_populates_plan_entries() {
    let s0 = init_state();

    let plan_json = plan_update_json(
        "sess-plan",
        &[
            ("Implement the feature", "high"),
            ("Write tests", "medium"),
            ("Update docs", "low"),
        ],
    );
    let s1 = reduce(s0, stream(&plan_json));

    let entries = get_plan_entries(&s1);
    assert_eq!(
        entries.len(),
        3,
        "SessionState must carry 3 plan_entries after plan_update, got: {entries:?}"
    );
    assert_eq!(
        entries[0]
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "Implement the feature",
        "first plan_entry content must match"
    );
    assert_eq!(
        entries[0]
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "high",
        "first plan_entry priority must be 'high'"
    );
    assert_eq!(
        entries[1]
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "medium",
        "second plan_entry priority must be 'medium'"
    );
    assert_eq!(
        entries[2]
            .get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "low",
        "third plan_entry priority must be 'low'"
    );
}

// ---------------------------------------------------------------------------
// ep2: plan_entries are empty before any plan_update
// ---------------------------------------------------------------------------

/// `SessionState.plan_entries` must start empty (no entries until a plan_update
/// arrives). This pins the zero-valued initial state.
///
/// RED: field doesn't exist yet, so serialized JSON will have no `plan_entries`
/// key at all — `.get("plan_entries")` returns None and the assertion fails.
#[test]
fn ep2_plan_entries_empty_before_any_update() {
    let s = init_state();
    let json = serde_json::to_value(&s).expect("serialize");

    // The field must be present and be an empty array (or absent and treated as
    // empty by #[serde(default)]). We check both: if it's absent the test still
    // correctly fails (field not yet added). If it's present but non-empty that
    // is also wrong.
    let entries = json
        .get("plan_entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default(); // absent = treat as empty for this assertion
    assert!(
        entries.is_empty(),
        "plan_entries must be empty before any plan_update, got: {entries:?}"
    );

    // But also assert the FIELD EXISTS in the serialized form (so the UI always
    // has a stable key to read). Without the field the `unwrap_or_default` above
    // silently passes — this assertion pins that the field IS present.
    let has_key = json.get("plan_entries").is_some();
    assert!(
        has_key,
        "SessionState JSON must always include a 'plan_entries' key \
         (even when empty) — field not yet added (RED)"
    );
}

// ---------------------------------------------------------------------------
// ep3: plan_entries are ordered (insertion order preserved)
// ---------------------------------------------------------------------------

/// Plan entries must appear in the order delivered by the ACP event.
/// The reducer must not sort, dedup, or reorder them.
///
/// RED: field/reducer absent.
#[test]
fn ep3_plan_entries_ordered() {
    let s0 = init_state();

    let plan_json = plan_update_json(
        "sess-plan",
        &[
            ("Step A", "high"),
            ("Step B", "low"),
            ("Step C", "medium"),
            ("Step D", "high"),
        ],
    );
    let s1 = reduce(s0, stream(&plan_json));
    let entries = get_plan_entries(&s1);

    assert_eq!(entries.len(), 4, "must preserve all 4 entries");
    let contents: Vec<&str> = entries
        .iter()
        .map(|e| e.get("content").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert_eq!(
        contents,
        vec!["Step A", "Step B", "Step C", "Step D"],
        "plan_entries must be in insertion order"
    );
}

// ---------------------------------------------------------------------------
// ep4: second plan_update REPLACES (not appends) the plan
// ---------------------------------------------------------------------------

/// A second `plan_update` must replace `plan_entries` wholesale — it is a full
/// plan snapshot, not an incremental patch. The reducer must not accumulate or
/// append.
///
/// RED: field/reducer absent.
#[test]
fn ep4_second_plan_update_replaces_first() {
    let s0 = init_state();

    let first_plan = plan_update_json(
        "sess-plan",
        &[
            ("Old step 1", "high"),
            ("Old step 2", "medium"),
            ("Old step 3", "low"),
        ],
    );
    let s1 = reduce(s0, stream(&first_plan));
    let after_first = get_plan_entries(&s1);
    assert_eq!(after_first.len(), 3, "sanity: first plan has 3 entries");

    let second_plan = plan_update_json(
        "sess-plan",
        &[("New step 1", "high"), ("New step 2", "low")],
    );
    let s2 = reduce(s1, stream(&second_plan));
    let after_second = get_plan_entries(&s2);

    assert_eq!(
        after_second.len(),
        2,
        "second plan_update must REPLACE the first: expected 2 entries, got {}: {after_second:?}",
        after_second.len()
    );
    let contents: Vec<&str> = after_second
        .iter()
        .map(|e| e.get("content").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(
        !contents.contains(&"Old step 1"),
        "old entries must be gone after second plan_update, got: {contents:?}"
    );
    assert!(
        contents.contains(&"New step 1"),
        "new entries must be present after second plan_update, got: {contents:?}"
    );
}

// ---------------------------------------------------------------------------
// ep5: plan_update does not change Phase
// ---------------------------------------------------------------------------

/// A `plan_update` is an informational event — it must not change the session
/// phase (Working → Needs, etc.).
///
/// RED: field/reducer absent; even if the phase assertion passes the plan_entries
/// assertion above it will fail.
#[test]
fn ep5_plan_update_does_not_change_phase() {
    let s0 = init_state();
    assert_eq!(s0.phase, Phase::Working, "sanity: Working after init");

    let plan_json = plan_update_json("sess-plan", &[("Do something", "high")]);
    let s1 = reduce(s0, stream(&plan_json));

    // Phase must remain Working (plan_update is informational only).
    assert_eq!(
        s1.phase,
        Phase::Working,
        "plan_update must not change phase from Working"
    );

    // AND plan_entries must be populated (the real red assertion).
    let entries = get_plan_entries(&s1);
    assert_eq!(
        entries.len(),
        1,
        "plan_entries must have 1 entry after plan_update, got: {entries:?}"
    );
}

// ---------------------------------------------------------------------------
// ep6: plan_entries survive a non-plan event (state is not cleared inadvertently)
// ---------------------------------------------------------------------------

/// After a plan_update, feeding an unrelated stream event (e.g. an assistant
/// message) must NOT clear plan_entries.
///
/// RED: field/reducer absent.
#[test]
fn ep6_plan_entries_survive_unrelated_event() {
    let s0 = init_state();

    let plan_json = plan_update_json(
        "sess-plan",
        &[("Persistent step", "high")],
    );
    let s1 = reduce(s0, stream(&plan_json));

    // Feed an assistant message (unrelated to plan).
    let assistant_json = r#"{"type":"assistant","message":{"id":"msg-after-plan","content":[{"type":"text","text":"Thinking…"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let s2 = reduce(s1, stream(assistant_json));

    let entries = get_plan_entries(&s2);
    assert_eq!(
        entries.len(),
        1,
        "plan_entries must survive an unrelated assistant event, got: {entries:?}"
    );
    assert_eq!(
        entries[0]
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "Persistent step",
        "plan entry content must survive unrelated event"
    );
}
