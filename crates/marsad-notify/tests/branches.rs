//! B10 `marsad-notify` — branch coverage tests.
//!
//! Covers:
//! - Each event toggle OFF → None
//! - DND on → None AND no sound
//! - play_sound gating (sound && High && !dnd) across all 8 combinations
//! - Priority mapping: Needs/Error=High, Done/Started=Low
//! - Actions per kind: exact labels and primary flags
//! - group ON collapses same-kind bursts into one summary ("N sessions finished")
//! - group OFF returns input unchanged
//! - Mixed-kind groups (only same-kind are collapsed; different kinds are separate)

use marsad_notify::{
    build_notification, group, NotifEvent, NotifKind, NotifPrefs, Notification, Priority,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_prefs(
    needs: bool,
    error: bool,
    done: bool,
    started: bool,
    sound: bool,
    group_on: bool,
    dnd: bool,
) -> NotifPrefs {
    NotifPrefs {
        needs,
        error,
        done,
        started,
        sound,
        group: group_on,
        dnd,
    }
}

fn all_on_prefs() -> NotifPrefs {
    make_prefs(true, true, true, true, true, true, false)
}

fn needs_ev() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Needs,
        session_id: "s-needs".to_string(),
        host: "localhost".to_string(),
        title: "api-gateway needs you".to_string(),
        body: "Wants to run: prisma migrate".to_string(),
    }
}

fn error_ev() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Error,
        session_id: "s-error".to_string(),
        host: "localhost".to_string(),
        title: "payments errored".to_string(),
        body: "TypeError in computeTax".to_string(),
    }
}

fn done_ev() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Done,
        session_id: "s-done".to_string(),
        host: "hetzner-fra1".to_string(),
        title: "etl-pipeline finished".to_string(),
        body: "9 files changed".to_string(),
    }
}

fn started_ev() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Started,
        session_id: "s-started".to_string(),
        host: "localhost".to_string(),
        title: "worker booted".to_string(),
        body: "New session is starting".to_string(),
    }
}

/// Build N done notifications using all_on_prefs()
fn build_done_burst(n: usize) -> Vec<Notification> {
    (0..n)
        .map(|i| {
            let ev = NotifEvent {
                kind: NotifKind::Done,
                session_id: format!("sess-done-{i}"),
                host: "localhost".to_string(),
                title: format!("session-{i} finished"),
                body: "All work done".to_string(),
            };
            build_notification(&ev, &all_on_prefs()).unwrap()
        })
        .collect()
}

