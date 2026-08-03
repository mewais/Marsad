//! modes.rs — agent-reported session modes (available_modes + current_mode).
//!
//! Modes are NEVER a hardcoded Marsad list. Wave 1: the wasit driver computes the
//! available-modes list (version-derived for claude, configOptions for opencode)
//! and puts it on `SessionInit.available_modes` / `ModesUpdated` directly — the
//! reducer uses it verbatim and no longer re-derives at init. `claude_available_modes`
//! stays a public helper for the pre-launch capability-discovery path.

mod common;
use common::*;

use marsad_events::{claude_available_modes, reduce, SessionState};
use wasit_core::SessionMode;

// ── claude version-derived table (still public for capability discovery) ─────

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
    assert!(ids.contains(&"bypassPermissions".to_string()));
}

#[test]
fn claude_major_zero_drops_bypass_permissions() {
    let ids: Vec<String> = claude_available_modes("0.9.0")
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert!(!ids.contains(&"bypassPermissions".to_string()));
    assert!(ids.contains(&"plan".to_string()), "base modes still present");
}

// ── SessionInit carries the driver-computed available list + current ─────────

#[test]
fn init_uses_driver_provided_modes_and_current() {
    // The wasit driver already derived the list from the version; the reducer
    // uses `available_modes` DIRECTLY (does not re-derive) and sets current_mode.
    let modes = claude_available_modes("2.1.165");
    let init = session_init_versioned("claude-opus", "acceptEdits", "2.1.165", modes);
    let st = after_init(init);
    assert!(!st.available_modes.is_empty(), "init carries the mode list");
    assert_eq!(
        st.current_mode.as_deref(),
        Some("acceptEdits"),
        "permissionMode is the current mode"
    );
}

#[test]
fn init_with_empty_modes_leaves_list_untouched() {
    // codex-style init: no modes reported. The reducer must not wipe a previously
    // learned list.
    let modes = vec![SessionMode {
        id: "build".to_string(),
        label: "build".to_string(),
        description: String::new(),
    }];
    let mut st = after_init(modes_update_init(modes.clone()));
    assert_eq!(st.available_modes.len(), 1);
    // A later init with no modes must not clear it.
    st = reduce(st, ev(session_init("s", "m", "/p")));
    assert_eq!(st.available_modes.len(), 1, "empty-modes init must not wipe the list");
}

// Helper: a session already carrying modes (feed a modes_update after a plain init).
fn modes_update_init(modes: Vec<SessionMode>) -> wasit_core::AgentEvent {
    // Return an init that already carries the modes (single event convenience).
    wasit_core::AgentEvent::SessionInit {
        native_session_id: Some("s".to_string()),
        model: Some("m".to_string()),
        cwd: Some("/p".to_string()),
        tools: vec![],
        mcp_servers: vec![],
        slash_commands: vec![],
        permission_mode: String::new(),
        claude_code_version: String::new(),
        available_modes: modes,
        current_mode: String::new(),
    }
}

// ── ModesUpdated → live current-mode change without wiping the list ──────────

#[test]
fn modes_update_current_only_does_not_wipe_list() {
    let modes = claude_available_modes("2.1.165");
    let init = session_init_versioned("m", "default", "2.1.165", modes);
    let mut st = after_init(init);
    let list_len = st.available_modes.len();
    assert!(list_len > 0);

    // A current-only update (empty available_modes) must not wipe the list.
    st = reduce(st, ev(modes_update(vec![], Some("plan".to_string()))));
    assert_eq!(st.current_mode.as_deref(), Some("plan"));
    assert_eq!(
        st.available_modes.len(),
        list_len,
        "a current-only update must NOT wipe the available list"
    );
}

// ── opencode full list + current via ModesUpdated ────────────────────────────

#[test]
fn modes_update_sets_list_and_current() {
    let modes = vec![
        SessionMode {
            id: "build".to_string(),
            label: "build".to_string(),
            description: "default".to_string(),
        },
        SessionMode {
            id: "plan".to_string(),
            label: "plan".to_string(),
            description: "no edits".to_string(),
        },
    ];
    let st = reduce(
        SessionState::initial(),
        ev(modes_update(modes, Some("build".to_string()))),
    );
    let ids: Vec<String> = st.available_modes.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, vec!["build", "plan"]);
    assert_eq!(st.current_mode.as_deref(), Some("build"));
}

#[test]
fn model_changed_updates_model() {
    let st = after_init(session_init("s", "old-model", "/p"));
    let st = reduce(st, ev(wasit_core::AgentEvent::ModelChanged {
        model: "new-model".to_string(),
    }));
    assert_eq!(st.model.as_deref(), Some("new-model"));
}
