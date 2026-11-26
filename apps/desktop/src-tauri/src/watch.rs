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

/// Whether a fleet subscription that is not running is started (again) now that the hub link is
/// in `link`. A fleet subscription that failed (the daemon refused the topic, or closed it) is not
/// retried on a timer: the hub link coming back is the event that starts it again.
pub(crate) fn resubscribes_on(link: &LinkState) -> bool {
    link.is_connected()
}

/// What happens to the fleet subscription after it failed or closed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AfterFailure {
    /// Started again now: the failure itself is the event that says the topic must be asked again.
    Resubscribe,
    /// Not started until the hub link changes.
    WaitForLink,
}

/// Decide what a failed or closed fleet subscription does next.
///
/// `silent_restart` says the subscription that failed was itself started by a failure and
/// delivered nothing. The daemon would most likely fail it again, so that is not asked again at
/// once: the hub link's next change starts it. This bounds the restarts to one per link state, so
/// no loop of refused subscriptions can run without an event between them.
pub(crate) fn after_failure(link: &LinkState, silent_restart: bool) -> AfterFailure {
    if resubscribes_on(link) && !silent_restart {
        AfterFailure::Resubscribe
    } else {
        AfterFailure::WaitForLink
    }
}

/// Run the watcher (forever, unless the link to the daemon itself is gone).
pub async fn run(app: AppHandle) {
    let shell = app.state::<Shell>();
    // `None` while the fleet subscription is down: after a failure it is started again at once
    // (see [`after_failure`]), otherwise when the hub link is back.
    let mut sub = Some(shell.client.fleet(None));
    // The current subscription was started by a failure, and has delivered nothing since.
    let mut silent_restart = false;
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
            item = async {
                match sub.as_mut() {
                    Some(s) => s.next().await,
                    None => None,
                }
            }, if sub.is_some() => {
                let ev = match item {
                    Some(Ok((_, ev))) => ev,
                    failed => {
                        // No notification, tray count or host link update until the subscription
                        // is back; the reason goes to the app log.
                        match failed {
                            Some(Err(e)) => diag::failed("fleet_feed", &e),
                            _ => diag::failed("fleet_feed", &"the daemon closed the fleet topic"),
                        }
                        let again = after_failure(&daemon, silent_restart);
                        silent_restart = again == AfterFailure::Resubscribe;
                        sub = match again {
                            AfterFailure::Resubscribe => Some(shell.client.fleet(None)),
                            AfterFailure::WaitForLink => None,
                        };
                        continue;
                    }
                };
                silent_restart = false;
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
                // A fresh subscription starts with a `reset` of the hosts, which replaces what is held.
                if sub.is_none() && resubscribes_on(&daemon) {
                    sub = Some(shell.client.fleet(None));
                    silent_restart = false;
                }
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
                        state: marsad_proto::LinkState::Connecting {
                            stage: None,
                            since_ms: 0,
                        },
                    },
                    sessions_live: false,
                    sessions: Vec::new(),
                    auth_prompt: None,
                    git: Vec::new(),
                    feed_error: None,
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
        // The hub decides whether a host's sessions are live; this only records it.
        FleetEvent::SessionsLive { host, live } => {
            if let Some(h) = hosts.iter_mut().find(|h| h.host.id == *host) {
                h.sessions_live = *live;
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
    fn a_failed_fleet_feed_is_started_again_only_when_the_hub_link_is_connected() {
        let hello = wasit_control::Hello::new("test", "marsad-desktop", "gui");
        assert!(resubscribes_on(&LinkState::Connected { hello }));
        assert!(!resubscribes_on(&LinkState::Connecting {
            stage: None,
            since_ms: 0
        }));
        assert!(!resubscribes_on(&LinkState::Failed {
            reason: "cannot start marsad-agent".into(),
            problem: None,
        }));
    }

    #[test]
    fn a_failed_fleet_subscription_is_started_again_on_the_error_itself() {
        let hello = wasit_control::Hello::new("test", "marsad-desktop", "gui");
        let up = LinkState::Connected { hello };
        // The error is the event: the subscription is asked again without waiting for the link.
        assert_eq!(after_failure(&up, false), AfterFailure::Resubscribe);
        // The daemon failed the fresh subscription before it delivered anything: asked again only
        // when the link changes, so refusals cannot follow each other without an event between.
        assert_eq!(after_failure(&up, true), AfterFailure::WaitForLink);
        // Without a connected hub link there is nothing to ask.
        assert_eq!(
            after_failure(
                &LinkState::Connecting {
                    stage: None,
                    since_ms: 0
                },
                false
            ),
            AfterFailure::WaitForLink
        );
    }

    #[test]
    fn the_hub_decides_whether_a_hosts_sessions_are_live() {
        let entry = |id: &str| marsad_proto::hosts::HostEntry {
            id: HostId::new(id),
            name: id.to_string(),
            kind: marsad_proto::hosts::HostKind::Wsl {
                distro: "Ubuntu".into(),
            },
            platform: None,
            created_at_ms: 0,
            interactive_auth: false,
            ask_before_connecting: false,
            local_files: false,
            shared_connection: false,
            signed_out: false,
            machine: None,
        };
        let mut hosts = Vec::new();
        apply(&mut hosts, &FleetEvent::HostUpserted { host: entry("gpu") });
        // Added as not live: nothing marks it live until the hub says so.
        assert!(!hosts[0].sessions_live);
        apply(
            &mut hosts,
            &FleetEvent::SessionsLive {
                host: HostId::new("gpu"),
                live: true,
            },
        );
        assert!(hosts[0].sessions_live);
        apply(
            &mut hosts,
            &FleetEvent::SessionsLive {
                host: HostId::new("gpu"),
                live: false,
            },
        );
        assert!(
            !hosts[0].sessions_live,
            "a lost feed is applied as the hub sent it"
        );
        // A flag for an unknown host changes nothing.
        apply(
            &mut hosts,
            &FleetEvent::SessionsLive {
                host: HostId::new("other"),
                live: true,
            },
        );
        assert_eq!(hosts.len(), 1);
        assert!(!hosts[0].sessions_live);
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
