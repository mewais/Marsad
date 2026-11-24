//! Which fleet changes deserve an OS notification (the single notifier, ARCHITECTURE §7):
//! a session that starts needing you, fails, or finishes its work, a remote host whose link is
//! lost, and a remote host that waits for the user to sign in. Pure: [`Tracker::apply`] turns
//! fleet events into [`Note`]s; the watcher filters them by the stored preferences and shows
//! them.
//!
//! What a host's link change deserves ([`link_verdict`]; `was` is whether the host's previous
//! link was connected, `-` = never seen by this tracker, i.e. first sight at app start):
//!
//! | was       | now                                  | note                                           |
//! |-----------|--------------------------------------|------------------------------------------------|
//! | connected | `auth_needed`, `held`                | "needs you to sign in" (the hub will not retry) |
//! | -         | `auth_needed`, `held`                | "needs you to sign in" (first sight)           |
//! | not up    | `auth_needed`, `held`                | none: already announced, or the user just tried |
//! | any       | `auth_needed`, not `held`            | none from `-`/not up; "lost" from connected     |
//! | connected | `reconnecting`/`connecting`/…        | "lost", saying Marsad is reconnecting          |
//! | connected | `failed`/`incompatible`/`stopped`    | "lost", saying Marsad does not retry on its own |
//!
//! So a host that stays held raises one note; a new one needs it to be connected again and
//! dropped again. A user cancelling their own attempt gives `held: false`: nothing. The
//! tracker's memory of "was connected" survives a `Reset` (a re-subscription), so a repeated
//! snapshot of the same held link is not a second announcement.
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

use std::collections::{BTreeMap, BTreeSet, HashMap};

use marsad_proto::fleet::{FleetEvent, FleetHost, FleetLink};
use marsad_proto::store::NotificationPrefs;
use marsad_proto::{HostId, LinkState, SessionId, SessionSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    NeedsYou,
    Failed,
    Finished,
    HostLost,
    /// A host waits for the user at sign-in (`auth_needed`, `held`).
    SignInNeeded,
}

impl Kind {
    /// The kind as a diagnostics word.
    pub fn code(self) -> &'static str {
        match self {
            Kind::NeedsYou => "needs_you",
            Kind::Failed => "failed",
            Kind::Finished => "finished",
            Kind::HostLost => "host_lost",
            Kind::SignInNeeded => "sign_in_needed",
        }
    }

    pub fn enabled(self, p: &NotificationPrefs) -> bool {
        match self {
            Kind::NeedsYou => p.needs_you,
            Kind::Failed => p.failed,
            Kind::Finished => p.finished,
            Kind::HostLost => p.host_lost,
            Kind::SignInNeeded => p.sign_in_needed,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub kind: Kind,
    pub host: HostId,
    /// The session the note is about; none: it is about the host (clicking it opens the host).
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

/// What a host's link change deserves, when anything (see the module docs): pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkVerdict {
    /// The host waits for the user at sign-in; the hub does not retry it.
    SignIn,
    /// A connected host dropped; `retrying`: the hub is reconnecting it by itself.
    Lost { retrying: bool },
}

fn link_verdict(was_connected: Option<bool>, now: &LinkState) -> Option<LinkVerdict> {
    if matches!(now, LinkState::AuthNeeded { held: true, .. }) {
        return matches!(was_connected, None | Some(true)).then_some(LinkVerdict::SignIn);
    }
    if was_connected == Some(true) && !now.is_connected() {
        let retrying = matches!(
            now,
            LinkState::Reconnecting { .. } | LinkState::Connecting | LinkState::Provisioning { .. }
        );
        return Some(LinkVerdict::Lost { retrying });
    }
    None
}

/// How many things wait for the user, for the tray: sessions that need an answer and hosts
/// that wait for a sign-in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attention {
    pub sessions: usize,
    pub hosts: usize,
}

impl Attention {
    pub fn total(self) -> usize {
        self.sessions + self.hosts
    }

