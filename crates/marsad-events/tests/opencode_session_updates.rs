//! opencode_session_updates.rs — events-level TDD (RED phase) tests for
//! opencode mode_update and slash_update stream events.
//!
//! ALL tests in this file MUST FAIL (red phase). They assert that feeding the
//! contracted `system/mode_update` and `system/slash_update` stream-json shapes
//! into the events reducer populates the corresponding `SessionState` fields —
//! fields and reducer arms that do not yet exist.
//!
//! Strategy (mirroring opencode_plan.rs):
//! - Parse the stream-json line with `parse_stream_line`.
//! - Fold it with `reduce`.
//! - Round-trip the resulting `SessionState` through `serde_json::to_value`.
//! - Assert the expected JSON key and value.
//!
//! This compiles cleanly (no reference to missing Rust fields by name) and
//! fails at runtime with a clear message until the fields and reducer arms are
//! implemented.
//!
//! Implementation gate (all must be done before these tests go green):
//!   mode_update tests:
//!     1. `SessionState` gains `pub current_mode: Option<String>` with
//!        `#[serde(default)]`.
//!     2. `marsad_proto::parse_stream_line` recognises the `system/mode_update`
//!        subtype (currently falls through to `StreamMsg::Unknown`).
//!     3. `reduce_stream` handles the new variant: sets
//!        `state.current_mode = Some(current_mode_id)`.
//!
//!   slash_update tests:
//!     1. `marsad_proto::parse_stream_line` recognises the `system/slash_update`
//!        subtype (currently falls through to `StreamMsg::Unknown`).
//!     2. `reduce_stream` handles the new variant: REPLACES
//!        `state.slash_commands` with the incoming list (mid-session update path,
//!        distinct from the `system/init` seeding that already exists).
//!     Note: `slash_commands` already exists on `SessionState`; only the
//!     reducer arm is missing for the mid-session update path.

use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_proto::parse_stream_line;

// ---------------------------------------------------------------------------
// JSON helpers (same pattern as opencode_plan.rs)
// ---------------------------------------------------------------------------

fn stream(json: &str) -> Event {
    let msg = parse_stream_line(json)
        .unwrap_or_else(|e| panic!("test JSON failed to parse: {e:?}\njson: {json}"));
    Event::Stream(msg)
}

/// system/init that seeds a Working session with a known session_id.
/// Uses a different session_id from opencode_plan.rs to avoid confusion.
fn system_init_json() -> &'static str {
    r#"{"type":"system","subtype":"init","session_id":"sess-updates","model":"gpt-4o","cwd":"/tmp/proj","tools":[],"mcp_servers":[],"permissionMode":"default"}"#
}

/// Build a `system/mode_update` stream-json line (the canonical shape that
/// `acp_translate` emits for ACP `current_mode_update` notifications).
fn mode_update_json(session_id: &str, current_mode: &str) -> String {
    format!(
        r#"{{"type":"system","subtype":"mode_update","session_id":"{session_id}","current_mode":"{current_mode}"}}"#
    )
}

/// Build a `system/slash_update` stream-json line (the canonical shape that
/// `acp_translate` emits for ACP `available_commands_update` notifications).
/// Each command is `(name, description, argument_hint)`.
fn slash_update_json(session_id: &str, commands: &[(&str, &str, &str)]) -> String {
    let cmds_json: String = commands
        .iter()
        .map(|(name, description, argument_hint)| {
            format!(
                r#"{{"name":"{name}","description":"{description}","argumentHint":"{argument_hint}"}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"type":"system","subtype":"slash_update","session_id":"{session_id}","slash_commands":[{cmds_json}]}}"#
    )
}

fn init_state() -> SessionState {
    reduce(SessionState::initial(), stream(system_init_json()))
}

// ---------------------------------------------------------------------------
// Helper: assert current_mode from a serialized SessionState
// ---------------------------------------------------------------------------

/// Round-trip `state` through JSON, assert that `current_mode` equals
/// `expected_id`. Panics with a clear RED message when the field is absent
/// (not yet implemented) or when the value does not match.
fn assert_current_mode(state: &SessionState, expected_id: &str) {
    let json = serde_json::to_value(state).expect("SessionState must serialize");
    let actual = json
        .get("current_mode")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            panic!(
                "SessionState JSON must contain a 'current_mode' string after mode_update \
                 — field missing or null (RED: implement current_mode on SessionState + reducer arm)"
            )
        });
    assert_eq!(
        actual, expected_id,
        "current_mode must equal '{expected_id}', got '{actual}'"
    );
}

