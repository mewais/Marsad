//! modes.rs — agent-reported session modes (available_modes + current_mode).
//!
//! Modes are NEVER a hardcoded Marsad list: opencode reports its full list on the
//! wire (configOptions → `system/modes_update`), and claude — which reports only
//! the CURRENT permissionMode — gets a version-derived list on init plus a
//! current-only update on `system/status`. These tests pin that contract.

use marsad_events::{claude_available_modes, reduce, Event, SessionState};
use marsad_proto::parse_stream_line;

fn stream(json: &str) -> Event {
    Event::Stream(parse_stream_line(json).expect("valid stream-json"))
}

// ── claude version-derived table ────────────────────────────────────────────

#[test]
fn claude_modes_include_the_base_three_always() {
    let ids: Vec<String> = claude_available_modes("2.1.165")
        .into_iter()
        .map(|m| m.id)
        .collect();
    for want in ["default", "acceptEdits", "plan"] {
        assert!(ids.contains(&want.to_string()), "modern claude must expose {want}");
    }
}

#[test]
fn claude_modern_version_includes_bypass_permissions() {
    let ids: Vec<String> = claude_available_modes("2.1.165")
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert!(
        ids.contains(&"bypassPermissions".to_string()),
        "claude 2.x supports full autonomy"
    );
}

#[test]
fn claude_major_zero_drops_bypass_permissions() {
    // The user's point: older claude lacks "full autonomy" — the list is
    // version-aware, not a fixed set.
    let ids: Vec<String> = claude_available_modes("0.9.0")
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert!(!ids.contains(&"bypassPermissions".to_string()));
    assert!(ids.contains(&"plan".to_string()), "base modes still present");
}

// ── claude init → available list + current ──────────────────────────────────

#[test]
fn claude_init_populates_available_modes_and_current() {
    let init = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus","cwd":"/p","tools":[],"permissionMode":"acceptEdits","claude_code_version":"2.1.165"}"#;
    let st = reduce(SessionState::initial(), stream(init));
    assert!(!st.available_modes.is_empty(), "init derives the mode list");
    assert_eq!(
        st.current_mode.as_deref(),
        Some("acceptEdits"),
        "permissionMode is the current mode"
    );
}

// ── claude system/status → live current-mode change ─────────────────────────

#[test]
fn claude_status_updates_current_mode_without_wiping_list() {
    let init = r#"{"type":"system","subtype":"init","session_id":"s","model":"m","cwd":"/p","tools":[],"permissionMode":"default","claude_code_version":"2.1.165"}"#;
    let mut st = reduce(SessionState::initial(), stream(init));
    let list_len = st.available_modes.len();
    assert!(list_len > 0);

    // claude enters plan mode on its own → a system/status carries the new mode.
    // This is a VERBATIM line captured from a real claude 2.1.165 session log.
    let status = r#"{"type":"system","subtype":"status","status":null,"permissionMode":"plan","uuid":"5f9f71b0-0f1f-42fa-a720-862986dda561","session_id":"d44785ed-3206-4e64-b617-14681915728c"}"#;
    st = reduce(st, stream(status));
    assert_eq!(st.current_mode.as_deref(), Some("plan"), "status updates current");
    assert_eq!(
        st.available_modes.len(),
        list_len,
        "a current-only update must NOT wipe the available list"
    );
}

// ── opencode configOptions → full list + current (via modes_update) ─────────

#[test]
fn opencode_modes_update_sets_list_and_current() {
    let mu = r#"{"type":"system","subtype":"modes_update","session_id":"s","available_modes":[{"id":"build","label":"build","description":"default"},{"id":"plan","label":"plan","description":"no edits"}],"current_mode":"build"}"#;
    let st = reduce(SessionState::initial(), stream(mu));
    let ids: Vec<String> = st.available_modes.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, vec!["build", "plan"]);
    assert_eq!(st.current_mode.as_deref(), Some("build"));
}
