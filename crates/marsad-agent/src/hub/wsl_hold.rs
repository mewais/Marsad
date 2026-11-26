//! The hub's half of the WSL keep-alive: attaches one `hold-sessions` holder (see
//! [`crate::hold`]) to each WSL distribution that has live sessions, so WSL does not shut the
//! distribution down, and with it the daemon and the session shims, for being idle.
//!
//! Event-driven: the watcher wakes on every fleet event (a link state, a host, a session) and on
//! the end of each holder it started. It starts a holder for a distribution when
//! * the distribution's link comes up while it has live sessions,
//! * its live-session count goes from 0 to more than 0, or
//! * its previous holder ended cleanly (no live session at that moment) and sessions are live
//!   again since, which is the race of a session starting as the holder leaves.
//!
//! It never starts a second holder while one of its own runs, and a holder that exits because
//! another one holds the distribution's lock, or because it failed, is not started again until
//! one of the triggers above happens: a holder that cannot run must not be restarted in a loop.
//! The holder itself enforces "one per distribution" through the lock.

use std::collections::{HashMap, HashSet};
use std::io;
use std::process::Stdio;
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::StreamExt;
use marsad_proto::fleet::{FleetHost, FleetLink};
use marsad_proto::hosts::HostKind;
use tokio::task::JoinHandle;

use super::fleet::Fleet;
use super::transport::build_dir;
use super::wsl::wsl_exe;
use crate::hold;
use crate::hub_events;

/// What the policy needs to know of one WSL host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslFacts {
    pub distro: String,
    /// The host's link is connected.
    pub connected: bool,
    /// Sessions that keep their shim running (the feed is live; otherwise zero).
    pub live: usize,
}

/// The facts of one host, `None` unless it is a WSL host. `phases` are the wire names of the
/// host's session phases; they count only while the session feed is live.
pub fn facts<'a>(
    kind: &HostKind,
    link: &FleetLink,
    sessions_live: bool,
    phases: impl Iterator<Item = &'a str>,
) -> Option<WslFacts> {
    let HostKind::Wsl { distro } = kind else {
        return None;
    };
    let connected = matches!(link, FleetLink::Remote { state } if state.is_connected());
    let live = if sessions_live {
        phases.filter(|p| hold::keeps_alive(p)).count()
    } else {
        0
    };
    Some(WslFacts {
        distro: distro.clone(),
        connected,
        live,
    })
}

/// The facts of every WSL host in a fleet snapshot.
pub fn fleet_facts(hosts: &[FleetHost]) -> Vec<WslFacts> {
    hosts
        .iter()
        .filter_map(|h| {
            facts(
                &h.host.kind,
                &h.link,
                h.sessions_live,
                h.sessions.iter().map(|s| s.phase.name()),
            )
        })
        .collect()
}

/// Which holders to start, and what the watcher remembers between events.
#[derive(Debug, Default)]
pub struct Policy {
    /// The facts of each distribution at the previous decision.
    seen: HashMap<String, (bool, usize)>,
    /// Distributions whose holder ended cleanly and that are asked about again.
    again: HashSet<String>,
    /// Distributions with a holder this watcher started and has not seen end.
    running: HashSet<String>,
}

impl Policy {
    /// The distributions to start a holder for, given the facts now. Each one is marked running
    /// until [`Policy::ended`] or [`Policy::not_started`] says otherwise.
    pub fn decide(&mut self, facts: &[WslFacts]) -> Vec<String> {
        let mut start = Vec::new();
        for f in facts {
            let (was_connected, was_live) = self
                .seen
                .insert(f.distro.clone(), (f.connected, f.live))
                .unwrap_or((false, 0));
            let again = self.again.remove(&f.distro);
            let came_up = f.connected && !was_connected;
            let first_live = was_live == 0;
            let wanted = f.live > 0 && (came_up || first_live || again);
            if wanted && self.running.insert(f.distro.clone()) {
                start.push(f.distro.clone());
            }
        }
        start
    }

    /// The holder of `distro` ended with `code`. Only a clean end (no live session then) asks for
    /// a new holder, and only if sessions are live again by the next decision.
    pub fn ended(&mut self, distro: &str, code: Option<i32>) {
        self.running.remove(distro);
        if code == Some(i32::from(hold::EXIT_NO_LIVE_SESSIONS)) {
            self.again.insert(distro.to_string());
        }
    }

    /// The holder of `distro` could not be started at all.
    pub fn not_started(&mut self, distro: &str) {
        self.running.remove(distro);
    }
}