/// Assert that `current_mode` is absent (null / missing) in the serialized JSON.
/// Used to pin the zero-value before any mode_update arrives.
fn assert_current_mode_absent(state: &SessionState) {
    let json = serde_json::to_value(state).expect("SessionState must serialize");
    // Acceptable forms: key absent, key present with null, or key present with
    // empty string — anything that is NOT a non-empty string is "absent" here.
    // The hard assertion is that the field is present in serialized form even
    // when unset (with null/`None`) — see eu2 below.
    let is_absent = json
        .get("current_mode")
        .map(|v| v.is_null())
        .unwrap_or(true); // missing key is also "absent"
    assert!(
        is_absent,
        "current_mode must be null/absent before any mode_update, got: {:?}",
        json.get("current_mode")
    );
}

// ---------------------------------------------------------------------------
// Helper: get slash_commands array from a serialized SessionState
// ---------------------------------------------------------------------------

/// Round-trip `state` through JSON, assert that `slash_commands` key exists,
/// and return the array. Panics if the key is missing.
fn get_slash_commands(state: &SessionState) -> Vec<serde_json::Value> {
    let json = serde_json::to_value(state).expect("SessionState must serialize");
    json.get("slash_commands")
        .and_then(|v| v.as_array())
        .cloned()
        .expect(
            "SessionState JSON must contain a 'slash_commands' array \
             — field missing or null (RED: reducer arm for slash_update missing)",
        )
}

// ===========================================================================
// mode_update tests
// ===========================================================================

// ---------------------------------------------------------------------------
// eu1: mode_update populates current_mode on SessionState
// ---------------------------------------------------------------------------

/// Feeding a `system/mode_update` stream event into the events reducer must
/// set `SessionState.current_mode` to the delivered id.
///
/// RED: `current_mode` field and reducer arm do not yet exist.
#[test]
fn eu1_mode_update_sets_current_mode() {
    let s0 = init_state();

    let mode_json = mode_update_json("sess-updates", "plan");
    let s1 = reduce(s0, stream(&mode_json));

    assert_current_mode(&s1, "plan");
}

// ---------------------------------------------------------------------------
// eu2: current_mode is absent/null before any mode_update
// ---------------------------------------------------------------------------

/// `SessionState.current_mode` must be null/absent before any `mode_update`
/// arrives. This pins the zero-value initial state.
///
/// RED: field absent → key missing from serialized JSON; `unwrap_or(true)` in
/// `assert_current_mode_absent` would pass the `is_absent` guard, BUT the
/// second assertion below (field present in JSON) pins that the field IS added
/// with a `#[serde(default)]` null-compatible type.
#[test]
fn eu2_current_mode_absent_before_any_update() {
    let s = init_state();
    let json = serde_json::to_value(&s).expect("serialize");

    // Field must be present in serialized form so the UI has a stable key.
    // When the field does not exist yet, `json.get("current_mode")` is None
    // and this assertion catches that.
    let has_key = json.get("current_mode").is_some();
    assert!(
        has_key,
        "SessionState JSON must always include a 'current_mode' key \
         (even when null) — field not yet added to SessionState (RED)"
    );

    // The value itself must be null / None before any update.
    assert_current_mode_absent(&s);
}

// ---------------------------------------------------------------------------
// eu3: second mode_update REPLACES the current_mode (not appended)
// ---------------------------------------------------------------------------

/// A second `mode_update` must replace `current_mode` — there is only ever one
/// current mode. The reducer must not accumulate or ignore later events.
///
/// RED: field/reducer absent.
#[test]
fn eu3_second_mode_update_replaces_first() {
    let s0 = init_state();

    let first_mode = mode_update_json("sess-updates", "plan");
    let s1 = reduce(s0, stream(&first_mode));
    assert_current_mode(&s1, "plan");

    let second_mode = mode_update_json("sess-updates", "build");
    let s2 = reduce(s1, stream(&second_mode));

    // Must be "build", not "plan".
    assert_current_mode(&s2, "build");

    // Verify that "plan" is indeed gone.
    let json = serde_json::to_value(&s2).expect("serialize");
    let actual = json
        .get("current_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_ne!(
        actual, "plan",
        "second mode_update must replace 'plan' with 'build', got '{actual}'"
    );
}