    /// The tray's line: what waits, in words ("Nothing needs you" when nothing does).
    pub fn words(self) -> String {
        let plural = |n: usize, one: &str, many: &str| {
            if n == 1 {
                one.to_string()
            } else {
                many.to_string()
            }
        };
        let sessions = (self.sessions > 0).then(|| {
            format!(
                "{} {}",
                self.sessions,
                plural(self.sessions, "session needs you", "sessions need you")
            )
        });
        let hosts = (self.hosts > 0).then(|| {
            format!(
                "{} {}",
                self.hosts,
                plural(
                    self.hosts,
                    "host needs you to sign in",
                    "hosts need you to sign in"
                )
            )
        });
        match (sessions, hosts) {
            (None, None) => "Nothing needs you".to_string(),
            (Some(s), None) => s,
            (None, Some(h)) => h,
            (Some(s), Some(h)) => format!("{s} · {h}"),
        }
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
    /// Whether each remote host's link was connected at its last report.
    connected: HashMap<HostId, bool>,
    /// Remote hosts whose link is `auth_needed` with `held`: they wait for a sign-in.
    waiting: BTreeSet<HostId>,
    names: BTreeMap<HostId, String>,
    /// Sessions currently needing an answer (for the tray).
    needs_you: HashMap<(HostId, SessionId), ()>,
}

impl Tracker {
    /// What waits for the user now.
    pub fn attention(&self) -> Attention {
        Attention {
            sessions: self.needs_you.len(),
            hosts: self.waiting.len(),
        }
    }

    fn host_name(&self, h: &HostId) -> String {
        self.names.get(h).cloned().unwrap_or_else(|| h.to_string())
    }

    /// Take in a host of a `Reset`: no note for its sessions, but its link is judged against
    /// what was known of it before (`was`), so a first sight at app start of a host that waits
    /// for a sign-in is announced and a repeated snapshot of the same wait is not.
    fn baseline(&mut self, host: &FleetHost, was: Option<bool>) -> Option<Note> {
        self.names
            .insert(host.host.id.clone(), host.host.name.clone());
        for s in &host.sessions {
            self.set(&host.host.id, s);
        }
        match &host.link {
            FleetLink::Remote { state } => self.link(&host.host.id, state, was),
            FleetLink::Local => None,
        }
    }

