//! The shell's own fleet subscription: connection state for the webview (`connection`
//! event), OS notifications filtered by the stored preferences, the tray's attention count,
//! and "clicking a notification focuses what it was about".
//!
//! Click handling: desktop notification APIs do not report clicks uniformly, but clicking
//! one brings the app forward (focus, a second launch caught by single-instance, or the tray).
//! The last notified target (a session, or just a host: the one that waits for a sign-in, the
//! one that was lost) is remembered for a few minutes; when the window is brought forward in
//! that time the webview gets a `navigate` event `{host, session?}` for it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use marsad_proto::diag::AppEvent;
use marsad_proto::fleet::{FleetEvent, FleetHost};
use marsad_proto::store::NotificationPrefs;
use marsad_proto::{methods, HostId, SessionId};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use wasit_control::LinkState;
use wasit_diag::{Code, ErrorInfo};

use crate::bridge::Shell;
use crate::connection::ConnectionState;
use crate::diag;
use crate::notify::{Note, Tracker};

/// How long a notification's target stays the target of "bring the app forward".
const FOCUS_WINDOW: Duration = Duration::from_secs(300);

/// Payload of the `navigate` event: a session of `host`, or (no `session`) the host itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Navigate {
    pub host: HostId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
}

/// What the last notification was about.
#[derive(Default)]
pub struct FocusTarget(Mutex<Option<(Navigate, Instant)>>);

impl FocusTarget {
    fn set(&self, n: Navigate) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((n, Instant::now()));
    }

    /// Take the target if it is recent.
    pub fn take(&self) -> Option<Navigate> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .filter(|(_, at)| at.elapsed() < FOCUS_WINDOW)
            .map(|(n, _)| n)
    }
}

/// Show and focus the main window; navigate to what the last notification was about, if recent.
pub fn bring_forward(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if let Err(e) = w
            .show()
            .and_then(|_| w.unminimize())
            .and_then(|_| w.set_focus())
        {
            diag::failed("bring_forward", &e);
        }
    }
    navigate_pending(app);
}

pub fn navigate_pending(app: &AppHandle) {
    if let Some(n) = app.state::<FocusTarget>().take() {
        if let Err(e) = app.emit("navigate", &n) {
            diag::failed("navigate", &e);
        }
    }
}

fn publish(app: &AppHandle, shell: &Shell, next: ConnectionState) {
    let changed = shell.connection.send_if_modified(|c| {
        if *c == next {
            false
        } else {
            *c = next.clone();
            true
        }
    });
    if changed {
        if let Err(e) = app.emit("connection", &next) {
            diag::failed("connection_event", &e);
        }
    }
}

async fn show(app: &AppHandle, shell: &Shell, notes: Vec<Note>) {
    if notes.is_empty() {
        return;
    }
    let prefs: NotificationPrefs = match shell
        .client
        .call_raw(
            None,
            methods::STORE_NOTIFICATIONS_GET,
            serde_json::Value::Null,
        )
        .await
        .map(serde_json::from_value)
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            diag::failed("notification_prefs", &e);
            NotificationPrefs::default()
        }
        Err(e) => {
            diag::failed("notification_prefs", &e);
            NotificationPrefs::default()
        }
    };
    // The Settings page's switches (UI settings in the hub store): do not disturb, and
    // whether to stay quiet while the app is focused (default on).
    let settings = shell
        .client
        .call_raw(None, methods::STORE_SETTINGS_GET, serde_json::Value::Null)
        .await
        .ok()
        .and_then(|v| serde_json::from_value::<marsad_proto::store::Settings>(v).ok())
        .unwrap_or_default();
    let flag = |k: &str, default: bool| {
        settings
            .values
            .get(k)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default)
    };
    let dnd = flag("notify.dnd", false);
    let quiet_when_focused = flag("notify.only_unfocused", true);
    let focused = quiet_when_focused
        && app
            .get_webview_window("main")
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(false);
    for n in notes {
        let suppressed = if !n.kind.enabled(&prefs) {
            Some("prefs")
        } else if dnd {
            Some("dnd")
        } else if focused {
            Some("focus")
        } else {
            None
        };
        record(&n, suppressed);
        if let Some(why) = suppressed {
            diag::emit(notification(&n, why));
            continue;
        }
        app.state::<FocusTarget>().set(Navigate {
            host: n.host.clone(),
            session: n.session.clone(),
        });
        match app
            .notification()
            .builder()
            .title(&n.title)
            .body(&n.body)
            .show()
        {
            Ok(()) => diag::emit(notification(&n, "shown")),
            Err(e) => diag::failed("notification", &e),
        }
    }
}

/// What was decided about a notification (never its title or body: they name sessions).
fn notification(n: &Note, outcome: &'static str) -> AppEvent {
    AppEvent::Notification {
        kind: Code::st(n.kind.code()),
        outcome: Code::st(outcome),
    }
}

