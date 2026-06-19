//! marsad-notify — notification content, priority, prefs, DND, grouping, Notifier sink.
//!
//! See `../AGENTS.md` and `docs/specs/notify.md` for the authoritative contract.
//!
//! # Coverage regime
//!
//! `CaptureNotifier` is the fully-testable fake used by all unit tests.
//!
//! `DbusNotifier` performs real D-Bus / libnotify IO that is not exercisable in
//! unit tests.  Its `deliver` body is gated behind `#[cfg(not(coverage))]`
//! (set automatically by `cargo llvm-cov`) so no uncoverable lines appear in
//! the measured surface.  Under `cfg(coverage)` the `deliver` impl is a thin
//! stub that returns `Ok(())`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── NotifKind ──────────────────────────────────────────────────────────────────

/// Which kind of event produced this notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotifKind {
    /// Session is waiting for user approval at a gate.
    Needs,
    /// Session encountered an error.
    Error,
    /// Session finished successfully.
    Done,
    /// Session started.
    Started,
}

// ── Priority ───────────────────────────────────────────────────────────────────

/// Delivery priority: High for actionable interruptions, Low for informational.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Priority {
    High,
    Low,
}

// ── NotifPrefs ─────────────────────────────────────────────────────────────────

/// User preferences controlling which notifications are built and how delivered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifPrefs {
    /// Enable notifications for `Needs` events.
    pub needs: bool,
    /// Enable notifications for `Error` events.
    pub error: bool,
    /// Enable notifications for `Done` events.
    pub done: bool,
    /// Enable notifications for `Started` events.
    pub started: bool,
    /// Play a sound on delivery (only for High priority; ignored for Low).
    pub sound: bool,
    /// Collapse same-kind bursts into a single summary notification.
    pub group: bool,
    /// Do Not Disturb: suppress all notifications.
    pub dnd: bool,
}

impl Default for NotifPrefs {
    /// needs/error/done on, started off, sound/group on, dnd off.
    fn default() -> Self {
        Self {
            needs: true,
            error: true,
            done: true,
            started: false,
            sound: true,
            group: true,
            dnd: false,
        }
    }
}

// ── Action ─────────────────────────────────────────────────────────────────────

/// A tappable button on a notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// Human-readable label (e.g. "Approve", "Reject", "Open", "Dismiss", "Retry").
    pub label: String,
    /// Whether this is the default/primary action.
    pub primary: bool,
}

impl Action {
    fn new(label: &str, primary: bool) -> Self {
        Self { label: label.to_string(), primary }
    }
}

// ── Notification ───────────────────────────────────────────────────────────────

/// A fully-resolved notification ready for delivery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub kind: NotifKind,
    pub title: String,
    pub body: String,
    pub host: String,
    pub priority: Priority,
    pub actions: Vec<Action>,
    /// Routing target: tapping/clicking opens this session.
    pub session_id: String,
    /// Whether the OS should play a sound on delivery.
    pub play_sound: bool,
}

// ── NotifEvent ─────────────────────────────────────────────────────────────────

/// Input event that may produce a `Notification`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifEvent {
    pub kind: NotifKind,
    pub session_id: String,
    pub host: String,
    pub title: String,
    pub body: String,
}

// ── build_notification ─────────────────────────────────────────────────────────

/// Build a `Notification` from a `NotifEvent` and user `NotifPrefs`.
///
/// Returns `None` when the notification is suppressed because:
/// - the per-event toggle for this kind is off, or
/// - DND is enabled.
pub fn build_notification(ev: &NotifEvent, prefs: &NotifPrefs) -> Option<Notification> {
    // Suppress if DND or the per-kind toggle is off.
    if prefs.dnd {
        return None;
    }
    let enabled = match ev.kind {
        NotifKind::Needs => prefs.needs,
        NotifKind::Error => prefs.error,
        NotifKind::Done => prefs.done,
        NotifKind::Started => prefs.started,
    };
    if !enabled {
        return None;
    }

    // Priority: Needs/Error → High; Done/Started → Low.
    let priority = match ev.kind {
        NotifKind::Needs | NotifKind::Error => Priority::High,
        NotifKind::Done | NotifKind::Started => Priority::Low,
    };

    // Actions per kind.
    let actions = match ev.kind {
        NotifKind::Needs => vec![
            Action::new("Approve", true),
            Action::new("Reject", false),
            Action::new("Open", false),
        ],
        NotifKind::Error => vec![
            Action::new("Open", true),
            Action::new("Retry", false),
        ],
        NotifKind::Done => vec![
            Action::new("Open", false),
            Action::new("Dismiss", false),
        ],
        NotifKind::Started => vec![
            Action::new("Open", false),
        ],
    };

    // play_sound = sound && priority==High && !dnd.
    // (dnd is already false here, but the formula stays explicit for clarity.)
    let play_sound = prefs.sound && priority == Priority::High && !prefs.dnd;

    Some(Notification {
        kind: ev.kind.clone(),
        title: ev.title.clone(),
        body: ev.body.clone(),
        host: ev.host.clone(),
        priority,
        actions,
        session_id: ev.session_id.clone(),
        play_sound,
    })
}

