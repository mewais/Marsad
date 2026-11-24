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
use std::future::Future;
use std::time::Duration;

use marsad_proto::fleet::GitSummary;
use marsad_proto::git::{GitStatus, GitStatusRequest, GitTarget};
use marsad_proto::{methods, HostId, MarsadError};
use tokio::sync::mpsc;
use tokio::task::{Id, JoinSet};
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
        tokio::spawn(run(
            move |(host, cwd): Key| {
                let link = link.clone();
                async move { summarize(&link, &host, &cwd).await }
            },
            fleet,
            rx,
        ));
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

/// The actor. `summarize` is the `git.status` call in production; tests substitute their own.
async fn run<S, Fut>(summarize: S, fleet: Fleet, mut rx: mpsc::Receiver<Key>)
where
    S: Fn(Key) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = GitSummary> + Send + 'static,
{
    let mut dirs: HashMap<Key, Dir> = HashMap::new();
    let mut inflight: JoinSet<(Key, GitSummary)> = JoinSet::new();
    // The directory of each running refresh, by task id. A panicking refresh returns no key, so
    // this is how its directory is found again.
    let mut running: HashMap<Id, Key> = HashMap::new();
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
            let summarize = summarize.clone();
            let key = k.clone();
            let task = inflight.spawn(async move {
                let s = summarize(key.clone()).await;
                (key, s)
            });
            running.insert(task.id(), k);
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
            Some(done) = inflight.join_next_with_id(), if !inflight.is_empty() => {
                match done {
                    Ok((id, (k, summary))) => {
                        running.remove(&id);
                        finish(&mut dirs, &k);
                        fleet.git_summary(k.0, summary).await;
                    }
                    Err(e) => {
                        hub_events::git_summary_failed(&e);
                        if let Some(k) = running.remove(&e.id()) {
                            finish(&mut dirs, &k);
                        }
                    }
                }
            }
            _ = timer => {}
        }
    }
}

/// A refresh ended, with a result or a panic: the directory may run again once [`MIN_INTERVAL`]
/// has passed since then. A touch that arrived meanwhile is already marked dirty.
fn finish(dirs: &mut HashMap<Key, Dir>, k: &Key) {
    if let Some(d) = dirs.get_mut(k) {
        d.running = false;
        d.last_run = Some(Instant::now());
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    fn summary(cwd: &str) -> GitSummary {
        GitSummary {
            cwd: cwd.to_string(),
            repo_root: None,
            branch: None,
            ahead: 0,
            behind: 0,
            files: 0,
            added: 0,
            removed: 0,
            refreshed_at_ms: 0,
            error: None,
        }
    }

    /// The first refresh panics. The directory is touched again right away; it must be refreshed
    /// again (the second summarizer call) rather than stay marked as running for good.
    #[tokio::test]
    async fn a_panicking_refresh_frees_its_directory_for_a_later_touch() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let (done_tx, mut done) = mpsc::unbounded_channel::<()>();
        let (tx, rx) = mpsc::channel(QUEUE);
        let calls = attempts.clone();
        tokio::spawn(run(
            move |(_, cwd): Key| {
                let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                let done_tx = done_tx.clone();
                async move {
                    if n == 1 {
                        panic!("summary panicked on purpose");
                    }
                    let _ = done_tx.send(());
                    summary(&cwd)
                }
            },
            Fleet::spawn(),
            rx,
        ));
        let key = (HostId::new("box"), "/work/repo".to_string());
        tx.send(key.clone()).await.unwrap();
        tx.send(key).await.unwrap();
        tokio::time::timeout(Duration::from_secs(15), done.recv())
            .await
            .expect("the directory was refreshed again after the panic")
            .expect("the refresh reported");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