/// Test hook: with `$MARSAD_NOTIFY_LOG` set, every notification decision is appended to that
/// file as one JSON line `{kind, title, body, suppressed}` (`suppressed`: `"prefs"`, `"dnd"`,
/// `"focus"` or `null` = shown), so a test can check them without scraping the OS.
fn record(n: &Note, suppressed: Option<&str>) {
    let Some(path) = std::env::var_os("MARSAD_NOTIFY_LOG") else {
        return;
    };
    let line = serde_json::json!({
        "kind": format!("{:?}", n.kind),
        "title": n.title,
        "body": n.body,
        "suppressed": suppressed,
    });
    use std::io::Write;
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = r {
        diag::failed("notification_log", &e);
    }
}

/// Run the watcher (forever).
pub async fn run(app: AppHandle) {
    let shell = app.state::<Shell>();
    let mut sub = shell.client.fleet(None);
    let mut link = shell.client.link().state();
    let mut tracker = Tracker::default();
    let mut hosts: Vec<FleetHost> = Vec::new();
    let mut daemon: LinkState = link.borrow_and_update().clone();
    diag::emit(diag::hub_link(&daemon));
    // The webview reads `connection_state` at start: it must not see the placeholder while
    // the link is already up (or already failed).
    publish(&app, &shell, ConnectionState::new(&daemon, &hosts));
    loop {
        tokio::select! {
            item = sub.next() => {
                let ev = match item {
                    Some(Ok((_, ev))) => ev,
                    Some(Err(e)) => {
                        // From here on no notification, no tray count, no host link updates.
                        ended("fleet_failed", Some(diag::foreign(&e.to_string())));
                        return;
                    }
                    None => {
                        ended("fleet_closed", None);
                        return;
                    }
                };
                apply(&mut hosts, &ev);
                let notes = tracker.apply(&ev);
                crate::tray::set_attention(&app, tracker.attention());
                publish(&app, &shell, ConnectionState::new(&daemon, &hosts));
                show(&app, &shell, notes).await;
            }
            changed = link.changed() => {
                if changed.is_err() {
                    ended("link_closed", None);
                    return;
                }
                daemon = link.borrow_and_update().clone();
                diag::emit(diag::hub_link(&daemon));
                publish(&app, &shell, ConnectionState::new(&daemon, &hosts));
            }
        }
    }
}

/// The watcher stopped for good: nothing updates the tray, the notifications or the host links
/// any more, so the app log must say so (`app.watcher_ended`).
fn ended(reason: &'static str, error: Option<ErrorInfo>) {
    diag::emit(AppEvent::WatcherEnded {
        reason: Code::st(reason),
        error,
    });
}

/// Keep the host list (with links) current.
fn apply(hosts: &mut Vec<FleetHost>, ev: &FleetEvent) {
    match ev {
        FleetEvent::Reset { hosts: h } => *hosts = h.clone(),
        FleetEvent::HostUpserted { host } => {
            match hosts.iter_mut().find(|h| h.host.id == host.id) {
                Some(h) => h.host = host.clone(),
                None => hosts.push(FleetHost {
                    host: host.clone(),
                    link: marsad_proto::fleet::FleetLink::Remote {
                        state: marsad_proto::LinkState::Connecting,
                    },
                    sessions_live: false,
                    sessions: Vec::new(),
                    auth_prompt: None,
                    git: Vec::new(),
                }),
            }
        }
        FleetEvent::HostRemoved { id } => hosts.retain(|h| h.host.id != *id),
        FleetEvent::Link { host, state } => {
            if let Some(h) = hosts.iter_mut().find(|h| h.host.id == *host) {
                h.link = marsad_proto::fleet::FleetLink::Remote {
                    state: state.clone(),
                };
            }
        }
        // Sessions are not part of the connection state.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_target_carries_no_session_and_a_session_target_does() {
        let host = Navigate {
            host: HostId::new("gpu"),
            session: None,
        };
        assert_eq!(
            serde_json::to_value(&host).expect("json"),
            serde_json::json!({"host": "gpu"})
        );
        let id = SessionId::random();
        let session = Navigate {
            host: HostId::new("gpu"),
            session: Some(id.clone()),
        };
        assert_eq!(
            serde_json::to_value(&session).expect("json"),
            serde_json::json!({"host": "gpu", "session": id})
        );
    }

    #[test]
    fn the_last_notification_is_the_target_once() {
        let t = FocusTarget::default();
        assert_eq!(t.take(), None);
        let n = Navigate {
            host: HostId::new("gpu"),
            session: None,
        };
        t.set(n.clone());
        assert_eq!(t.take(), Some(n));
        assert_eq!(
            t.take(),
            None,
            "bringing the window forward again goes nowhere"
        );
    }
}
