//! The shell's own fleet subscription: connection state for the webview (`connection`
//! event), OS notifications filtered by the stored preferences, the tray's attention count,
//! and "clicking a notification focuses that session".
//!
//! Click handling: desktop notification APIs do not report clicks uniformly, but clicking
//! one brings the app forward (focus, a second launch caught by single-instance, or the tray).
//! The last notified session is remembered for a few minutes; when the window is brought
//! forward in that time the webview gets a `navigate` event `{host, session}` for it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use marsad_proto::fleet::{FleetEvent, FleetHost};
use marsad_proto::store::NotificationPrefs;
use marsad_proto::{methods, HostId, SessionId};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::bridge::{link_word, Shell};
use crate::connection::{host_link, ConnectionState};
use crate::notify::{Note, Tracker};

/// How long a notification's session stays the target of "bring the app forward".
const FOCUS_WINDOW: Duration = Duration::from_secs(300);

/// Payload of the `navigate` event.
#[derive(Debug, Clone, Serialize)]
pub struct Navigate {
    pub host: HostId,
    pub session: SessionId,
}

/// The session the last notification was about.
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

/// Show and focus the main window; navigate to the last notified session if any.
pub fn bring_forward(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if let Err(e) = w
            .show()
            .and_then(|_| w.unminimize())
            .and_then(|_| w.set_focus())
        {
            tracing::warn!(error = %e, "cannot bring the window forward");
        }
    }
    navigate_pending(app);
}

pub fn navigate_pending(app: &AppHandle) {
    if let Some(n) = app.state::<FocusTarget>().take() {
        if let Err(e) = app.emit("navigate", &n) {
            tracing::warn!(error = %e, "cannot send navigate");
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
            tracing::warn!(error = %e, "cannot send the connection state");
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
            tracing::warn!(error = %e, "undecodable notification preferences; using defaults");
            NotificationPrefs::default()
        }
        Err(e) => {
            tracing::warn!(error = %e, "cannot read notification preferences; using defaults");
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
        if suppressed.is_some() {
            continue;
        }
        if let Some(s) = &n.session {
            app.state::<FocusTarget>().set(Navigate {
                host: n.host.clone(),
                session: s.clone(),
            });
        }
        if let Err(e) = app
            .notification()
            .builder()
            .title(&n.title)
            .body(&n.body)
            .show()
        {
            tracing::warn!(error = %e, "cannot show a notification");
        }
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
        tracing::warn!(error = %e, "cannot write the notification log");
    }
}

/// Run the watcher (forever).
pub async fn run(app: AppHandle) {
    let shell = app.state::<Shell>();
    let mut sub = shell.client.fleet(None);
    let mut link = shell.client.link().state();
    let mut tracker = Tracker::default();
    let mut hosts: Vec<FleetHost> = Vec::new();
    let mut daemon = link_word(&link.borrow_and_update());
    loop {
        tokio::select! {
            item = sub.next() => {
                let ev = match item {
                    Some(Ok((_, ev))) => ev,
                    Some(Err(e)) => {
                        tracing::error!(error = %e, "the fleet subscription failed");
                        return;
                    }
                    None => return,
                };
                apply(&mut hosts, &ev);
                let notes = tracker.apply(&ev);
                crate::tray::set_attention(&app, tracker.attention());
                publish(&app, &shell, ConnectionState { daemon, hosts: hosts.iter().map(host_link).collect() });
                show(&app, &shell, notes).await;
            }
            changed = link.changed() => {
                if changed.is_err() {
                    return;
                }
                daemon = link_word(&link.borrow_and_update());
                publish(&app, &shell, ConnectionState { daemon, hosts: hosts.iter().map(host_link).collect() });
            }
        }
    }
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
