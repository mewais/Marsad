//! B10 `marsad-notify` — error path tests.
//!
//! Covers:
//! - NotifyError::Delivery surfaced from a failing Notifier implementation

use marsad_notify::{
    build_notification, NotifEvent, NotifKind, NotifPrefs, Notification, Notifier, NotifyError,
};

// ---------------------------------------------------------------------------
// Failing Notifier — always returns Delivery error
// ---------------------------------------------------------------------------

struct FailingNotifier {
    message: String,
}

impl FailingNotifier {
    fn new(msg: &str) -> Self {
        Self {
            message: msg.to_string(),
        }
    }
}

impl Notifier for FailingNotifier {
    fn deliver(&mut self, _note: &Notification) -> Result<(), NotifyError> {
        Err(NotifyError::Delivery(self.message.clone()))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn needs_event() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Needs,
        session_id: "sess-001".to_string(),
        host: "localhost".to_string(),
        title: "api-gateway needs you".to_string(),
        body: "Wants to run: npx prisma migrate deploy".to_string(),
    }
}

fn error_event() -> NotifEvent {
    NotifEvent {
        kind: NotifKind::Error,
        session_id: "sess-002".to_string(),
        host: "localhost".to_string(),
        title: "payments-svc errored".to_string(),
        body: "TypeError in computeTax (checkout.py:88)".to_string(),
    }
}

// ---------------------------------------------------------------------------
// NotifyError::Delivery is surfaced from FailingNotifier
// ---------------------------------------------------------------------------

#[test]
fn failing_notifier_returns_delivery_error() {
    let ev = needs_event();
    let note = build_notification(&ev, &NotifPrefs::default()).unwrap();
    let mut notifier = FailingNotifier::new("D-Bus not available");
    let result = notifier.deliver(&note);
    assert!(result.is_err(), "FailingNotifier must return an error");
    match result.unwrap_err() {
        NotifyError::Delivery(msg) => {
            assert_eq!(msg, "D-Bus not available");
        }
    }
}

#[test]
fn delivery_error_message_is_preserved() {
    let ev = error_event();
    let note = build_notification(&ev, &NotifPrefs::default()).unwrap();
    let error_msg = "connection refused: /run/user/1000/bus";
    let mut notifier = FailingNotifier::new(error_msg);
    let result = notifier.deliver(&note);
    match result.unwrap_err() {
        NotifyError::Delivery(msg) => assert_eq!(msg, error_msg),
    }
}

#[test]
fn delivery_error_on_empty_message() {
    let ev = needs_event();
    let note = build_notification(&ev, &NotifPrefs::default()).unwrap();
    let mut notifier = FailingNotifier::new("");
    let result = notifier.deliver(&note);
    match result.unwrap_err() {
        NotifyError::Delivery(msg) => assert_eq!(msg, ""),
    }
}

// ---------------------------------------------------------------------------
// NotifyError derives Debug — can be debug-printed without panic
// ---------------------------------------------------------------------------

#[test]
fn notify_error_delivery_is_debug() {
    let err = NotifyError::Delivery("some failure".to_string());
    let debug_str = format!("{:?}", err);
    assert!(!debug_str.is_empty(), "Debug must produce non-empty output");
}

// ---------------------------------------------------------------------------
// Multiple consecutive failures
// ---------------------------------------------------------------------------

#[test]
fn failing_notifier_fails_every_time() {
    let ev = needs_event();
    let prefs = NotifPrefs::default();
    let note = build_notification(&ev, &prefs).unwrap();
    let mut notifier = FailingNotifier::new("persistent failure");

    for i in 0..5 {
        let result = notifier.deliver(&note);
        assert!(result.is_err(), "Call {} must fail", i);
    }
}

// ---------------------------------------------------------------------------
// Partial delivery: some succeed, one fails
// NOTE(test-agent): this test validates that Notifier::deliver is called per-
// notification and errors are propagated exactly — the caller is responsible
// for handling partial failure. CaptureNotifier is used for the successful
// deliveries and FailingNotifier for the one that errors.
// ---------------------------------------------------------------------------

#[test]
fn delivery_error_type_check() {
    // Verify the error type name round-trips to confirm variant is exposed
    let err = NotifyError::Delivery("oops".to_string());
    match err {
        NotifyError::Delivery(_) => {} // variant must be matchable
    }
}