    /// A remote host's link reports `state`; `was`: whether it was connected before.
    fn link(&mut self, host: &HostId, state: &LinkState, was: Option<bool>) -> Option<Note> {
        self.connected.insert(host.clone(), state.is_connected());
        if matches!(state, LinkState::AuthNeeded { held: true, .. }) {
            self.waiting.insert(host.clone());
        } else {
            self.waiting.remove(host);
        }
        let name = self.host_name(host);
        let (kind, title, body) = match link_verdict(was, state)? {
            LinkVerdict::SignIn => (
                Kind::SignInNeeded,
                format!("{name} needs you to sign in"),
                "Sessions there keep running. Marsad does not reconnect it without you.".to_string(),
            ),
            LinkVerdict::Lost { retrying: true } => (
                Kind::HostLost,
                format!("Lost the connection to {name}"),
                "Sessions keep running there; Marsad is reconnecting.".to_string(),
            ),
            LinkVerdict::Lost { retrying: false } => (
                Kind::HostLost,
                format!("Lost the connection to {name}"),
                "Sessions keep running there. Marsad does not retry this on its own; open Hosts to connect it.".to_string(),
            ),
        };
        Some(Note {
            kind,
            host: host.clone(),
            session: None,
            title,
            body,
        })
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
                let known = std::mem::take(&mut self.connected);
                *self = Tracker::default();
                for h in hosts {
                    out.extend(self.baseline(h, known.get(&h.host.id).copied()));
                }
            }
            FleetEvent::HostUpserted { host } => {
                self.names.insert(host.id.clone(), host.name.clone());
            }
            FleetEvent::HostRemoved { id } => {
                self.connected.remove(id);
                self.waiting.remove(id);
                self.sessions.retain(|(h, _), _| h != id);
                self.needs_you.retain(|(h, _), _| h != id);
            }
            FleetEvent::Link { host, state } => {
                let was = self.connected.get(host).copied();
                out.extend(self.link(host, state, was));
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
            FleetEvent::AuthPrompt { .. }
            | FleetEvent::Git { .. }
            | FleetEvent::FeedError { .. } => {}
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

    /// The host "Box" with `link` and `sessions`.
    fn fleet_host(link: FleetLink, sessions: Vec<SessionSnapshot>) -> FleetHost {
        FleetHost {
            host: HostEntry {
                id: HostId::new("box"),
                name: "Box".into(),
                kind: HostKind::Wsl { distro: "x".into() },
                platform: None,
                created_at_ms: 0,
                interactive_auth: false,
                local_files: false,
                shared_connection: false,
                signed_out: false,
                machine: None,
            },
            link,
            sessions_live: true,
            sessions,
            auth_prompt: None,
            git: Vec::new(),
            feed_error: None,
        }
    }

    /// A tracker that knows one host with one session in `first`.
    fn tracker(first: SessionSnapshot) -> (Tracker, HostId) {
        let mut t = Tracker::default();
        t.apply(&FleetEvent::Reset {
            hosts: vec![fleet_host(
                FleetLink::Remote {
                    state: LinkState::Connecting,
                },
                vec![first],
            )],
        });
        (t, HostId::new("box"))
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
        assert_eq!(t.attention().sessions, 1);
        assert!(t.apply(&up(snap(Phase::NeedsYou))).is_empty(), "no repeat");
        assert!(t.apply(&up(working_turn("input"))).is_empty());
        assert_eq!(t.attention().sessions, 0);
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

    fn connected() -> LinkState {
        serde_json::from_value(serde_json::json!({
            "state": "connected",
            "hello": {"protocol": {"major": 1, "minor": 0}, "build": "b", "host_id": "box", "role": "daemon"}
        }))
        .expect("link state")
    }

    /// `auth_needed`; `held`: parked without an attempt because the user must ask for it.
    fn auth_needed(held: bool, cause: Option<&str>) -> LinkState {
        serde_json::from_value(serde_json::json!({
            "state": "auth_needed", "problem": "auth_needed",
            "detail": "connect to sign in", "held": held, "cause": cause,
        }))
        .expect("link state")
    }

    fn reconnecting() -> LinkState {
        serde_json::from_value(serde_json::json!({
            "state": "reconnecting", "attempt": 1, "last_error": "reset",
            "next_at": {"secs_since_epoch": 0, "nanos_since_epoch": 0},
        }))
        .expect("link state")
    }

    fn link(h: &HostId, state: LinkState) -> FleetEvent {
        FleetEvent::Link {
            host: h.clone(),
            state,
        }
    }

    fn reset_with(state: LinkState) -> FleetEvent {
        FleetEvent::Reset {
            hosts: vec![fleet_host(FleetLink::Remote { state }, Vec::new())],
        }
    }

    #[test]
    fn the_link_verdict_table() {
        use LinkVerdict::*;
        let held = auth_needed(true, Some("reset"));
        // Into held: from connected, or first sight; never from a link that was not up.
        assert_eq!(link_verdict(Some(true), &held), Some(SignIn));
        assert_eq!(link_verdict(None, &held), Some(SignIn));
        assert_eq!(link_verdict(Some(false), &held), None);
        // Not held (the user cancelled, a key was refused): no sign-in note.
        let cancelled = auth_needed(false, None);
        assert_eq!(link_verdict(None, &cancelled), None);
        assert_eq!(link_verdict(Some(false), &cancelled), None);
        assert_eq!(
            link_verdict(Some(true), &cancelled),
            Some(Lost { retrying: false })
        );
        // A drop the hub retries says so; one it parks says that too.
        assert_eq!(
            link_verdict(Some(true), &reconnecting()),
            Some(Lost { retrying: true })
        );
        let failed = LinkState::Failed {
            reason: "x".into(),
            problem: None,
        };
        assert_eq!(
            link_verdict(Some(true), &failed),
            Some(Lost { retrying: false })
        );
        assert_eq!(link_verdict(None, &failed), None);
        assert_eq!(link_verdict(Some(true), &connected()), None);
    }

    #[test]
    fn a_connected_host_that_is_held_raises_one_sign_in_note_and_no_lost_note() {
        let (mut t, h) = tracker(snap(Phase::Idle));
        t.apply(&link(&h, connected()));
        let n = t.apply(&link(
            &h,
            auth_needed(true, Some("connection reset by peer")),
        ));
        assert_eq!(n.len(), 1, "{n:?}");
        assert_eq!(n[0].kind, Kind::SignInNeeded);
        assert_eq!(n[0].title, "Box needs you to sign in");
        assert_eq!(n[0].host, h);
        assert_eq!(n[0].session, None, "it opens the host, not a session");
        assert!(!n[0].body.contains("reconnects when it can"));
        assert_eq!(t.attention().hosts, 1);
        // Still held (the hub says it again): nothing more.
        assert!(t.apply(&link(&h, auth_needed(true, None))).is_empty());
        assert_eq!(t.attention().hosts, 1);
    }

    #[test]
    fn a_new_sign_in_note_needs_the_host_connected_again_and_dropped_again() {
        let (mut t, h) = tracker(snap(Phase::Idle));
        t.apply(&link(&h, connected()));
        assert_eq!(t.apply(&link(&h, auth_needed(true, None))).len(), 1);
        // The user clicks: the attempt runs; it fails and the host is held again: no new note.
        assert!(t.apply(&link(&h, LinkState::Connecting)).is_empty());
        assert_eq!(
            t.attention().hosts,
            0,
            "an attempt is running: nobody waits"
        );
        assert!(t.apply(&link(&h, auth_needed(true, None))).is_empty());
        // Cancelled: not held, so no note and nothing waits.
        assert!(t.apply(&link(&h, LinkState::Connecting)).is_empty());
        assert!(t.apply(&link(&h, auth_needed(false, None))).is_empty());
        assert_eq!(t.attention().hosts, 0);
        // Connected, then dropped again: one more.
        t.apply(&link(&h, connected()));
        let again = t.apply(&link(&h, auth_needed(true, Some("closed"))));
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].kind, Kind::SignInNeeded);
    }