/// Starts a holder in a distribution. The future resolves with the holder's exit code when it
/// has ended (`None`: it had none).
pub trait Launcher: Send + Sync + 'static {
    fn launch(&self, distro: &str) -> io::Result<BoxFuture<'static, Option<i32>>>;
}

/// The Windows launcher: `wsl.exe` runs the helper of this hub's build in the distribution, as
/// the distribution's default user, detached from the app's lifetime (the same spawn the daemon
/// starts with), so the holder outlives the app and nothing is killed by name.
pub struct WslLauncher {
    build: String,
}

impl WslLauncher {
    pub fn new(build: &str) -> WslLauncher {
        WslLauncher {
            build: build.to_string(),
        }
    }
}

/// The command `sh` runs in the distribution: the helper of `build` in the directory the bootstrap
/// installs it to (`~/.marsad/bin/<build>/<uname -s>-<uname -m>/`). `sh` expands `$HOME`, which
/// `wsl.exe --exec` alone would not.
fn holder_script(build: &str) -> String {
    format!(
        "exec $HOME/.marsad/bin/{}/$(uname -s)-$(uname -m)/marsad-agent {}",
        build_dir(build),
        hold::COMMAND
    )
}

impl Launcher for WslLauncher {
    fn launch(&self, distro: &str) -> io::Result<BoxFuture<'static, Option<i32>>> {
        // Off Windows there is no `wsl.exe`, and no WSL host to ask for a holder.
        let exe = wsl_exe().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "wsl.exe is not available on this PC",
            )
        })?;
        let mut cmd = wasit_process::detached_command(wasit_process::plain_path(&exe));
        cmd.args(["--distribution", distro, "--cd", "~", "--exec", "sh", "-c"])
            .arg(holder_script(&self.build))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = wasit_process::spawn_detached(&mut cmd)?;
        Ok(Box::pin(async move {
            child.wait().await.ok().and_then(|s| s.code())
        }))
    }
}

/// Starts the watcher for the life of the hub: holders for WSL distributions with live sessions,
/// started through `launcher`.
pub fn watch(fleet: Fleet, launcher: Arc<dyn Launcher>) -> JoinHandle<()> {
    tokio::spawn(run(fleet, launcher))
}