// ---------------------------------------------------------------------------
// eu4: an unrelated event does NOT change current_mode
// ---------------------------------------------------------------------------

/// After a `mode_update`, feeding an unrelated stream event (assistant message)
/// must NOT clear or change `current_mode`.
///
/// RED: field/reducer absent.
#[test]
fn eu4_unrelated_event_does_not_change_current_mode() {
    let s0 = init_state();

    let mode_json = mode_update_json("sess-updates", "build");
    let s1 = reduce(s0, stream(&mode_json));
    assert_current_mode(&s1, "build");

    // Feed an assistant message (unrelated to mode).
    let assistant_json = r#"{"type":"assistant","message":{"id":"msg-mode-test","content":[{"type":"text","text":"working on it"}],"usage":{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let s2 = reduce(s1, stream(assistant_json));

    // current_mode must still be "build".
    assert_current_mode(&s2, "build");
}

// ---------------------------------------------------------------------------
// eu5: mode_update does not change Phase
// ---------------------------------------------------------------------------

/// A `mode_update` is informational — it must not change the session phase.
///
/// RED: field/reducer absent; even if the phase check passes, the
/// current_mode assertion will catch the missing implementation.
#[test]
fn eu5_mode_update_does_not_change_phase() {
    let s0 = init_state();
    assert_eq!(s0.phase, Phase::Working, "sanity: Working after init");

    let mode_json = mode_update_json("sess-updates", "plan");
    let s1 = reduce(s0, stream(&mode_json));

    assert_eq!(
        s1.phase,
        Phase::Working,
        "mode_update must not change phase from Working"
    );

    // AND current_mode must be populated (the real red assertion).
    assert_current_mode(&s1, "plan");
}

// ===========================================================================
// slash_update tests
// ===========================================================================

// ---------------------------------------------------------------------------
// es1: slash_update mid-session replaces slash_commands
// ---------------------------------------------------------------------------

/// Feeding a `system/slash_update` stream event (the mid-session update path,
/// as opposed to `system/init` seeding) into the reducer must REPLACE
/// `SessionState.slash_commands` with the delivered list.
///
/// RED: the reducer arm for `subtype == "slash_update"` does not yet exist.
/// The event currently parses as `StreamMsg::Unknown` and is a no-op.
/// After init, `slash_commands` is empty (the init JSON above has none).
/// The test then feeds a `slash_update` and asserts the new commands land.
#[test]
fn es1_slash_update_replaces_slash_commands() {
    let s0 = init_state();

    // Confirm slash_commands is empty after init (the init JSON above has none).
    let before = get_slash_commands(&s0);
    assert!(
        before.is_empty(),
        "slash_commands must be empty after init with no commands, got: {before:?}"
    );

    let slash_json = slash_update_json(
        "sess-updates",
        &[
            ("/build", "Build the project", ""),
            ("/test", "Run tests", "[pattern]"),
            ("/clear", "Clear context", ""),
        ],
    );
    let s1 = reduce(s0, stream(&slash_json));

    let after = get_slash_commands(&s1);
    assert_eq!(
        after.len(),
        3,
        "slash_update must deliver 3 slash_commands, got: {after:?}"
    );
    assert_eq!(
        after[0].get("name").and_then(|v| v.as_str()).unwrap_or(""),
        "/build",
        "first slash command name must be '/build'"
    );
    assert_eq!(
        after[1]
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "Run tests",
        "second slash command description must be 'Run tests'"
    );
    assert_eq!(
        after[2].get("name").and_then(|v| v.as_str()).unwrap_or(""),
        "/clear",
        "third slash command name must be '/clear'"
    );
}

// ---------------------------------------------------------------------------
// es2: second slash_update REPLACES (not appends) the command list
// ---------------------------------------------------------------------------

