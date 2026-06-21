//! B10 `marsad-notify` — fakes / CaptureNotifier contract tests.
//!
//! Covers:
//! - CaptureNotifier implements Notifier trait
//! - deliver records notifications in order
//! - delivered() accessor returns the correct slice
//! - new() starts empty
//! - Multiple delivers accumulate in insertion order
//! - CaptureNotifier always returns Ok(())

use marsad_notify::{
    build_notification, CaptureNotifier, NotifEvent, NotifKind, NotifPrefs, Notification, Notifier,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn all_on_prefs() -> NotifPrefs {
    NotifPrefs {
        needs: true,
        error: true,
        done: true,
        started: true,
        sound: true,
        group: true,
        dnd: false,
    }
}

fn needs_notification() -> Notification {
    let ev = NotifEvent {
        kind: NotifKind::Needs,
        session_id: "sess-001".to_string(),
        host: "localhost".to_string(),
        title: "api-gateway needs you".to_string(),
        body: "Wants to run: prisma migrate deploy".to_string(),
    };
    build_notification(&ev, &all_on_prefs()).unwrap()
}

fn error_notification() -> Notification {
    let ev = NotifEvent {
        kind: NotifKind::Error,
        session_id: "sess-002".to_string(),
        host: "localhost".to_string(),
        title: "payments errored".to_string(),
        body: "TypeError in computeTax".to_string(),
    };
    build_notification(&ev, &all_on_prefs()).unwrap()
}

fn done_notification() -> Notification {
    let ev = NotifEvent {
        kind: NotifKind::Done,
        session_id: "sess-003".to_string(),
        host: "hetzner-fra1".to_string(),
        title: "etl-pipeline finished".to_string(),
        body: "9 files changed".to_string(),
    };
    build_notification(&ev, &all_on_prefs()).unwrap()
}

fn started_notification() -> Notification {
    let ev = NotifEvent {
        kind: NotifKind::Started,
        session_id: "sess-004".to_string(),
        host: "localhost".to_string(),
        title: "worker booted".to_string(),
        body: "Session starting".to_string(),
    };
    build_notification(&ev, &all_on_prefs()).unwrap()
}

// ---------------------------------------------------------------------------
// new() starts empty
// ---------------------------------------------------------------------------

#[test]
fn capture_notifier_new_is_empty() {
    let cap = CaptureNotifier::new();
    assert!(
        cap.delivered().is_empty(),
        "new CaptureNotifier must have no delivered notifications"
    );
}

#[test]
fn capture_notifier_delivered_returns_empty_slice() {
    let cap = CaptureNotifier::new();
    assert_eq!(cap.delivered().len(), 0);
}

// ---------------------------------------------------------------------------
// deliver always returns Ok(())
// ---------------------------------------------------------------------------

#[test]
fn capture_notifier_deliver_returns_ok() {
    let mut cap = CaptureNotifier::new();
    let note = needs_notification();
    let result = cap.deliver(&note);
    assert!(
        result.is_ok(),
        "CaptureNotifier::deliver must always return Ok(())"
    );
}

#[test]
fn capture_notifier_deliver_error_returns_ok() {
    let mut cap = CaptureNotifier::new();
    let note = error_notification();
    assert!(cap.deliver(&note).is_ok());
}

#[test]
fn capture_notifier_deliver_done_returns_ok() {
    let mut cap = CaptureNotifier::new();
    let note = done_notification();
    assert!(cap.deliver(&note).is_ok());
}

#[test]
fn capture_notifier_deliver_started_returns_ok() {
    let mut cap = CaptureNotifier::new();
    let note = started_notification();
    assert!(cap.deliver(&note).is_ok());
}

// ---------------------------------------------------------------------------
// deliver records notifications in order
// ---------------------------------------------------------------------------

#[test]
fn capture_notifier_records_single_delivery() {
    let mut cap = CaptureNotifier::new();
    let note = needs_notification();
    cap.deliver(&note).unwrap();
    assert_eq!(cap.delivered().len(), 1);
    assert_eq!(cap.delivered()[0].kind, NotifKind::Needs);
}

#[test]
fn capture_notifier_records_delivery_title_and_body() {
    let mut cap = CaptureNotifier::new();
    let note = needs_notification();
    cap.deliver(&note).unwrap();
    assert_eq!(cap.delivered()[0].title, "api-gateway needs you");
    assert_eq!(
        cap.delivered()[0].body,
        "Wants to run: prisma migrate deploy"
    );
}

#[test]
fn capture_notifier_records_session_id() {
    let mut cap = CaptureNotifier::new();
    let note = needs_notification();
    cap.deliver(&note).unwrap();
    assert_eq!(cap.delivered()[0].session_id, "sess-001");
}

#[test]
fn capture_notifier_records_host() {
    let mut cap = CaptureNotifier::new();
    let note = needs_notification();
    cap.deliver(&note).unwrap();
    assert_eq!(cap.delivered()[0].host, "localhost");
}

#[test]
fn capture_notifier_records_two_deliveries_in_order() {
    let mut cap = CaptureNotifier::new();
    let note1 = needs_notification();
    let note2 = error_notification();
    cap.deliver(&note1).unwrap();
    cap.deliver(&note2).unwrap();
    assert_eq!(cap.delivered().len(), 2);
    assert_eq!(cap.delivered()[0].kind, NotifKind::Needs);
    assert_eq!(cap.delivered()[1].kind, NotifKind::Error);
}

#[test]
fn capture_notifier_records_four_deliveries_in_order() {
    let mut cap = CaptureNotifier::new();
    let notes = [
        needs_notification(),
        error_notification(),
        done_notification(),
        started_notification(),
    ];
    let expected_kinds = [
        NotifKind::Needs,
        NotifKind::Error,
        NotifKind::Done,
        NotifKind::Started,
    ];
    for note in &notes {
        cap.deliver(note).unwrap();
    }
    assert_eq!(cap.delivered().len(), 4);
    for (i, expected_kind) in expected_kinds.iter().enumerate() {
        assert_eq!(
            &cap.delivered()[i].kind,
            expected_kind,
            "Position {i} must be {expected_kind:?}"
        );
    }
}

#[test]
fn capture_notifier_records_reversed_order() {
    let mut cap = CaptureNotifier::new();
    cap.deliver(&done_notification()).unwrap();
    cap.deliver(&needs_notification()).unwrap();
    assert_eq!(cap.delivered()[0].kind, NotifKind::Done);
    assert_eq!(cap.delivered()[1].kind, NotifKind::Needs);
}

// ---------------------------------------------------------------------------
// Multiple delivers accumulate
// ---------------------------------------------------------------------------

#[test]
fn capture_notifier_accumulates_ten_deliveries() {
    let mut cap = CaptureNotifier::new();
    let note = needs_notification();
    for _ in 0..10 {
        cap.deliver(&note).unwrap();
    }
    assert_eq!(
        cap.delivered().len(),
        10,
        "CaptureNotifier must accumulate all 10 deliveries"
    );
}

// ---------------------------------------------------------------------------
// delivered() is accessible as shared reference (not consuming)
// ---------------------------------------------------------------------------

#[test]
fn capture_notifier_delivered_accessible_multiple_times() {
    let mut cap = CaptureNotifier::new();
    cap.deliver(&needs_notification()).unwrap();
    // Call delivered() twice — must not move or consume
    let first = cap.delivered().len();
    let second = cap.delivered().len();
    assert_eq!(
        first, second,
        "delivered() must be accessible multiple times"
    );
}

// ---------------------------------------------------------------------------
// CaptureNotifier is usable through Notifier trait object
// ---------------------------------------------------------------------------

#[test]
fn capture_notifier_via_trait_object() {
    let mut cap: Box<dyn Notifier> = Box::new(CaptureNotifier::new());
    let note = needs_notification();
    cap.deliver(&note)
        .expect("trait-object deliver must succeed");
    // NOTE(test-agent): we cannot call .delivered() through a trait object because it
    // is not part of the Notifier trait. This is by design — callers downcast or use
    // the concrete type. The test validates that CaptureNotifier satisfies the trait.
}

#[test]
fn capture_notifier_is_notifier_via_generic() {
    fn deliver_via_generic<N: Notifier>(notifier: &mut N, note: &Notification) {
        notifier
            .deliver(note)
            .expect("generic deliver must succeed");
    }
    let mut cap = CaptureNotifier::new();
    deliver_via_generic(&mut cap, &needs_notification());
    assert_eq!(
        cap.delivered().len(),
        1,
        "Generic deliver must record the notification"
    );
}

// ---------------------------------------------------------------------------
// Notification data types: derive PartialEq, Clone, Debug
// ---------------------------------------------------------------------------

#[test]
fn notification_derives_clone_and_partial_eq() {
    let note = needs_notification();
    let cloned = note.clone();
    assert_eq!(note, cloned, "Notification must derive Clone and PartialEq");
}

#[test]
fn notification_derives_debug() {
    let note = needs_notification();
    let s = format!("{:?}", note);
    assert!(!s.is_empty(), "Notification must derive Debug");
}

#[test]
fn notif_kind_derives_clone_and_partial_eq() {
    let k1 = NotifKind::Needs;
    let k2 = k1.clone();
    assert_eq!(k1, k2, "NotifKind must derive Clone and PartialEq");
}

#[test]
fn notif_kind_derives_debug() {
    let s = format!("{:?}", NotifKind::Error);
    assert!(!s.is_empty());
}
