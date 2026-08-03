//! opencode_plan.rs — plan-mode PlanUpdated folds into SessionState.plan_entries.
//!
//! Each PlanUpdated is a FULL SNAPSHOT (replace, not append). Feeding one via
//! the reducer populates `plan_entries`; a later one replaces it wholesale; and
//! unrelated events must not clear it. Phase is untouched.

mod common;
use common::*;

use marsad_events::{reduce, Phase, SessionState};
use wasit_core::PlanEntry;

fn init_state() -> SessionState {
    after_init(session_init("sess-plan", "gpt-4o", "/tmp/work"))
}

fn entries(items: &[(&str, &str)]) -> Vec<PlanEntry> {
    items
        .iter()
        .map(|(content, priority)| PlanEntry {
            content: content.to_string(),
            priority: priority.to_string(),
        })
        .collect()
}

#[test]
fn ep1_plan_update_populates_plan_entries() {
    let s1 = reduce(
        init_state(),
        ev(plan_update(entries(&[
            ("Implement the feature", "high"),
            ("Write tests", "medium"),
            ("Update docs", "low"),
        ]))),
    );
    assert_eq!(s1.plan_entries.len(), 3);
    assert_eq!(s1.plan_entries[0].content, "Implement the feature");
    assert_eq!(s1.plan_entries[0].priority, "high");
    assert_eq!(s1.plan_entries[1].priority, "medium");
    assert_eq!(s1.plan_entries[2].priority, "low");
}

#[test]
fn ep2_plan_entries_empty_before_any_update() {
    let s = init_state();
    assert!(s.plan_entries.is_empty());
    // The field is always present in serialized form.
    let json = serde_json::to_value(&s).expect("serialize");
    assert!(json.get("plan_entries").is_some());
}

#[test]
fn ep3_plan_entries_ordered() {
    let s1 = reduce(
        init_state(),
        ev(plan_update(entries(&[
            ("Step A", "high"),
            ("Step B", "low"),
            ("Step C", "medium"),
            ("Step D", "high"),
        ]))),
    );
    let contents: Vec<&str> = s1.plan_entries.iter().map(|e| e.content.as_str()).collect();
    assert_eq!(contents, vec!["Step A", "Step B", "Step C", "Step D"]);
}

#[test]
fn ep4_second_plan_update_replaces_first() {
    let s1 = reduce(
        init_state(),
        ev(plan_update(entries(&[
            ("Old step 1", "high"),
            ("Old step 2", "medium"),
            ("Old step 3", "low"),
        ]))),
    );
    assert_eq!(s1.plan_entries.len(), 3);
    let s2 = reduce(
        s1,
        ev(plan_update(entries(&[("New step 1", "high"), ("New step 2", "low")]))),
    );
    assert_eq!(s2.plan_entries.len(), 2, "second plan_update REPLACES the first");
    let contents: Vec<&str> = s2.plan_entries.iter().map(|e| e.content.as_str()).collect();
    assert!(!contents.contains(&"Old step 1"));
    assert!(contents.contains(&"New step 1"));
}

#[test]
fn ep5_plan_update_does_not_change_phase() {
    let s0 = init_state();
    assert_eq!(s0.phase, Phase::Working);
    let s1 = reduce(s0, ev(plan_update(entries(&[("Do something", "high")]))));
    assert_eq!(s1.phase, Phase::Working);
    assert_eq!(s1.plan_entries.len(), 1);
}

#[test]
fn ep6_plan_entries_survive_unrelated_event() {
    let s1 = reduce(init_state(), ev(plan_update(entries(&[("Persistent step", "high")]))));
    let s2 = reduce(s1, ev(assistant_delta("Thinking…")));
    assert_eq!(s2.plan_entries.len(), 1);
    assert_eq!(s2.plan_entries[0].content, "Persistent step");
}
