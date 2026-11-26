//! `marsad-agent hold-sessions`: keeps a WSL distribution from idle-terminating while its Marsad
//! daemon has live sessions.
//!
//! WSL shuts a distribution down once its last Windows-side process exits, which kills the
//! daemon and every session shim in it. The hub therefore attaches one small process per
//! distribution (`wsl.exe -d <distro> … hold-sessions`, see [`crate::hub::wsl_hold`]); this is the
//! process it runs, inside the distribution:
//!
//! * It takes an instance lock in the distribution's runtime directory, so one distribution has
//!   at most one holder: a second one exits at once ([`EXIT_LOCK_HELD`]).
//! * It subscribes to the daemon's `host.sessions` topic and keeps the set of live sessions
//!   from it. It exits the moment that set is empty ([`EXIT_NO_LIVE_SESSIONS`]), and when the
//!   daemon goes away ([`EXIT_DAEMON_GONE`]). Nothing is polled and nothing waits on a timer.
//! * It never starts a daemon and never signals a process; it only holds a subscription.
//!
//! Stdout carries one `live=<n>` line per applied session event (the count after it). The hub
//! ignores it; tests use it to wait for the holder's state without sleeping.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use serde::Deserialize;
use wasit_control::{
    ClientOptions, CloseReason, ConnectError, ControlClient, Endpoint, Hello, InstanceLock,
    LockError,
};

use crate::cli::Options;
use crate::{diag, version};

/// The subcommand's name, as the hub runs it.
pub const COMMAND: &str = "hold-sessions";

/// The lock that makes one holder per distribution, in the runtime directory.
pub const LOCK_FILE: &str = "hold-sessions.lock";

/// Exit status: no live session is left (also at the start, when none is).
pub const EXIT_NO_LIVE_SESSIONS: u8 = 0;
/// Exit status: the daemon's feed failed or sent something this build cannot read.
pub const EXIT_ERROR: u8 = 1;
/// Exit status: another holder of this distribution holds the lock.
pub const EXIT_LOCK_HELD: u8 = 3;
/// Exit status: no daemon is reachable, or it went away while sessions were being held.
pub const EXIT_DAEMON_GONE: u8 = 4;

/// Whether a session in the phase named `state` keeps its shim running, and so keeps the
/// distribution busy. Only the phases of a session whose shim has ended (`exited`, `failed`,
/// `lost`) do not; a phase this build does not know counts as live, so an unknown state can
/// only keep the distribution up for longer, never end sessions early.
pub fn keeps_alive(state: &str) -> bool {
    !matches!(state, "exited" | "failed" | "lost")
}

/// The part of a `host.sessions` event this holder reads. The events carry whole session
/// snapshots; only the id and the phase's state name are needed, so a change to the rest of
/// the snapshot cannot break the holder.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Event {
    Reset { sessions: Vec<Entry> },
    Upsert { session: Entry },
    Removed { id: String },
}

#[derive(Debug, Deserialize)]
struct Entry {
    id: String,
    phase: PhaseState,
}

#[derive(Debug, Deserialize)]
struct PhaseState {
    state: String,
}

/// The live sessions of the daemon, as the feed has described them so far.
#[derive(Debug, Default)]
pub struct Live {
    /// Every session the feed has named, with whether it keeps its shim running.
    sessions: HashMap<String, bool>,
}

impl Live {
    fn apply(&mut self, ev: Event) {
        match ev {
            // A snapshot replaces everything, so a resubscription starts from the daemon's word.
            Event::Reset { sessions } => {
                self.sessions.clear();
                for s in sessions {
                    self.sessions.insert(s.id, keeps_alive(&s.phase.state));
                }
            }
            Event::Upsert { session } => {
                self.sessions
                    .insert(session.id, keeps_alive(&session.phase.state));
            }
            Event::Removed { id } => {
                self.sessions.remove(&id);
            }
        }
    }

    /// The number of sessions that keep their shim running.
    pub fn count(&self) -> usize {
        self.sessions.values().filter(|live| **live).count()
    }
}