/// Build N needs notifications using all_on_prefs()
fn build_needs_burst(n: usize) -> Vec<Notification> {
    (0..n)
        .map(|i| {
            let ev = NotifEvent {
                kind: NotifKind::Needs,
                session_id: format!("sess-needs-{i}"),
                host: "localhost".to_string(),
                title: format!("session-{i} needs you"),
                body: "Gate waiting".to_string(),
            };
            build_notification(&ev, &all_on_prefs()).unwrap()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Event toggle OFF → None
// ---------------------------------------------------------------------------

#[test]
fn needs_toggle_off_returns_none() {
    let prefs = make_prefs(false, true, true, true, true, true, false);
    assert!(
        build_notification(&needs_ev(), &prefs).is_none(),
        "needs=false must suppress Needs notifications"
    );
}

#[test]
fn error_toggle_off_returns_none() {
    let prefs = make_prefs(true, false, true, true, true, true, false);
    assert!(
        build_notification(&error_ev(), &prefs).is_none(),
        "error=false must suppress Error notifications"
    );
}

#[test]
fn done_toggle_off_returns_none() {
    let prefs = make_prefs(true, true, false, true, true, true, false);
    assert!(
        build_notification(&done_ev(), &prefs).is_none(),
        "done=false must suppress Done notifications"
    );
}

#[test]
fn started_toggle_off_returns_none() {
    let prefs = make_prefs(true, true, true, false, true, true, false);
    assert!(
        build_notification(&started_ev(), &prefs).is_none(),
        "started=false must suppress Started notifications"
    );
}

// ---------------------------------------------------------------------------
// DND on → None AND no sound
// ---------------------------------------------------------------------------

#[test]
fn dnd_on_suppresses_needs() {
    let prefs = make_prefs(true, true, true, true, true, true, true);
    assert!(
        build_notification(&needs_ev(), &prefs).is_none(),
        "DND must suppress Needs notifications"
    );
}

#[test]
fn dnd_on_suppresses_error() {
    let prefs = make_prefs(true, true, true, true, true, true, true);
    assert!(
        build_notification(&error_ev(), &prefs).is_none(),
        "DND must suppress Error notifications"
    );
}

#[test]
fn dnd_on_suppresses_done() {
    let prefs = make_prefs(true, true, true, true, true, true, true);
    assert!(
        build_notification(&done_ev(), &prefs).is_none(),
        "DND must suppress Done notifications"
    );
}

#[test]
fn dnd_on_suppresses_started() {
    let prefs = make_prefs(true, true, true, true, true, true, true);
    assert!(
        build_notification(&started_ev(), &prefs).is_none(),
        "DND must suppress Started notifications"
    );
}

// ---------------------------------------------------------------------------
// play_sound gating: sound && priority==High && !dnd
// 8 combinations of (sound: bool, priority: High/Low, dnd: bool)
// ---------------------------------------------------------------------------

// sound=true, High, dnd=false → play_sound=true
#[test]
fn play_sound_true_when_sound_high_no_dnd() {
    let prefs = make_prefs(true, true, true, true, true, true, false);
    let note = build_notification(&needs_ev(), &prefs).unwrap(); // Needs is High
    assert!(
        note.play_sound,
        "sound=true + High + dnd=false must play sound"
    );
}

// sound=false, High, dnd=false → play_sound=false
#[test]
fn play_sound_false_when_sound_off_high_no_dnd() {
    let prefs = make_prefs(true, true, true, true, false, true, false);
    let note = build_notification(&needs_ev(), &prefs).unwrap();
    assert!(
        !note.play_sound,
        "sound=false must prevent sound even for High priority"
    );
}

// sound=true, Low, dnd=false → play_sound=false
#[test]
fn play_sound_false_when_sound_on_low_no_dnd() {
    let prefs = make_prefs(true, true, true, true, true, true, false);
    let note = build_notification(&done_ev(), &prefs).unwrap(); // Done is Low
    assert!(
        !note.play_sound,
        "Low priority must not play sound even if sound=true"
    );
}

// sound=false, Low, dnd=false → play_sound=false
#[test]
fn play_sound_false_when_sound_off_low_no_dnd() {
    let prefs = make_prefs(true, true, true, true, false, true, false);
    let note = build_notification(&done_ev(), &prefs).unwrap();
    assert!(!note.play_sound, "sound=false + Low must not play sound");
}

// DND combinations: all must return None, so play_sound cannot be observed directly.
// We verify the None suppression (already done above) is sufficient for DND coverage.
// NOTE(test-agent): When dnd=true, build_notification returns None, so play_sound
// on the notification struct cannot be asserted post-build. The spec says
// "DND on → None AND no sound" which is satisfied by returning None (no notification
// is delivered, hence no sound). The code-agent must ensure play_sound=false on any
// notification if it were somehow constructed with dnd=true, but since the public
// API returns None in that case, we cannot test the field directly.

// sound=true, High, dnd=true → None (no delivery, no sound)
#[test]
fn dnd_on_sound_on_high_returns_none() {
    let prefs = make_prefs(true, true, true, true, true, true, true);
    assert!(build_notification(&needs_ev(), &prefs).is_none());
}

// sound=false, High, dnd=true → None
#[test]
fn dnd_on_sound_off_high_returns_none() {
    let prefs = make_prefs(true, true, true, true, false, true, true);
    assert!(build_notification(&needs_ev(), &prefs).is_none());
}

// sound=true, Low, dnd=true → None
#[test]
fn dnd_on_sound_on_low_returns_none() {
    let prefs = make_prefs(true, true, true, true, true, true, true);
    assert!(build_notification(&done_ev(), &prefs).is_none());
}

// sound=false, Low, dnd=true → None
#[test]
fn dnd_on_sound_off_low_returns_none() {
    let prefs = make_prefs(true, true, true, true, false, true, true);
    assert!(build_notification(&done_ev(), &prefs).is_none());
}

// Error (High) + sound=true, dnd=false → play_sound=true
#[test]
fn play_sound_true_for_error_kind() {
    let prefs = make_prefs(true, true, true, true, true, true, false);
    let note = build_notification(&error_ev(), &prefs).unwrap();
    assert!(
        note.play_sound,
        "Error/High + sound=true + dnd=false must play sound"
    );
}

// Started (Low) + sound=true, dnd=false → play_sound=false
#[test]
fn play_sound_false_for_started_kind() {
    let prefs = make_prefs(true, true, true, true, true, true, false);
    let note = build_notification(&started_ev(), &prefs).unwrap();
    assert!(!note.play_sound, "Started/Low must not play sound");
}

// ---------------------------------------------------------------------------
// Priority mapping
// ---------------------------------------------------------------------------

#[test]
fn priority_needs_is_high() {
    let note = build_notification(&needs_ev(), &all_on_prefs()).unwrap();
    assert_eq!(note.priority, Priority::High);
}

#[test]
fn priority_error_is_high() {
    let note = build_notification(&error_ev(), &all_on_prefs()).unwrap();
    assert_eq!(note.priority, Priority::High);
}

#[test]
fn priority_done_is_low() {
    let note = build_notification(&done_ev(), &all_on_prefs()).unwrap();
    assert_eq!(note.priority, Priority::Low);
}

#[test]
fn priority_started_is_low() {
    let note = build_notification(&started_ev(), &all_on_prefs()).unwrap();
    assert_eq!(note.priority, Priority::Low);
}

// ---------------------------------------------------------------------------
// Actions per kind — labels and primary flags
// ---------------------------------------------------------------------------

#[test]
fn actions_needs_approve_primary_reject_open() {
    let note = build_notification(&needs_ev(), &all_on_prefs()).unwrap();
    assert_eq!(note.actions.len(), 3);
    assert_eq!(note.actions[0].label, "Approve");
    assert!(note.actions[0].primary);
    assert_eq!(note.actions[1].label, "Reject");
    assert!(!note.actions[1].primary);
    assert_eq!(note.actions[2].label, "Open");
    assert!(!note.actions[2].primary);
}

#[test]
fn actions_error_open_primary_retry() {
    let note = build_notification(&error_ev(), &all_on_prefs()).unwrap();
    assert_eq!(note.actions.len(), 2);
    assert_eq!(note.actions[0].label, "Open");
    assert!(note.actions[0].primary);
    assert_eq!(note.actions[1].label, "Retry");
    assert!(!note.actions[1].primary);
}

#[test]
fn actions_done_open_dismiss() {
    let note = build_notification(&done_ev(), &all_on_prefs()).unwrap();
    assert_eq!(note.actions.len(), 2);
    assert_eq!(note.actions[0].label, "Open");
    assert!(!note.actions[0].primary);
    assert_eq!(note.actions[1].label, "Dismiss");
    assert!(!note.actions[1].primary);
}

#[test]
fn actions_started_open_only() {
    let note = build_notification(&started_ev(), &all_on_prefs()).unwrap();
    assert_eq!(note.actions.len(), 1);
    assert_eq!(note.actions[0].label, "Open");
    assert!(!note.actions[0].primary);
}

// ---------------------------------------------------------------------------
// group ON — collapses same-kind bursts into one summary
// ---------------------------------------------------------------------------

#[test]
fn group_on_single_notification_unchanged() {
    let prefs = all_on_prefs();
    let notes = build_done_burst(1);
    let result = group(&notes, &prefs);
    assert_eq!(
        result.len(),
        1,
        "Single notification must pass through unchanged"
    );
}

#[test]
fn group_on_two_done_collapses_to_one() {
    let prefs = all_on_prefs();
    let notes = build_done_burst(2);
    let result = group(&notes, &prefs);
    assert_eq!(
        result.len(),
        1,
        "Two Done notifications must collapse to one with group=true"
    );
}

#[test]
fn group_on_five_done_collapses_to_one() {
    let prefs = all_on_prefs();
    let notes = build_done_burst(5);
    let result = group(&notes, &prefs);
    assert_eq!(
        result.len(),
        1,
        "Five Done notifications must collapse to one with group=true"
    );
}

#[test]
fn group_on_summary_body_contains_count() {
    let prefs = all_on_prefs();
    let notes = build_done_burst(5);
    let result = group(&notes, &prefs);
    let summary = &result[0];
    // The summary must mention the count (5) somewhere in title or body
    let text = format!("{} {}", summary.title, summary.body);
    assert!(
        text.contains('5'),
        "Summary notification must mention count 5 in title or body; got: '{text}'"
    );
}

#[test]
fn group_on_needs_burst_collapses_to_one() {
    let prefs = all_on_prefs();
    let notes = build_needs_burst(3);
    let result = group(&notes, &prefs);
    assert_eq!(
        result.len(),
        1,
        "Three Needs notifications must collapse to one"
    );
}

#[test]
fn group_on_summary_kind_preserved() {
    let prefs = all_on_prefs();
    let notes = build_done_burst(3);
    let result = group(&notes, &prefs);
    assert_eq!(
        result[0].kind,
        NotifKind::Done,
        "Summary must preserve the collapsed kind"
    );
}

// ---------------------------------------------------------------------------
// group OFF — returns input unchanged
// ---------------------------------------------------------------------------

#[test]
fn group_off_single_notification_unchanged() {
    let mut prefs = all_on_prefs();
    prefs.group = false;
    let notes = build_done_burst(1);
    let result = group(&notes, &prefs);
    assert_eq!(result.len(), 1);
}

#[test]
fn group_off_burst_not_collapsed() {
    let mut prefs = all_on_prefs();
    prefs.group = false;
    let notes = build_done_burst(5);
    let result = group(&notes, &prefs);
    assert_eq!(
        result.len(),
        5,
        "group=false must return all 5 notifications unchanged"
    );
}

#[test]
fn group_off_mixed_kinds_not_collapsed() {
    let mut prefs = all_on_prefs();
    prefs.group = false;
    let mut notes = build_done_burst(2);
    notes.extend(build_needs_burst(2));
    let result = group(&notes, &prefs);
    assert_eq!(
        result.len(),
        4,
        "group=false must not collapse any notifications"
    );
}

// ---------------------------------------------------------------------------
// Mixed-kind groups — only same-kind collapse; different kinds are separate
// ---------------------------------------------------------------------------

#[test]
fn group_on_mixed_kinds_collapses_each_kind_separately() {
    let prefs = all_on_prefs();
    // 3 Done + 2 Needs → should produce 2 groups (one Done summary, one Needs summary)
    let mut notes = build_done_burst(3);
    notes.extend(build_needs_burst(2));
    let result = group(&notes, &prefs);
    assert_eq!(
        result.len(),
        2,
        "Mixed-kind burst must collapse each kind separately; expected 2 summaries, got {}",
        result.len()
    );
    let kinds: Vec<&NotifKind> = result.iter().map(|n| &n.kind).collect();
    assert!(
        kinds.contains(&&NotifKind::Done),
        "Mixed group result must contain a Done summary"
    );
    assert!(
        kinds.contains(&&NotifKind::Needs),
        "Mixed group result must contain a Needs summary"
    );
}

#[test]
fn group_on_all_four_kinds_two_each() {
    let prefs = all_on_prefs();
    // 2 of each kind → 4 summaries
    let mut notes = build_done_burst(2);
    notes.extend(build_needs_burst(2));
    // Error and Started bursts
    let error_notes: Vec<Notification> = (0..2)
        .map(|i| {
            let ev = NotifEvent {
                kind: NotifKind::Error,
                session_id: format!("sess-error-{i}"),
                host: "localhost".to_string(),
                title: format!("session-{i} errored"),
                body: "Error".to_string(),
            };
            build_notification(&ev, &prefs).unwrap()
        })
        .collect();
    let started_notes: Vec<Notification> = (0..2)
        .map(|i| {
            let ev = NotifEvent {
                kind: NotifKind::Started,
                session_id: format!("sess-started-{i}"),
                host: "localhost".to_string(),
                title: format!("session-{i} started"),
                body: "Started".to_string(),
            };
            build_notification(&ev, &prefs).unwrap()
        })
        .collect();
    notes.extend(error_notes);
    notes.extend(started_notes);
    let result = group(&notes, &prefs);
    assert_eq!(
        result.len(),
        4,
        "Four kinds × 2 each must produce 4 summaries, got {}",
        result.len()
    );
}

#[test]
fn group_on_empty_input_returns_empty() {
    let prefs = all_on_prefs();
    let result = group(&[], &prefs);
    assert!(
        result.is_empty(),
        "group() on empty input must return empty vec"
    );
}

#[test]
fn group_off_empty_input_returns_empty() {
    let mut prefs = all_on_prefs();
    prefs.group = false;
    let result = group(&[], &prefs);
    assert!(
        result.is_empty(),
        "group() with group=false on empty input must return empty vec"
    );
}
