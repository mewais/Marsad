//! B10 `marsad-notify` — happy path tests.
//!
//! Covers:
//! - build_notification for each NotifKind with default prefs → correct
//!   title/body/priority/actions
//! - deliver via CaptureNotifier and assert delivered()

use marsad_notify::{
    build_notification, Action, CaptureNotifier, NotifEvent, NotifKind, NotifPrefs, Notifier,
    Notification, Priority,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_prefs() -> NotifPrefs {
    NotifPrefs::default()
}

fn needs_event() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Needs,
        session_id: "sess-needs".to_string(),
        host: "localhost".to_string(),
        title: "api-gateway needs you".to_string(),
        body: "Wants to run: npx prisma migrate deploy".to_string(),
    }
}

fn error_event() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Error,
        session_id: "sess-error".to_string(),
        host: "localhost".to_string(),
        title: "payments-svc errored".to_string(),
        body: "TypeError in computeTax (checkout.py:88)".to_string(),
    }
}

fn done_event() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Done,
        session_id: "sess-done".to_string(),
        host: "hetzner-fra1".to_string(),
        title: "etl-pipeline finished".to_string(),
        body: "9 files changed · batch job is 6.1× faster".to_string(),
    }
}

fn started_event() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Started,
        session_id: "sess-started".to_string(),
        host: "localhost".to_string(),
        title: "worker booted".to_string(),
        body: "New session is starting".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Default prefs invariants
// ---------------------------------------------------------------------------

#[test]
fn default_prefs_values() {
    let p = NotifPrefs::default();
    assert!(p.needs, "needs should be on by default");
    assert!(p.error, "error should be on by default");
    assert!(p.done, "done should be on by default");
    assert!(!p.started, "started should be off by default");
    assert!(p.sound, "sound should be on by default");
    assert!(p.group, "group should be on by default");
    assert!(!p.dnd, "dnd should be off by default");
}

// ---------------------------------------------------------------------------
// build_notification — Needs kind
// ---------------------------------------------------------------------------

#[test]
fn build_notification_needs_returns_some_with_default_prefs() {
    let ev = needs_event();
    let prefs = default_prefs();
    let note = build_notification(&ev, &prefs);
    assert!(note.is_some(), "Needs event with default prefs must produce a notification");
}

#[test]
fn build_notification_needs_title_and_body() {
    let ev = needs_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.title, ev.title);
    assert_eq!(note.body, ev.body);
}

#[test]
fn build_notification_needs_priority_high() {
    let ev = needs_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.priority, Priority::High, "Needs must map to High priority");
}

#[test]
fn build_notification_needs_host_and_session_id() {
    let ev = needs_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.host, "localhost");
    assert_eq!(note.session_id, "sess-needs");
}

#[test]
fn build_notification_needs_actions_approve_primary_reject_open() {
    let ev = needs_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    // Must have exactly: Approve (primary), Reject, Open
    assert_eq!(note.actions.len(), 3, "Needs must have 3 actions");
    let approve = &note.actions[0];
    assert_eq!(approve.label, "Approve");
    assert!(approve.primary, "Approve must be primary");
    let reject = &note.actions[1];
    assert_eq!(reject.label, "Reject");
    assert!(!reject.primary, "Reject must not be primary");
    let open = &note.actions[2];
    assert_eq!(open.label, "Open");
    assert!(!open.primary, "Open must not be primary");
}

#[test]
fn build_notification_needs_play_sound_true_with_default_prefs() {
    // sound=true, priority=High, dnd=false → play_sound should be true
    let ev = needs_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert!(note.play_sound, "Needs/High + sound on + dnd off must play sound");
}

#[test]
fn build_notification_needs_kind_field() {
    let ev = needs_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.kind, NotifKind::Needs);
}

// ---------------------------------------------------------------------------
// build_notification — Error kind
// ---------------------------------------------------------------------------

#[test]
fn build_notification_error_returns_some_with_default_prefs() {
    let ev = error_event();
    let note = build_notification(&ev, &default_prefs());
    assert!(note.is_some(), "Error event with default prefs must produce a notification");
}

#[test]
fn build_notification_error_title_and_body() {
    let ev = error_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.title, ev.title);
    assert_eq!(note.body, ev.body);
}

#[test]
fn build_notification_error_priority_high() {
    let ev = error_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.priority, Priority::High, "Error must map to High priority");
}

#[test]
fn build_notification_error_actions_open_primary_retry() {
    let ev = error_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    // Must have exactly: Open (primary), Retry
    assert_eq!(note.actions.len(), 2, "Error must have 2 actions");
    let open = &note.actions[0];
    assert_eq!(open.label, "Open");
    assert!(open.primary, "Open must be primary for Error");
    let retry = &note.actions[1];
    assert_eq!(retry.label, "Retry");
    assert!(!retry.primary, "Retry must not be primary");
}

#[test]
fn build_notification_error_play_sound_true_with_default_prefs() {
    let ev = error_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert!(note.play_sound, "Error/High + sound on + dnd off must play sound");
}