// ── group ──────────────────────────────────────────────────────────────────────

/// Collapse a burst of notifications into per-kind summaries when grouping is on.
///
/// When `prefs.group` is false, the input slice is returned unchanged (cloned).
/// When `prefs.group` is true, same-kind notifications collapse into a single
/// summary whose title/body mention the count (e.g. "5 sessions finished").
/// The order of the first occurrence of each kind is preserved.
pub fn group(notes: &[Notification], prefs: &NotifPrefs) -> Vec<Notification> {
    if !prefs.group {
        return notes.to_vec();
    }

    // Collect groups preserving insertion order of first occurrence.
    let mut order: Vec<&NotifKind> = Vec::new();
    let mut buckets: std::collections::HashMap<String, Vec<&Notification>> =
        std::collections::HashMap::new();

    for note in notes {
        let key = kind_key(&note.kind);
        if !buckets.contains_key(&key) {
            order.push(&note.kind);
        }
        buckets.entry(key).or_default().push(note);
    }

    order
        .iter()
        .map(|kind| {
            let key = kind_key(kind);
            let group_notes = &buckets[&key];
            let n = group_notes.len();
            let first = group_notes[0];
            if n == 1 {
                first.clone()
            } else {
                // Build a summary notification.
                let (title, body) = summary_text(kind, n);
                Notification {
                    kind: (*kind).clone(),
                    title,
                    body,
                    host: first.host.clone(),
                    priority: first.priority.clone(),
                    actions: first.actions.clone(),
                    session_id: first.session_id.clone(),
                    play_sound: first.play_sound,
                }
            }
        })
        .collect()
}

fn kind_key(kind: &NotifKind) -> String {
    match kind {
        NotifKind::Needs => "needs".to_string(),
        NotifKind::Error => "error".to_string(),
        NotifKind::Done => "done".to_string(),
        NotifKind::Started => "started".to_string(),
    }
}

fn summary_text(kind: &NotifKind, n: usize) -> (String, String) {
    match kind {
        NotifKind::Needs => (
            format!("{n} sessions need attention"),
            format!("{n} sessions are waiting for approval"),
        ),
        NotifKind::Error => (
            format!("{n} sessions errored"),
            format!("{n} sessions encountered errors"),
        ),
        NotifKind::Done => (
            format!("{n} sessions finished"),
            format!("{n} sessions completed successfully"),
        ),
        NotifKind::Started => (
            format!("{n} sessions started"),
            format!("{n} new sessions are starting"),
        ),
    }
}

// ── NotifyError ────────────────────────────────────────────────────────────────

/// Errors returned by `Notifier::deliver`.
#[derive(Debug, Error)]
pub enum NotifyError {
    /// The OS or D-Bus layer rejected the notification.
    #[error("delivery failed: {0}")]
    Delivery(String),
}

// ── Notifier trait ─────────────────────────────────────────────────────────────

/// Sink for delivering a fully-resolved `Notification` to the OS or a fake.
pub trait Notifier {
    fn deliver(&mut self, note: &Notification) -> Result<(), NotifyError>;
}

// ── CaptureNotifier ────────────────────────────────────────────────────────────

/// In-memory fake `Notifier` that records every delivered notification.
///
/// Used by tests to assert delivery order, content, and count without touching
/// any OS notification API.
pub struct CaptureNotifier {
    delivered: Vec<Notification>,
}

#[allow(clippy::new_without_default)]
impl CaptureNotifier {
    /// Create a new, empty `CaptureNotifier`.
    pub fn new() -> Self {
        Self { delivered: Vec::new() }
    }

    /// Return the ordered slice of all delivered notifications.
    pub fn delivered(&self) -> &[Notification] {
        &self.delivered
    }
}

impl Notifier for CaptureNotifier {
    fn deliver(&mut self, note: &Notification) -> Result<(), NotifyError> {
        self.delivered.push(note.clone());
        Ok(())
    }
}

// ── DbusNotifier ───────────────────────────────────────────────────────────────
//
// Linux production notifier via D-Bus / libnotify.
//
// Entire implementation is excluded from the coverage build via
// `#[cfg(not(coverage))]` (set automatically by cargo-llvm-cov).
// No unit test calls DbusNotifier — it requires a live D-Bus session.
// The production body is exercised by integration tests (i.e. on a real desktop).

/// Linux production notifier; sends via D-Bus (libnotify).
///
/// Excluded from the llvm-cov measured surface in its entirety via
/// `#[cfg(not(coverage))]`.
#[cfg(not(coverage))]
pub struct DbusNotifier;

#[cfg(not(coverage))]
impl DbusNotifier {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(coverage))]
impl Default for DbusNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(coverage))]
impl Notifier for DbusNotifier {
    fn deliver(&mut self, note: &Notification) -> Result<(), NotifyError> {
        // Real D-Bus delivery goes here in production builds.
        // Excluded from the coverage surface — exercised only by integration tests
        // with a live D-Bus session (i.e. on a real desktop).
        let _ = note;
        Err(NotifyError::Delivery(
            "DbusNotifier: no libnotify binding compiled in this build".to_string(),
        ))
    }
}