/// A second `slash_update` must replace `slash_commands` wholesale — it is a
/// full catalog snapshot, not an incremental patch. The reducer must not
/// accumulate or merge.
///
/// RED: reducer arm absent (both slash_updates hit Unknown and are no-ops).
#[test]
fn es2_second_slash_update_replaces_first() {
    let s0 = init_state();

    let first_slash = slash_update_json(
        "sess-updates",
        &[
            ("/old-cmd-1", "Old command one", ""),
            ("/old-cmd-2", "Old command two", "[args]"),
        ],
    );
    let s1 = reduce(s0, stream(&first_slash));
    let after_first = get_slash_commands(&s1);
    assert_eq!(after_first.len(), 2, "sanity: first slash_update delivers 2 commands");

    let second_slash = slash_update_json(
        "sess-updates",
        &[("/new-cmd", "New command", "")],
    );
    let s2 = reduce(s1, stream(&second_slash));

    let after_second = get_slash_commands(&s2);
    assert_eq!(
        after_second.len(),
        1,
        "second slash_update must REPLACE the first: expected 1 command, got {}: {after_second:?}",
        after_second.len()
    );

    let names: Vec<&str> = after_second
        .iter()
        .map(|c| c.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(
        !names.contains(&"/old-cmd-1"),
        "old commands must be gone after second slash_update, got: {names:?}"
    );
    assert!(
        names.contains(&"/new-cmd"),
        "new command must be present after second slash_update, got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// es3: slash_update carries name, description, argument_hint
// ---------------------------------------------------------------------------

/// Each entry in a `slash_update` must carry `name`, `description`, and
/// `argumentHint` (mapped to `argument_hint` in the Rust struct / `argumentHint`
/// on the wire per `SlashCommandInfo`).
///
/// RED: reducer arm absent.
#[test]
fn es3_slash_update_carries_all_fields() {
    let s0 = init_state();

    let slash_json = slash_update_json(
        "sess-updates",
        &[("/deploy", "Deploy to production", "<env>")],
    );
    let s1 = reduce(s0, stream(&slash_json));

    let cmds = get_slash_commands(&s1);
    assert_eq!(cmds.len(), 1, "must have exactly 1 command");

    let cmd = &cmds[0];
    assert_eq!(
        cmd.get("name").and_then(|v| v.as_str()).unwrap_or(""),
        "/deploy",
        "name must be '/deploy'"
    );
    assert_eq!(
        cmd.get("description").and_then(|v| v.as_str()).unwrap_or(""),
        "Deploy to production",
        "description must match"
    );
    // argumentHint is the wire key (camelCase, per SlashCommandInfo's serde rename)
    // — assert both the camelCase key and the snake_case alias if present.
    let hint = cmd
        .get("argumentHint")
        .or_else(|| cmd.get("argument_hint"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(hint, "<env>", "argumentHint must be '<env>'");
}

// ---------------------------------------------------------------------------
// es4: slash_update does not change Phase
// ---------------------------------------------------------------------------

/// A `slash_update` is informational — it must not change the session phase.
///
/// RED: reducer arm absent.
#[test]
fn es4_slash_update_does_not_change_phase() {
    let s0 = init_state();
    assert_eq!(s0.phase, Phase::Working, "sanity: Working after init");

    let slash_json = slash_update_json("sess-updates", &[("/help", "Show help", "")]);
    let s1 = reduce(s0, stream(&slash_json));

    assert_eq!(
        s1.phase,
        Phase::Working,
        "slash_update must not change phase from Working"
    );

    // AND slash_commands must be populated (the real red assertion).
    let cmds = get_slash_commands(&s1);
    assert_eq!(
        cmds.len(),
        1,
        "slash_commands must have 1 entry after slash_update, got: {cmds:?}"
    );
}

// ---------------------------------------------------------------------------
// es5: slash_commands survive an unrelated event (not inadvertently cleared)
// ---------------------------------------------------------------------------

/// After a `slash_update`, an unrelated stream event must NOT clear
/// `slash_commands`.
///
/// RED: reducer arm absent (slash_update is a no-op; slash_commands starts
/// empty; the assertion below checks it is non-empty after the update, which
/// fails when the reducer arm is missing).
#[test]
fn es5_slash_commands_survive_unrelated_event() {
    let s0 = init_state();

    let slash_json = slash_update_json(
        "sess-updates",
        &[("/persist", "Should survive", "")],
    );
    let s1 = reduce(s0, stream(&slash_json));

    // Feed an assistant message (unrelated).
    let assistant_json = r#"{"type":"assistant","message":{"id":"msg-slash-test","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":4,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let s2 = reduce(s1, stream(assistant_json));

    let cmds = get_slash_commands(&s2);
    assert_eq!(
        cmds.len(),
        1,
        "slash_commands must survive an unrelated assistant event, got: {cmds:?}"
    );
    assert_eq!(
        cmds[0].get("name").and_then(|v| v.as_str()).unwrap_or(""),
        "/persist",
        "slash command name must survive unrelated event"
    );
}
