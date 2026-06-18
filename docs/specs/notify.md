# B10 — `marsad-notify`

**Tier:** L1. **Mode:** one-shot build + sink. **Coverage:** 100% line+region (via `CaptureNotifier`
fake; real D-Bus/libnotify delivery is desktop glue, not unit-covered).

## Purpose

Builds notification content/priority from events + user prefs, applies DND/grouping/sound gating, and
delivers via a `Notifier` trait. Port of `mkPopup` (desktop) / `mkPush` (mobile) content + the
per-event prefs model. The app owns content/actions/priority; the OS owns the chrome.

## Public API (authoritative)

```rust
use marsad_events::HookEvent;

pub enum NotifKind { Needs, Error, Done, Started }
pub enum Priority { High, Low }

pub struct NotifPrefs {
    pub needs: bool, pub error: bool, pub done: bool, pub started: bool, // per-event toggles
    pub sound: bool, pub group: bool, pub dnd: bool,                     // delivery
}
impl Default for NotifPrefs { /* needs/error/done on, started off, sound/group on, dnd off */ }

pub struct Action { pub label: String, pub primary: bool }              // e.g. Approve / Reject / Open / Dismiss
pub struct Notification {
    pub kind: NotifKind,
    pub title: String,
    pub body: String,
    pub host: String,
    pub priority: Priority,
    pub actions: Vec<Action>,
    pub session_id: String,        // tap routes here
    pub play_sound: bool,
}

pub struct NotifEvent { pub kind: NotifKind, pub session_id: String, pub host: String, pub title: String, pub body: String }

/// Returns None when suppressed (event toggle off, or DND on).
pub fn build_notification(ev: &NotifEvent, prefs: &NotifPrefs) -> Option<Notification>;

/// Collapse a burst into one when grouping is enabled (e.g. "5 sessions finished").
pub fn group(notes: &[Notification], prefs: &NotifPrefs) -> Vec<Notification>;

pub trait Notifier { fn deliver(&mut self, note: &Notification) -> Result<(), NotifyError>; }
pub struct CaptureNotifier { /* records delivered */ }
impl CaptureNotifier { pub fn new() -> Self; pub fn delivered(&self) -> &[Notification]; }
pub struct DbusNotifier; // Linux production (libnotify/D-Bus)

pub enum NotifyError { Delivery(String) }
```

Derive `Debug, Clone, PartialEq` (+ serde) on data types.

## Behavior notes (port `mkPopup`/`mkPush`)

- Priority: Needs/Error → High; Done/Started → Low.
- Actions per kind: Needs → [Approve(primary), Reject, Open]; Error → [Open(primary), Retry];
  Done → [Open, Dismiss]; Started → [Open].
- `build_notification` returns `None` if the matching event toggle is off **or** `dnd` is true.
- `play_sound` = `prefs.sound && priority==High && !dnd`.
- `group`: when `prefs.group`, collapse same-kind bursts into a single summary notification
  ("N sessions finished"); otherwise return input unchanged.

## Test buckets (`tests/`)

- `happy.rs` — build one of each kind with default prefs; deliver via `CaptureNotifier`.
- `errors.rs` — `Delivery` failure surfaced.
- `branches.rs` — each toggle off → None; DND → None + no sound; sound gating by priority; grouping on
  vs off; mixed-kind groups.
- `fakes.rs` — `Notifier` contract via `CaptureNotifier`.

## Edit scope

Code-agent: write `crates/marsad-notify/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