/// Holds the distribution for the daemon at `ep` while it has live sessions, with the lock at
/// `lock`. Returns the exit status (one of the `EXIT_` codes). Every applied event is reported
/// on `out` as `live=<count>`.
pub async fn hold(ep: &Endpoint, lock: &Path, out: &mut impl Write) -> u8 {
    // Taken before anything else: a second holder does nothing, not even connect.
    let _lock = match InstanceLock::try_acquire(lock).await {
        Ok(l) => l,
        Err(LockError::Held(_)) => return EXIT_LOCK_HELD,
        Err(_) => return EXIT_ERROR,
    };
    let opts = ClientOptions::new(Hello::new(
        version::build_id(),
        "marsad-agent-hold-sessions",
        "cli",
    ));
    let client = match ControlClient::connect_endpoint(ep, opts).await {
        Ok(c) => c,
        Err(ConnectError::Io(_)) => return EXIT_DAEMON_GONE,
        Err(_) => return EXIT_ERROR,
    };
    let mut live = Live::default();
    loop {
        // A fresh subscription starts with a snapshot, which also rebuilds `live` after a lag.
        let mut sub = match client
            .subscribe(marsad_proto::runtime::topics::HOST_SESSIONS, None)
            .await
        {
            Ok(s) => s,
            Err(_) => return EXIT_ERROR,
        };
        while let Some((_, v)) = sub.next().await {
            let Ok(ev) = serde_json::from_value::<Event>(v) else {
                return EXIT_ERROR;
            };
            live.apply(ev);
            let _ = writeln!(out, "live={}", live.count()).and_then(|()| out.flush());
            if live.count() == 0 {
                return EXIT_NO_LIVE_SESSIONS;
            }
        }
        match sub.close_reason() {
            Some(CloseReason::Lagged) => continue,
            Some(CloseReason::Error { .. }) => return EXIT_ERROR,
            // The connection or the feed ended: the daemon is gone (or no longer says anything).
            _ => return EXIT_DAEMON_GONE,
        }
    }
}

/// The subcommand: `marsad-agent hold-sessions [--data <dir>] [--run-dir <dir>] [--socket <path>]`
/// (the daemon's own locations; the defaults are the daemon's defaults).
pub fn run(args: &[String]) -> ExitCode {
    // No sink: this process is a holder, and what it could record is the daemon's business.
    diag::content_free_panic_hook("hold-sessions");
    let o = match Options::parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("marsad-agent hold-sessions: {e}");
            return ExitCode::from(2);
        }
    };
    let lock = o.layout().run.join(LOCK_FILE);
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("marsad-agent hold-sessions: cannot start the runtime: {e}");
            return ExitCode::from(1);
        }
    };
    let code = rt.block_on(hold(&o.endpoint(), &lock, &mut std::io::stdout()));
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, state: &str) -> Entry {
        Entry {
            id: id.into(),
            phase: PhaseState {
                state: state.into(),
            },
        }
    }

    #[test]
    fn only_ended_shims_do_not_keep_the_distribution() {
        for live in ["starting", "idle", "working", "needs_you"] {
            assert!(keeps_alive(live), "{live}");
        }
        for ended in ["exited", "failed", "lost"] {
            assert!(!keeps_alive(ended), "{ended}");
        }
        // An unknown phase is treated as live: it can only keep the distribution longer.
        assert!(keeps_alive("some_future_phase"));
    }

    #[test]
    fn a_reset_replaces_the_set_and_upserts_and_removals_update_it() {
        let mut live = Live::default();
        live.apply(Event::Reset {
            sessions: vec![entry("a", "idle"), entry("b", "exited")],
        });
        assert_eq!(live.count(), 1);
        live.apply(Event::Upsert {
            session: entry("b", "working"),
        });
        assert_eq!(live.count(), 2);
        live.apply(Event::Upsert {
            session: entry("a", "lost"),
        });
        assert_eq!(live.count(), 1);
        live.apply(Event::Removed { id: "b".into() });
        assert_eq!(live.count(), 0);
        // Removing an unknown session changes nothing.
        live.apply(Event::Removed { id: "zz".into() });
        assert_eq!(live.count(), 0);
        live.apply(Event::Reset {
            sessions: vec![entry("c", "needs_you")],
        });
        assert_eq!(live.count(), 1, "a snapshot replaces what was known");
    }

    #[test]
    fn events_decode_from_the_wire_shape_and_ignore_the_rest_of_a_snapshot() {
        let reset = serde_json::json!({
            "kind": "reset",
            "sessions": [{"id": "a", "phase": {"state": "idle", "extra": 1}, "cwd": "/x"}]
        });
        let ev: Event = serde_json::from_value(reset).expect("a reset");
        let mut live = Live::default();
        live.apply(ev);
        assert_eq!(live.count(), 1);
        let removed: Event =
            serde_json::from_value(serde_json::json!({"kind": "removed", "id": "a"}))
                .expect("a removal");
        live.apply(removed);
        assert_eq!(live.count(), 0);
        assert!(serde_json::from_value::<Event>(serde_json::json!({"kind": "bogus"})).is_err());
    }
}
