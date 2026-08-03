//! opencode_session_updates.rs — ModesUpdated (current mode) + SlashCommandsUpdated.
//!
//! A `ModesUpdated { current_mode }` sets `current_mode`; a `SlashCommandsUpdated`
//! REPLACES `slash_commands` wholesale (mid-session catalog snapshot). Neither
//! changes phase, and both survive unrelated events.

mod common;
use common::*;

use marsad_events::{reduce, Phase, SessionState};
use wasit_core::SlashCommandInfo;

fn init_state() -> SessionState {
    after_init(session_init("sess-updates", "gpt-4o", "/tmp/proj"))
}

fn cmds(items: &[(&str, &str, &str)]) -> Vec<SlashCommandInfo> {
    items
        .iter()
        .map(|(name, description, argument_hint)| SlashCommandInfo {
            name: name.to_string(),
            description: description.to_string(),
            argument_hint: argument_hint.to_string(),
        })
        .collect()
}

// ===========================================================================
// mode change → current_mode
// ===========================================================================

#[test]
fn eu1_mode_update_sets_current_mode() {
    let s1 = reduce(init_state(), ev(modes_update(vec![], Some("plan".to_string()))));
    assert_eq!(s1.current_mode.as_deref(), Some("plan"));
}

#[test]
fn eu2_current_mode_absent_before_any_update() {
    let s = init_state();
    assert!(s.current_mode.is_none());
    let json = serde_json::to_value(&s).expect("serialize");
    assert!(json.get("current_mode").is_some(), "field present even when null");
}

#[test]
fn eu3_second_mode_update_replaces_first() {
    let s1 = reduce(init_state(), ev(modes_update(vec![], Some("plan".to_string()))));
    assert_eq!(s1.current_mode.as_deref(), Some("plan"));
    let s2 = reduce(s1, ev(modes_update(vec![], Some("build".to_string()))));
    assert_eq!(s2.current_mode.as_deref(), Some("build"));
}

#[test]
fn eu4_unrelated_event_does_not_change_current_mode() {
    let s1 = reduce(init_state(), ev(modes_update(vec![], Some("build".to_string()))));
    let s2 = reduce(s1, ev(assistant_delta("working on it")));
    assert_eq!(s2.current_mode.as_deref(), Some("build"));
}

#[test]
fn eu5_mode_update_does_not_change_phase() {
    let s0 = init_state();
    assert_eq!(s0.phase, Phase::Working);
    let s1 = reduce(s0, ev(modes_update(vec![], Some("plan".to_string()))));
    assert_eq!(s1.phase, Phase::Working);
    assert_eq!(s1.current_mode.as_deref(), Some("plan"));
}

// ===========================================================================
// slash catalog update
// ===========================================================================

#[test]
fn es1_slash_update_replaces_slash_commands() {
    let s0 = init_state();
    assert!(s0.slash_commands.is_empty());
    let s1 = reduce(
        s0,
        ev(slash_update(cmds(&[
            ("/build", "Build the project", ""),
            ("/test", "Run tests", "[pattern]"),
            ("/clear", "Clear context", ""),
        ]))),
    );
    assert_eq!(s1.slash_commands.len(), 3);
    assert_eq!(s1.slash_commands[0].name, "/build");
    assert_eq!(s1.slash_commands[1].description, "Run tests");
    assert_eq!(s1.slash_commands[2].name, "/clear");
}

#[test]
fn es2_second_slash_update_replaces_first() {
    let s1 = reduce(
        init_state(),
        ev(slash_update(cmds(&[
            ("/old-cmd-1", "Old command one", ""),
            ("/old-cmd-2", "Old command two", "[args]"),
        ]))),
    );
    assert_eq!(s1.slash_commands.len(), 2);
    let s2 = reduce(s1, ev(slash_update(cmds(&[("/new-cmd", "New command", "")]))));
    assert_eq!(s2.slash_commands.len(), 1, "second slash_update REPLACES");
    let names: Vec<&str> = s2.slash_commands.iter().map(|c| c.name.as_str()).collect();
    assert!(!names.contains(&"/old-cmd-1"));
    assert!(names.contains(&"/new-cmd"));
}

#[test]
fn es3_slash_update_carries_all_fields() {
    let s1 = reduce(
        init_state(),
        ev(slash_update(cmds(&[("/deploy", "Deploy to production", "<env>")]))),
    );
    assert_eq!(s1.slash_commands.len(), 1);
    let cmd = &s1.slash_commands[0];
    assert_eq!(cmd.name, "/deploy");
    assert_eq!(cmd.description, "Deploy to production");
    assert_eq!(cmd.argument_hint, "<env>");
}

#[test]
fn es4_slash_update_does_not_change_phase() {
    let s0 = init_state();
    assert_eq!(s0.phase, Phase::Working);
    let s1 = reduce(s0, ev(slash_update(cmds(&[("/help", "Show help", "")]))));
    assert_eq!(s1.phase, Phase::Working);
    assert_eq!(s1.slash_commands.len(), 1);
}

#[test]
fn es5_slash_commands_survive_unrelated_event() {
    let s1 = reduce(init_state(), ev(slash_update(cmds(&[("/persist", "Should survive", "")]))));
    let s2 = reduce(s1, ev(assistant_delta("done")));
    assert_eq!(s2.slash_commands.len(), 1);
    assert_eq!(s2.slash_commands[0].name, "/persist");
}
