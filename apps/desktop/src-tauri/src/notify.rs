//! Which fleet changes deserve an OS notification (the single notifier, ARCHITECTURE §7):
//! a session that starts needing you, fails, or finishes its work, and a remote host whose
//! link is lost. Pure: [`Tracker::apply`] turns fleet events into [`Note`]s; the watcher
//! filters them by the stored preferences and shows them.

use std::collections::{BTreeMap, HashMap};

use marsad_proto::fleet::{FleetEvent, FleetHost, FleetLink};
use marsad_proto::store::NotificationPrefs;
use marsad_proto::{HostId, SessionId, SessionSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    NeedsYou,
    Failed,
    Finished,
    HostLost,
}

impl Kind {
    pub fn enabled(self, p: &NotificationPrefs) -> bool {
        match self {
            Kind::NeedsYou => p.needs_you,
            Kind::Failed => p.failed,
            Kind::Finished => p.finished,
            Kind::HostLost => p.host_lost,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub kind: Kind,
    pub host: HostId,
    pub session: Option<SessionId>,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Busy,
    Waiting,
    NeedsYou,
    Failed,
    Done,
}

fn state(s: &SessionSnapshot) -> State {
    match s.phase.name() {
        "starting" | "working" => State::Busy,
        "needs_you" => State::NeedsYou,
        "failed" | "lost" => State::Failed,
        "exited" => State::Done,
        _ => State::Waiting,
    }
}

fn label(s: &SessionSnapshot) -> String {
    s.title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| format!("{} session {}", s.driver, s.id))
}

/// See the module docs.
#[derive(Default)]
pub struct Tracker {
    sessions: HashMap<(HostId, SessionId), State>,
    connected: HashMap<HostId, bool>,
    names: BTreeMap<HostId, String>,
    /// Sessions currently needing an answer (for the tray).
    needs_you: HashMap<(HostId, SessionId), ()>,
}

impl Tracker {
    /// How many sessions need an answer.
    pub fn attention(&self) -> usize {
        self.needs_you.len()
    }

    fn host_name(&self, h: &HostId) -> String {
        self.names.get(h).cloned().unwrap_or_else(|| h.to_string())
    }

    fn baseline(&mut self, host: &FleetHost) {
        self.names
            .insert(host.host.id.clone(), host.host.name.clone());
        if let FleetLink::Remote { state } = &host.link {
            self.connected
                .insert(host.host.id.clone(), state.is_connected());
        }
        for s in &host.sessions {
            self.set(&host.host.id, s);
        }
    }

    fn set(&mut self, host: &HostId, s: &SessionSnapshot) -> Option<State> {
        let key = (host.clone(), s.id.clone());
        let now = state(s);
        if now == State::NeedsYou {
            self.needs_you.insert(key.clone(), ());
        } else {
            self.needs_you.remove(&key);
        }
        self.sessions.insert(key, now)
    }

    fn session(&mut self, host: &HostId, s: &SessionSnapshot) -> Option<Note> {
        let before = self.set(host, s)?;
        let now = state(s);
        if before == now {
            return None;
        }
        let where_ = self.host_name(host);
        let (kind, title, body) = match now {
            State::NeedsYou => (
                Kind::NeedsYou,
                format!("{} is waiting for your answer", label(s)),
                where_,
            ),
            State::Failed => (Kind::Failed, format!("{} failed", label(s)), where_),
            State::Waiting | State::Done if before == State::Busy => {
                (Kind::Finished, format!("{} finished", label(s)), where_)
            }
            _ => return None,
        };
        Some(Note {
            kind,
            host: host.clone(),
            session: Some(s.id.clone()),
            title,
            body,
        })
    }