async fn run(fleet: Fleet, launcher: Arc<dyn Launcher>) {
    let (ended_tx, mut ended_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Option<i32>)>();
    let mut policy = Policy::default();
    let Ok(mut events) = fleet.subscribe(None).await else {
        return;
    };
    loop {
        tokio::select! {
            item = events.next() => {
                // The fleet's feed ends only when it lagged or the fleet is gone: the next
                // subscription starts with the whole fleet, which is decided on as well.
                if item.is_none() {
                    match fleet.subscribe(None).await {
                        Ok(s) => events = s,
                        Err(_) => return,
                    }
                }
            }
            Some((distro, code)) = ended_rx.recv() => policy.ended(&distro, code),
        }
        for distro in policy.decide(&fleet_facts(&fleet.snapshot().await)) {
            match launcher.launch(&distro) {
                Ok(exit) => {
                    let tx = ended_tx.clone();
                    tokio::spawn(async move {
                        let code = exit.await;
                        let _ = tx.send((distro, code));
                    });
                }
                Err(e) => {
                    policy.not_started(&distro);
                    hub_events::wsl_hold_start_failed(&distro, &e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(distro: &str, connected: bool, live: usize) -> WslFacts {
        WslFacts {
            distro: distro.into(),
            connected,
            live,
        }
    }

    #[test]
    fn a_session_starting_in_a_quiet_distribution_starts_one_holder() {
        let mut p = Policy::default();
        assert_eq!(p.decide(&[f("Ubuntu", true, 0)]), Vec::<String>::new());
        assert_eq!(p.decide(&[f("Ubuntu", true, 1)]), ["Ubuntu"]);
        // Still live, holder running: nothing new.
        assert!(p.decide(&[f("Ubuntu", true, 2)]).is_empty());
        assert!(p.decide(&[f("Ubuntu", true, 1)]).is_empty());
    }

    #[test]
    fn a_link_that_comes_up_with_live_sessions_starts_a_holder() {
        // Live sessions known while the link is still down (a reconnect in progress): a holder
        // starts at once, because the sessions' distribution may be going away.
        let mut p = Policy::default();
        assert_eq!(p.decide(&[f("Ubuntu", false, 2)]), ["Ubuntu"]);
        // The holder ended (its daemon went with the link); no trigger while the link stays down.
        p.ended("Ubuntu", Some(i32::from(hold::EXIT_DAEMON_GONE)));
        assert!(p.decide(&[f("Ubuntu", false, 2)]).is_empty());
        // The link comes up again: the connect is the trigger.
        assert_eq!(p.decide(&[f("Ubuntu", true, 2)]), ["Ubuntu"]);
    }

    #[test]
    fn no_live_session_means_no_holder() {
        let mut p = Policy::default();
        assert!(p.decide(&[f("Ubuntu", true, 0)]).is_empty());
        assert!(p.decide(&[f("Ubuntu", true, 0)]).is_empty());
    }

    #[test]
    fn a_clean_end_is_repeated_when_sessions_are_live_again() {
        let mut p = Policy::default();
        assert_eq!(p.decide(&[f("Ubuntu", true, 1)]), ["Ubuntu"]);
        // The holder saw zero and left; a session started before the watcher looked again.
        p.ended("Ubuntu", Some(0));
        assert_eq!(p.decide(&[f("Ubuntu", true, 1)]), ["Ubuntu"]);
        // A decision while that holder runs starts nothing more, and the repeat is used up.
        assert!(p.decide(&[f("Ubuntu", true, 1)]).is_empty());
        p.ended("Ubuntu", Some(i32::from(hold::EXIT_LOCK_HELD)));
        assert!(
            p.decide(&[f("Ubuntu", true, 1)]).is_empty(),
            "no repeat without a clean end"
        );
    }

    #[test]
    fn a_holder_that_cannot_run_is_not_restarted_in_a_loop() {
        let mut p = Policy::default();
        assert_eq!(p.decide(&[f("Ubuntu", true, 1)]), ["Ubuntu"]);
        // Another holder holds the lock (exit 3): no restart while sessions stay live.
        p.ended("Ubuntu", Some(i32::from(hold::EXIT_LOCK_HELD)));
        assert!(p.decide(&[f("Ubuntu", true, 1)]).is_empty());
        assert!(p.decide(&[f("Ubuntu", true, 2)]).is_empty());
        // A failed one (exit 1) the same.
        assert_eq!(p.decide(&[f("Ubuntu", true, 0)]), Vec::<String>::new());
        assert_eq!(
            p.decide(&[f("Ubuntu", true, 1)]),
            ["Ubuntu"],
            "0 -> >0 is a trigger"
        );
        p.ended("Ubuntu", Some(1));
        assert!(p.decide(&[f("Ubuntu", true, 1)]).is_empty());
        // A launch that failed outright is not retried either.
        p.not_started("Ubuntu");
        assert!(p.decide(&[f("Ubuntu", true, 1)]).is_empty());
    }

    #[test]
    fn each_distribution_has_its_own_holder() {
        let mut p = Policy::default();
        let mut started = p.decide(&[f("Ubuntu", true, 1), f("Debian", true, 1)]);
        started.sort();
        assert_eq!(started, ["Debian", "Ubuntu"]);
        p.ended("Debian", Some(0));
        assert_eq!(
            p.decide(&[f("Ubuntu", true, 1), f("Debian", true, 1)]),
            ["Debian"]
        );
    }

    #[test]
    fn the_holder_command_runs_the_helper_of_this_build_with_the_subcommand() {
        let s = holder_script("0.2.0+abc");
        assert_eq!(
            s,
            "exec $HOME/.marsad/bin/0.2.0+abc/$(uname -s)-$(uname -m)/marsad-agent hold-sessions"
        );
    }

    #[test]
    fn facts_are_taken_for_wsl_hosts_only() {
        let link = FleetLink::Remote {
            state: marsad_proto::LinkState::Connected {
                hello: wasit_control::Hello::new("b", "h", "daemon"),
            },
        };
        let wsl = HostKind::Wsl {
            distro: "Ubuntu".into(),
        };
        let got = facts(
            &wsl,
            &link,
            true,
            ["idle", "exited", "needs_you"].into_iter(),
        );
        assert_eq!(got, Some(f("Ubuntu", true, 2)));
        // Sessions the feed has not confirmed count for nothing.
        let unsure = facts(&wsl, &link, false, ["idle"].into_iter());
        assert_eq!(unsure, Some(f("Ubuntu", true, 0)));
        let local = facts(
            &HostKind::Local,
            &FleetLink::Local,
            true,
            ["idle"].into_iter(),
        );
        assert_eq!(local, None);
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_there_is_no_wsl_to_launch_into() {
        let l = WslLauncher::new("0.2.0");
        let err = l.launch("Ubuntu").err().expect("no wsl.exe here");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