#[test]
fn build_notification_error_kind_field() {
    let ev = error_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.kind, NotifKind::Error);
}

// ---------------------------------------------------------------------------
// build_notification — Done kind
// ---------------------------------------------------------------------------

#[test]
fn build_notification_done_returns_some_with_default_prefs() {
    let ev = done_event();
    let note = build_notification(&ev, &default_prefs());
    assert!(note.is_some(), "Done event with default prefs must produce a notification");
}

#[test]
fn build_notification_done_title_body_host() {
    let ev = done_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.title, ev.title);
    assert_eq!(note.body, ev.body);
    assert_eq!(note.host, "hetzner-fra1");
}

#[test]
fn build_notification_done_priority_low() {
    let ev = done_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.priority, Priority::Low, "Done must map to Low priority");
}

#[test]
fn build_notification_done_actions_open_dismiss() {
    let ev = done_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    // Must have exactly: Open, Dismiss
    assert_eq!(note.actions.len(), 2, "Done must have 2 actions");
    let open = &note.actions[0];
    assert_eq!(open.label, "Open");
    assert!(!open.primary, "Open must not be primary for Done");
    let dismiss = &note.actions[1];
    assert_eq!(dismiss.label, "Dismiss");
    assert!(!dismiss.primary, "Dismiss must not be primary");
}

#[test]
fn build_notification_done_play_sound_false_low_priority() {
    // sound=true but priority=Low → play_sound must be false
    let ev = done_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert!(!note.play_sound, "Done/Low priority must not play sound even if sound pref is on");
}

#[test]
fn build_notification_done_kind_field() {
    let ev = done_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    assert_eq!(note.kind, NotifKind::Done);
}

// ---------------------------------------------------------------------------
// build_notification — Started kind (off by default)
// ---------------------------------------------------------------------------

#[test]
fn build_notification_started_returns_none_with_default_prefs() {
    // started is OFF by default
    let ev = started_event();
    let note = build_notification(&ev, &default_prefs());
    assert!(note.is_none(), "Started is off by default; must return None");
}

#[test]
fn build_notification_started_returns_some_when_enabled() {
    let ev = started_event();
    let mut prefs = default_prefs();
    prefs.started = true;
    let note = build_notification(&ev, &prefs);
    assert!(note.is_some(), "Started with started=true must produce a notification");
}

#[test]
fn build_notification_started_priority_low() {
    let ev = started_event();
    let mut prefs = default_prefs();
    prefs.started = true;
    let note = build_notification(&ev, &prefs).unwrap();
    assert_eq!(note.priority, Priority::Low, "Started must map to Low priority");
}

#[test]
fn build_notification_started_actions_open_only() {
    let ev = started_event();
    let mut prefs = default_prefs();
    prefs.started = true;
    let note = build_notification(&ev, &prefs).unwrap();
    assert_eq!(note.actions.len(), 1, "Started must have exactly 1 action");
    assert_eq!(note.actions[0].label, "Open");
    assert!(!note.actions[0].primary, "Open must not be primary for Started");
}

#[test]
fn build_notification_started_kind_field() {
    let ev = started_event();
    let mut prefs = default_prefs();
    prefs.started = true;
    let note = build_notification(&ev, &prefs).unwrap();
    assert_eq!(note.kind, NotifKind::Started);
}

// ---------------------------------------------------------------------------
// Deliver via CaptureNotifier
// ---------------------------------------------------------------------------

#[test]
fn capture_notifier_deliver_needs_notification() {
    let ev = needs_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    let mut cap = CaptureNotifier::new();
    cap.deliver(&note).expect("deliver must succeed for CaptureNotifier");
    assert_eq!(cap.delivered().len(), 1);
    assert_eq!(cap.delivered()[0].title, ev.title);
}

#[test]
fn capture_notifier_deliver_error_notification() {
    let ev = error_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    let mut cap = CaptureNotifier::new();
    cap.deliver(&note).expect("deliver must succeed");
    assert_eq!(cap.delivered().len(), 1);
    assert_eq!(cap.delivered()[0].kind, NotifKind::Error);
}

#[test]
fn capture_notifier_deliver_done_notification() {
    let ev = done_event();
    let note = build_notification(&ev, &default_prefs()).unwrap();
    let mut cap = CaptureNotifier::new();
    cap.deliver(&note).expect("deliver must succeed");
    assert_eq!(cap.delivered().len(), 1);
    assert_eq!(cap.delivered()[0].kind, NotifKind::Done);
}

#[test]
fn capture_notifier_deliver_all_kinds() {
    let prefs = {
        let mut p = default_prefs();
        p.started = true;
        p
    };
    let events: Vec<NotifEvent> = vec![needs_event(), error_event(), done_event(), started_event()];
    let mut cap = CaptureNotifier::new();
    for ev in &events {
        let note = build_notification(ev, &prefs).unwrap();
        cap.deliver(&note).expect("deliver must succeed");
    }
    assert_eq!(cap.delivered().len(), 4, "All four kinds must be deliverable");
}