    /// Fold one fleet event; the notes it warrants.
    pub fn apply(&mut self, ev: &FleetEvent) -> Vec<Note> {
        let mut out = Vec::new();
        match ev {
            FleetEvent::Reset { hosts } => {
                *self = Tracker::default();
                for h in hosts {
                    self.baseline(h);
                }
            }
            FleetEvent::HostUpserted { host } => {
                self.names.insert(host.id.clone(), host.name.clone());
            }
            FleetEvent::HostRemoved { id } => {
                self.connected.remove(id);
                self.sessions.retain(|(h, _), _| h != id);
                self.needs_you.retain(|(h, _), _| h != id);
            }
            FleetEvent::Link { host, state } => {
                let up = state.is_connected();
                let was = self.connected.insert(host.clone(), up);
                if was == Some(true) && !up {
                    out.push(Note {
                        kind: Kind::HostLost,
                        host: host.clone(),
                        session: None,
                        title: format!("Lost the connection to {}", self.host_name(host)),
                        body: "Sessions keep running there; Marsad reconnects when it can.".into(),
                    });
                }
            }
            FleetEvent::Sessions { host, sessions } => {
                for s in sessions {
                    out.extend(self.session(host, s));
                }
            }
            FleetEvent::SessionUpserted { host, session } => {
                out.extend(self.session(host, session));
            }
            FleetEvent::SessionRemoved { host, id } => {
                let key = (host.clone(), id.clone());
                self.sessions.remove(&key);
                self.needs_you.remove(&key);
            }
            FleetEvent::AuthPrompt { .. } => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use marsad_proto::hosts::{HostEntry, HostKind};
    use marsad_proto::runtime::Phase;
    use marsad_proto::LinkState;

    fn snap(phase: Phase) -> SessionSnapshot {
        let caps: serde_json::Map<String, serde_json::Value> = [
            "interrupt",
            "set_model",
            "set_mode",
            "slash_commands",
            "permission_gates",
            "questions",
            "plan_gate",
            "usage",
            "cost",
            "resume",
            "continue_latest",
            "streaming_output",
        ]
        .iter()
        .map(|k| (k.to_string(), serde_json::Value::Bool(false)))
        .chain([("prompt_while_busy".to_string(), "queued_by_cli".into())])
        .collect();
        serde_json::from_value(serde_json::json!({
            "id": "aaaaaaaaaaaa", "driver": "claude", "created_at_ms": 0,
            "capabilities": caps, "phase": phase, "generation": 1,
            "turns_completed": 0, "usage": {"input_tokens":0,"output_tokens":0,
            "cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.0},
            "last_activity_ms": 0, "cursor": 0, "loaded": true
        }))
        .expect("snapshot")
    }

    #[test]
    fn transitions_become_notes() {
        let h = HostId::new("box");
        let mut t = Tracker::default();
        t.apply(&FleetEvent::Reset {
            hosts: vec![FleetHost {
                host: HostEntry {
                    id: h.clone(),
                    name: "Box".into(),
                    kind: HostKind::Wsl { distro: "x".into() },
                    platform: None,
                    created_at_ms: 0,
                    interactive_auth: false,
                },
                link: FleetLink::Remote {
                    state: LinkState::Connecting,
                },
                sessions_live: true,
                sessions: vec![snap(Phase::Working)],
                auth_prompt: None,
            }],
        });
        let up = |p| FleetEvent::SessionUpserted {
            host: h.clone(),
            session: Box::new(snap(p)),
        };
        let n = t.apply(&up(Phase::NeedsYou));
        assert_eq!(n[0].kind, Kind::NeedsYou);
        assert_eq!(t.attention(), 1);
        assert!(t.apply(&up(Phase::NeedsYou)).is_empty(), "no repeat");
        assert!(t.apply(&up(Phase::Working)).is_empty());
        assert_eq!(t.attention(), 0);
        assert_eq!(t.apply(&up(Phase::Idle))[0].kind, Kind::Finished);
        let failed = Phase::Failed {
            reason: "x".into(),
            stderr_tail: vec![],
        };
        assert_eq!(t.apply(&up(failed))[0].kind, Kind::Failed);
        assert!(t
            .apply(&FleetEvent::Link {
                host: h.clone(),
                state: LinkState::Stopped
            })
            .is_empty());
        let connected: LinkState = serde_json::from_value(serde_json::json!({
            "state": "connected",
            "hello": {"protocol": {"major": 1, "minor": 0}, "build": "b", "host_id": "box", "role": "daemon"}
        }))
        .expect("link state");
        t.apply(&FleetEvent::Link {
            host: h.clone(),
            state: connected,
        });
        let lost = t.apply(&FleetEvent::Link {
            host: h.clone(),
            state: LinkState::Failed {
                reason: "gone".into(),
            },
        });
        assert_eq!(lost[0].kind, Kind::HostLost);
        assert!(lost[0].title.contains("Box"));
        let p = NotificationPrefs {
            finished: false,
            ..NotificationPrefs::default()
        };
        assert!(!Kind::Finished.enabled(&p));
        assert!(Kind::HostLost.enabled(&p));
    }
}
