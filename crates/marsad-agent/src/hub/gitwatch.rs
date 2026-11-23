//! Background git summaries for list rows (`FleetHost.git`).
//!
//! One actor owns the per-directory state. The fleet aggregator *touches* a `(host, cwd)` when
//! a session in it appears, finishes a tool call or a turn, or changes phase; `fleet.git_refresh`
//! touches one explicitly. Touches are coalesced: a directory is refreshed at most once per
//! [`MIN_INTERVAL`] (a touch inside that window is served when it ends, by a timer), at most
//! [`MAX_INFLIGHT`] `git.status` calls run at once (each bounded by [`CALL_DEADLINE`]), and at
//! most [`MAX_DIRS`] directories are tracked (the least recently touched is forgotten). Results
//! go back to the fleet as [`GitSummary`] events. Nothing is polled: a directory nobody touches
//! is never asked again.

use std::collections::HashMap;
use std::time::Duration;

use marsad_proto::fleet::GitSummary;
use marsad_proto::git::{GitStatus, GitStatusRequest, GitTarget};
use marsad_proto::{methods, HostId, MarsadError};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::Instant;
use wasit_control::{RequestOpts, Supervised};

use super::fleet::Fleet;
use crate::hub_events;
use crate::util::now_ms;

pub const MIN_INTERVAL: Duration = Duration::from_secs(4);
pub const MAX_INFLIGHT: usize = 2;
pub const MAX_DIRS: usize = 512;
pub const CALL_DEADLINE: Duration = Duration::from_secs(20);
const QUEUE: usize = 1024;

type Key = (HostId, String);

/// Handle to the watcher. Cheap to clone.
#[derive(Clone)]
pub struct GitWatch {
    tx: mpsc::Sender<Key>,
}

#[derive(Default)]
struct Dir {
    dirty: bool,
    running: bool,
    last_run: Option<Instant>,
    touched: Option<Instant>,
}

impl GitWatch {
    /// Start the watcher; it calls `git.status` through `link` (the hub's own routing, so any
    /// host) and reports to `fleet`.
    pub fn spawn(link: Supervised, fleet: Fleet) -> GitWatch {
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(run(link, fleet, rx));
        GitWatch { tx }
    }

    /// Ask for a refresh of `cwd` on `host` (coalesced; never waits).
    pub fn touch(&self, host: HostId, cwd: String) {
        if let Err(e) = self.tx.try_send((host, cwd)) {
            // Only when 1024 touches are queued: the directory is being refreshed anyway soon,
            // and the next session change touches it again.
            hub_events::git_touch_dropped(&e);
        }
    }
}

async fn run(link: Supervised, fleet: Fleet, mut rx: mpsc::Receiver<Key>) {
    let mut dirs: HashMap<Key, Dir> = HashMap::new();
    let mut inflight: JoinSet<(Key, GitSummary)> = JoinSet::new();
    loop {
        // Start what may start now; find when the next deferred refresh is due.
        let now = Instant::now();
        let mut next_due: Option<Instant> = None;
        let mut ready: Vec<Key> = dirs
            .iter()
            .filter(|(_, d)| d.dirty && !d.running)
            .filter_map(|(k, d)| {
                let due = d.last_run.map_or(now, |t| t + MIN_INTERVAL);
                if due <= now {
                    Some(k.clone())
                } else {
                    next_due = Some(next_due.map_or(due, |n: Instant| n.min(due)));
                    None
                }
            })
            .collect();
        ready.sort_by_key(|k| dirs[k].touched);
        for k in ready {
            if inflight.len() >= MAX_INFLIGHT {
                break;
            }
            let d = dirs.get_mut(&k).expect("listed above");
            d.dirty = false;
            d.running = true;
            let link = link.clone();
            inflight.spawn(async move {
                let s = summarize(&link, &k.0, &k.1).await;
                (k, s)
            });
        }
        let timer = async {
            match next_due {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            m = rx.recv() => {
                let Some(k) = m else { return };
                let d = dirs.entry(k).or_default();
                d.dirty = true;
                d.touched = Some(Instant::now());
                if dirs.len() > MAX_DIRS {
                    if let Some(old) = dirs
                        .iter()
                        .filter(|(_, d)| !d.running)
                        .min_by_key(|(_, d)| d.touched)
                        .map(|(k, _)| k.clone())
                    {
                        dirs.remove(&old);
                    }
                }
            }
            Some(done) = inflight.join_next(), if !inflight.is_empty() => {
                match done {
                    Ok((k, summary)) => {
                        if let Some(d) = dirs.get_mut(&k) {
                            d.running = false;
                            d.last_run = Some(Instant::now());
                        }
                        fleet.git_summary(k.0, summary).await;
                    }
                    Err(e) => hub_events::git_summary_failed(&e),
                }
            }
            _ = timer => {}
        }
    }
}

async fn summarize(link: &Supervised, host: &HostId, cwd: &str) -> GitSummary {
    let mut s = GitSummary {
        cwd: cwd.to_string(),
        repo_root: None,
        branch: None,
        ahead: 0,
        behind: 0,
        files: 0,
        added: 0,
        removed: 0,
        refreshed_at_ms: now_ms(),
        error: None,
    };
    let req = GitStatusRequest {
        target: GitTarget::cwd(cwd),
    };
    let r: Result<GitStatus, _> = link
        .request(
            methods::GIT_STATUS,
            req,
            RequestOpts::new(CALL_DEADLINE).host(host.clone()),
            CALL_DEADLINE,
        )
        .await;
    match r {
        Ok(g) => {
            s.repo_root = Some(g.repo_root);
            s.branch = g.branch;
            s.ahead = g.ahead;
            s.behind = g.behind;
            s.files = g.files.len() as u32;
            for f in &g.files {
                s.added += f.staged_added + f.unstaged_added;
                s.removed += f.staged_removed + f.unstaged_removed;
            }
        }
        Err(e) => {
            s.error = Some(match &e {
                wasit_control::CallError::Rpc(rpc) => MarsadError::from_rpc(rpc)
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| rpc.message.clone()),
                other => other.to_string(),
            });
        }
    }
    s
}
