//! Which fleet changes deserve an OS notification (the single notifier, ARCHITECTURE §7):
//! a session that starts needing you, fails, or finishes its work, and a remote host whose
//! link is lost. Pure: [`Tracker::apply`] turns fleet events into [`Note`]s; the watcher
//! filters them by the stored preferences and shows them.
//!
//! When a session has *finished* (decided by [`verdict`], from the snapshots alone, no timer).
//! The engine's phase is `working` while a turn is open **or** a background task runs, so a
//! session that reads `idle` has no turn and no background task. A turn that ends while
//! background work continues therefore raises nothing (the phase never left `working`). From a
//! busy phase to a quiet one:
//!
//! | before                        | now      | note                                              |
//! |-------------------------------|----------|---------------------------------------------------|
//! | working, a turn open          | idle     | "finished" (none when the turn was interrupted)   |
//! | working, no turn (background) | idle     | none: see below                                   |
//! | starting                      | idle     | none (the handshake is not work)                  |
//! | working / starting            | exited   | "ended" (none when an exit was requested)         |
//!
//! What the events cannot tell: after the last background task ends the session reads `idle`,
//! and the agent usually starts a turn of its own a few milliseconds later; its completion is
//! then the real end and raises the one "finished" note. The agent gives no advance signal, so
//! nothing here can distinguish that gap from "the agent will not continue": in the second case
//! no "finished" note is raised at all. To close it the engine would have to say, per ended
//! task, whether the agent will report back (e.g. a snapshot count of ended tasks whose result
//! the agent has not yet turned over to the model, or the agent's own session-idle report):
//! `idle` with that count at zero would be the true end.

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
    /// The kind as a diagnostics word.
    pub fn code(self) -> &'static str {
        match self {
            Kind::NeedsYou => "needs_you",
            Kind::Failed => "failed",
            Kind::Finished => "finished",
            Kind::HostLost => "host_lost",
        }
    }

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

/// What the tracker remembers of a session between snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    state: State,
    /// A turn was open (`snapshot.turn`): a busy session without one only has background work (or is starting).
    turn_open: bool,
}

impl Seen {
    fn of(s: &SessionSnapshot) -> Self {
        Seen {
            state: state(s),
            turn_open: s.turn.is_some(),
        }
    }
}

/// How a session's work ended, when it did (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// A turn completed and nothing else runs.
    Finished,
    /// The process exited by itself while working.
    Ended,
}

/// The note for a session that was `before` and now is `now`, when work ended: pure.
fn verdict(before: Seen, now: &SessionSnapshot) -> Option<Verdict> {
    if before.state != State::Busy {
        return None;
    }
    match state(now) {
        State::Waiting if before.turn_open => {
            let interrupted = now.last_turn.as_ref().is_some_and(|t| t.was_interrupted);
            (!interrupted).then_some(Verdict::Finished)
        }
        State::Done => {
            let requested = now.exit.as_ref().is_some_and(|e| e.requested);
            (!requested).then_some(Verdict::Ended)
        }
        _ => None,
    }
}

fn label(s: &SessionSnapshot) -> String {
    s.display_name
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| s.title.clone().filter(|t| !t.is_empty()))
        .unwrap_or_else(|| {
            match s
                .cwd
                .as_deref()
                .and_then(|c| std::path::Path::new(c).file_name())
            {
                // Never the raw session id: the folder the session works in, or the agent.
                Some(dir) => format!("{} session in {}", s.driver, dir.to_string_lossy()),
                None => format!("{} session", s.driver),
            }
        })
}

/// See the module docs.
#[derive(Default)]
pub struct Tracker {
    sessions: HashMap<(HostId, SessionId), Seen>,
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

    fn set(&mut self, host: &HostId, s: &SessionSnapshot) -> Option<Seen> {
        let key = (host.clone(), s.id.clone());
        let now = Seen::of(s);
        if now.state == State::NeedsYou {
            self.needs_you.insert(key.clone(), ());
        } else {
            self.needs_you.remove(&key);
        }
        self.sessions.insert(key, now)
    }