    #[test]
    fn first_sight_at_app_start_announces_a_held_host_once_across_resets() {
        let mut t = Tracker::default();
        let first = t.apply(&reset_with(auth_needed(true, None)));
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].kind, Kind::SignInNeeded);
        assert_eq!(t.attention().hosts, 1);
        // The app's subscription starts over and the host is still held: not announced again.
        assert!(t.apply(&reset_with(auth_needed(true, None))).is_empty());
        assert_eq!(t.attention().hosts, 1);
        // A host that is only unreachable at start says nothing.
        let mut t = Tracker::default();
        assert!(t.apply(&reset_with(reconnecting())).is_empty());
        // One that dropped into held while the subscription was away is announced.
        let mut t = Tracker::default();
        t.apply(&reset_with(connected()));
        assert_eq!(t.apply(&reset_with(auth_needed(true, None))).len(), 1);
    }

    #[test]
    fn removing_a_held_host_clears_the_wait() {
        let mut t = Tracker::default();
        t.apply(&reset_with(auth_needed(true, None)));
        t.apply(&FleetEvent::HostRemoved {
            id: HostId::new("box"),
        });
        assert_eq!(t.attention(), Attention::default());
    }

    #[test]
    fn a_host_the_hub_reconnects_says_so_and_one_it_does_not_retry_says_that() {
        let (mut t, h) = tracker(snap(Phase::Idle));
        t.apply(&link(&h, connected()));
        let n = t.apply(&link(&h, reconnecting()));
        assert_eq!(n[0].kind, Kind::HostLost);
        assert_eq!(
            n[0].body,
            "Sessions keep running there; Marsad is reconnecting."
        );
        t.apply(&link(&h, connected()));
        let n = t.apply(&link(
            &h,
            LinkState::Failed {
                reason: "gone".into(),
                problem: None,
            },
        ));
        assert_eq!(n[0].kind, Kind::HostLost);
        assert!(
            n[0].body.contains("does not retry this on its own"),
            "{}",
            n[0].body
        );
        assert!(!n[0].body.contains("reconnects when it can"));
    }

    #[test]
    fn the_sign_in_note_follows_its_own_preference() {
        let off = NotificationPrefs {
            sign_in_needed: false,
            ..NotificationPrefs::default()
        };
        assert!(!Kind::SignInNeeded.enabled(&off));
        assert!(Kind::HostLost.enabled(&off));
        assert!(Kind::SignInNeeded.enabled(&NotificationPrefs::default()));
        assert_eq!(Kind::SignInNeeded.code(), "sign_in_needed");
    }

    #[test]
    fn the_tray_line_says_what_waits() {
        let words = |sessions, hosts| Attention { sessions, hosts }.words();
        assert_eq!(words(0, 0), "Nothing needs you");
        assert_eq!(words(1, 0), "1 session needs you");
        assert_eq!(words(3, 0), "3 sessions need you");
        assert_eq!(words(0, 1), "1 host needs you to sign in");
        assert_eq!(words(0, 2), "2 hosts need you to sign in");
        assert_eq!(
            words(2, 1),
            "2 sessions need you · 1 host needs you to sign in"
        );
        assert_eq!(
            Attention {
                sessions: 2,
                hosts: 1
            }
            .total(),
            3
        );
    }
}