    fn session(&mut self, host: &HostId, s: &SessionSnapshot) -> Option<Note> {
        let before = self.set(host, s)?;
        let now = state(s);
        if before.state == now {
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
            State::Waiting | State::Done => match verdict(before, s)? {
                Verdict::Finished => (Kind::Finished, format!("{} finished", label(s)), where_),
                Verdict::Ended => (Kind::Finished, format!("{} ended", label(s)), where_),
            },
            State::Busy => return None,
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
            FleetEvent::AuthPrompt { .. } | FleetEvent::Git { .. } => {}
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

    /// A snapshot in `phase`; `extra` overrides or adds top-level fields (`turn`, `last_turn`, `exit` ...).
    fn snap_with(phase: Phase, extra: serde_json::Value) -> SessionSnapshot {
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
            "stop_task",
        ]
        .iter()
        .map(|k| (k.to_string(), serde_json::Value::Bool(false)))
        .chain([("prompt_while_busy".to_string(), "queued_by_cli".into())])
        .collect();
        let mut v = serde_json::json!({
            "id": "aaaaaaaaaaaa", "driver": "claude", "created_at_ms": 0,
            "capabilities": caps, "phase": phase, "generation": 1,
            "turns_completed": 0, "usage": {"input_tokens":0,"output_tokens":0,
            "cache_read_tokens":0,"cache_creation_tokens":0,"cost_usd":0.0},
            "last_activity_ms": 0, "cursor": 0, "loaded": true
        });
        v.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        serde_json::from_value(v).expect("snapshot")
    }

    fn snap(phase: Phase) -> SessionSnapshot {
        snap_with(phase, serde_json::json!({}))
    }

    /// A turn that started for `trigger` (`input` or `agent`).
    fn turn(trigger: &str) -> serde_json::Value {
        serde_json::json!({"started_at_ms": 1, "trigger": trigger, "tool_calls": 0})
    }

    /// Working with a turn open.
    fn working_turn(trigger: &str) -> SessionSnapshot {
        snap_with(Phase::Working, serde_json::json!({ "turn": turn(trigger) }))
    }

    /// Working with no turn: only a background task runs.
    fn working_background() -> SessionSnapshot {
        snap_with(Phase::Working, serde_json::json!({"background_running": 1}))
    }

    /// Idle after a turn that ended (`interrupted` or not).
    fn idle_after_turn(interrupted: bool) -> SessionSnapshot {
        snap_with(
            Phase::Idle,
            serde_json::json!({"last_turn": {"ended_at_ms": 2, "was_interrupted": interrupted, "num_turns": 1}}),
        )
    }

    fn exited(requested: bool) -> SessionSnapshot {
        snap_with(
            Phase::Exited { code: Some(0) },
            serde_json::json!({"exit": {"code": 0, "at_ms": 3, "requested": requested}}),
        )
    }

    /// A tracker that knows one host with one session in `first`.
    fn tracker(first: SessionSnapshot) -> (Tracker, HostId) {
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
                    local_files: false,
                    machine: None,
                },
                link: FleetLink::Remote {
                    state: LinkState::Connecting,
                },
                sessions_live: true,
                sessions: vec![first],
                auth_prompt: None,
                git: Vec::new(),
            }],
        });
        (t, h)
    }

    fn feed(t: &mut Tracker, h: &HostId, s: SessionSnapshot) -> Vec<Note> {
        t.apply(&FleetEvent::SessionUpserted {
            host: h.clone(),
            session: Box::new(s),
        })
    }

    fn pure(before: &SessionSnapshot, now: &SessionSnapshot) -> Option<Verdict> {
        verdict(Seen::of(before), now)
    }

    #[test]
    fn a_completed_turn_is_finished() {
        assert_eq!(
            pure(&working_turn("input"), &idle_after_turn(false)),
            Some(Verdict::Finished)
        );
    }

    #[test]
    fn an_interrupted_turn_is_not_finished() {
        assert_eq!(pure(&working_turn("input"), &idle_after_turn(true)), None);
    }

    #[test]
    fn the_end_of_background_work_alone_is_not_finished() {
        // The agent usually starts a turn of its own next; that turn's completion is the end.
        assert_eq!(pure(&working_background(), &idle_after_turn(false)), None);
    }

    #[test]
    fn the_handshake_is_not_finished() {
        assert_eq!(pure(&snap(Phase::Starting), &snap(Phase::Idle)), None);
    }

    #[test]
    fn exits_end_the_work_unless_requested() {
        assert_eq!(pure(&working_turn("input"), &exited(true)), None);
        assert_eq!(
            pure(&working_turn("input"), &exited(false)),
            Some(Verdict::Ended)
        );
        assert_eq!(pure(&working_background(), &exited(true)), None);
    }

    #[test]
    fn nothing_is_decided_from_a_quiet_session() {
        assert_eq!(pure(&snap(Phase::Idle), &working_turn("input")), None);
        assert_eq!(pure(&snap(Phase::Idle), &snap(Phase::Idle)), None);
    }

    /// The whole story of a backgrounded subagent: one note, at the real end.
    #[test]
    fn background_work_and_the_turn_after_it_notify_once() {
        let (mut t, h) = tracker(snap(Phase::Idle));
        let mut notes = Vec::new();
        // A prompt starts a turn; a subagent starts in the background.
        notes.extend(feed(&mut t, &h, working_turn("input")));
        let both = snap_with(
            Phase::Working,
            serde_json::json!({"turn": turn("input"), "background_running": 1}),
        );
        notes.extend(feed(&mut t, &h, both));
        // The turn ends while the subagent runs: the phase stays working.
        notes.extend(feed(&mut t, &h, working_background()));
        // The last background task ends: idle, no turn (the gap).
        notes.extend(feed(&mut t, &h, idle_after_turn(false)));
        // The agent starts a turn of its own and completes it.
        notes.extend(feed(&mut t, &h, working_turn("agent")));
        notes.extend(feed(&mut t, &h, idle_after_turn(false)));
        let kinds: Vec<_> = notes.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, vec![Kind::Finished]);
        assert!(notes[0].title.ends_with("finished"));
    }

    #[test]
    fn notes_say_ended_for_an_unrequested_exit_and_nothing_for_a_requested_one() {
        let (mut t, h) = tracker(working_turn("input"));
        let n = feed(&mut t, &h, exited(false));
        assert_eq!(n.len(), 1);
        assert!(n[0].title.ends_with("ended"), "{}", n[0].title);
        let (mut t, h) = tracker(working_turn("input"));
        assert!(feed(&mut t, &h, exited(true)).is_empty());
        let (mut t, h) = tracker(working_turn("input"));
        assert!(feed(&mut t, &h, idle_after_turn(true)).is_empty());
        let (mut t, h) = tracker(snap(Phase::Starting));
        assert!(feed(&mut t, &h, snap(Phase::Idle)).is_empty());
    }

    #[test]
    fn transitions_become_notes() {
        let (mut t, h) = tracker(working_turn("input"));
        let up = |p| FleetEvent::SessionUpserted {
            host: h.clone(),
            session: Box::new(p),
        };
        let n = t.apply(&up(snap(Phase::NeedsYou)));
        assert_eq!(n[0].kind, Kind::NeedsYou);
        assert_eq!(t.attention(), 1);
        assert!(t.apply(&up(snap(Phase::NeedsYou))).is_empty(), "no repeat");
        assert!(t.apply(&up(working_turn("input"))).is_empty());
        assert_eq!(t.attention(), 0);
        assert_eq!(t.apply(&up(idle_after_turn(false)))[0].kind, Kind::Finished);
        let failed = Phase::Failed {
            reason: "x".into(),
            stderr_tail: vec![],
        };
        assert_eq!(t.apply(&up(snap(failed)))[0].kind, Kind::Failed);
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
                problem: None,
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
