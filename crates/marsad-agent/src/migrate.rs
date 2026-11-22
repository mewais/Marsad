//! Adopting the flat layout of builds before the per-machine layout (`marsad_proto::layout`).
//!
//! Earlier builds kept everything directly in the data root: `<root>/marsad.sock` (+ `.lock`),
//! `<root>/sessions/<id>/` (with each shim's `shim.lock` and `ctl.sock` inside), `<root>/wasl`
//! and `<root>/store`. On a home directory shared by several machines that layout belongs to
//! whichever machine ran it. At start, holding this machine's own lock, the daemon:
//!
//! 1. looks at the legacy daemon: its lock free ⇒ nobody runs it; held and its endpoint
//!    answers ⇒ an earlier build on THIS machine (sockets are per-kernel) — it is asked to exit;
//!    held but the endpoint refuses ⇒ another machine's daemon: the whole legacy layout is left
//!    alone;
//! 2. claims each legacy session by renaming its directory into
//!    `<root>/machines/<key>/sessions/` (same filesystem: a live shim keeps writing through its
//!    open descriptors; its socket moves with the directory, and the runtime finds a legacy
//!    shim's lock in the data directory until its next process generation; the old path
//!    becomes a symlink to the new one while that shim runs, since it reopens its journal by
//!    path — on Windows a live session's directory cannot be renamed and is claimed once it
//!    has exited): finished sessions
//!    and sessions whose shim answers locally are claimed; a session whose shim lock is held but
//!    whose socket refuses (another machine's) is left; a session with no live shim and no exit
//!    record that was written in the last 10 minutes is left for a later start (it may belong to
//!    a machine whose locks this one cannot see);
//! 3. moves `wasl/` and `store/` when this machine does not have its own yet, and removes the
//!    legacy socket and lock files once nothing is left to protect.
//!
//! Every step is a rename or a removal of something this machine owns; nothing is copied, and
//! a failure only logs and leaves the legacy file where it was.

use std::path::Path;
use std::time::{Duration, SystemTime};

use marsad_proto::layout::Layout;
use marsad_proto::methods;
use wasit_control::{ClientOptions, ControlClient, Endpoint, Hello, InstanceLock, LockError};
use wasit_shim::{ShimClient, ShimState};

use crate::version;

/// A session without an exit record touched more recently than this is not claimed yet.
const RECENT: Duration = Duration::from_secs(600);

/// Ask the daemon at `ep` to shut down and wait until it closed the connection. `false` when
/// nobody answers there.
pub async fn stop_endpoint(ep: &Endpoint) -> bool {
    let opts = ClientOptions::new(Hello::new(version::build_id(), "marsad-agent-stop", "cli"));
    let Ok(Ok(client)) = tokio::time::timeout(
        Duration::from_secs(5),
        ControlClient::connect_endpoint(ep, opts),
    )
    .await
    else {
        return false;
    };
    // The daemon may close the connection before answering; both mean it is going down.
    let _ = client
        .request::<marsad_proto::Ack>(methods::DAEMON_SHUTDOWN, (), Duration::from_secs(10))
        .await;
    // Bounded: a daemon that keeps the connection open is reported by the lock wait below.
    let _ = tokio::time::timeout(Duration::from_secs(10), client.closed()).await;
    true
}

/// What happened (for logs and tests).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub foreign_daemon: bool,
    pub stopped_daemon: bool,
    pub claimed: Vec<String>,
    pub left: Vec<String>,
}

pub async fn run(layout: &Layout) -> Report {
    let r = adopt(layout).await;
    if r != Report::default() {
        tracing::info!(?r, "adopted the earlier flat data layout");
    }
    r
}

async fn adopt(layout: &Layout) -> Report {
    let mut report = Report::default();
    let root = &layout.root;
    let legacy_sessions = root.join("sessions");
    let legacy_socket = layout.legacy_socket();
    let legacy_lock = wasit_control::lock_path_for(&legacy_socket);
    if !legacy_sessions.exists() && !legacy_lock.exists() && !legacy_socket.exists() {
        return report;
    }
    // 1. The legacy daemon.
    let lock = match take_legacy_lock(&legacy_lock).await {
        Some(l) => Some(l),
        None => {
            let ep = wasit_control::local_endpoint(&legacy_socket);
            if stop_endpoint(&ep).await {
                report.stopped_daemon = true;
                wait_legacy_lock(&legacy_lock).await
            } else {
                None
            }
        }
    };
    if lock.is_none() && legacy_lock.exists() {
        report.foreign_daemon = true;
        tracing::info!(
            root = %root.display(),
            "the earlier data layout here belongs to another machine's daemon; leaving it alone"
        );
        return report;
    }
    // 2. Sessions.
    let target = layout.data.join("sessions");
    if let Err(e) = tokio::fs::create_dir_all(&target).await {
        tracing::warn!(error = %e, "cannot create {}", target.display());
        return report;
    }
    if let Ok(mut rd) = tokio::fs::read_dir(&legacy_sessions).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let dir = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = tokio::fs::symlink_metadata(&dir).await else {
                continue;
            };
            if meta.file_type().is_symlink() {
                // Left by an earlier claim of a live session; gone once its shim is.
                let stale = !matches!(wasit_shim::probe(&dir).await, Ok(ShimState::Running));
                if stale
                    && tokio::fs::read_link(&dir)
                        .await
                        .is_ok_and(|t| t.starts_with(&target))
                {
                    let _ = tokio::fs::remove_file(&dir).await;
                } else {
                    report.left.push(name);
                }
                continue;
            }
            if !meta.is_dir() {
                continue;
            }
            if let Some(live) = claimable(&dir).await {
                let to = target.join(&name);
                match tokio::fs::rename(&dir, &to).await {
                    Ok(()) => {
                        // A live shim of the earlier build reopens its journal by path:
                        // keep that path working until it exits.
                        #[cfg(unix)]
                        if live {
                            if let Err(e) = tokio::fs::symlink(&to, &dir).await {
                                tracing::warn!(session = %name, error = %e, "cannot link the old session path");
                            }
                            link_runtime(layout, &name, &to).await;
                        }
                        #[cfg(not(unix))]
                        let _ = live;
                        report.claimed.push(name)
                    }
                    // Another machine claimed it first, or it cannot move: leave it.
                    Err(e) => {
                        tracing::warn!(session = %name, error = %e, "cannot claim legacy session");
                        report.left.push(name);
                    }
                }
            } else {
                report.left.push(name);
            }
        }
    }
    if report.left.is_empty() {
        // Empty now (a concurrent writer would make this fail, which is fine).
        let _ = tokio::fs::remove_dir(&legacy_sessions).await;
    }
    // 3. Node journal and hub store, when this machine has none yet.
    for d in ["wasl", "store"] {
        let from = root.join(d);
        let to = layout.data.join(d);
        if from.is_dir() && !to.exists() {
            if let Err(e) = tokio::fs::rename(&from, &to).await {
                tracing::warn!(error = %e, "cannot move {}", from.display());
            }
        }
    }
    let log = root.join("marsad-agent.log");
    if log.is_file() && !layout.log().exists() {
        let _ = tokio::fs::rename(&log, layout.log()).await;
    }
    // Stale legacy endpoint files never block anything, but are removed by their owner.
    if lock.is_some() {
        let _ = tokio::fs::remove_file(&legacy_socket).await;
        drop(lock);
        let _ = tokio::fs::remove_file(&legacy_lock).await;
    }
    report
}

async fn take_legacy_lock(path: &Path) -> Option<InstanceLock> {
    if !path.exists() {
        return None;
    }
    match InstanceLock::try_acquire(path).await {
        Ok(l) => Some(l),
        Err(LockError::Held(_)) => None,
        Err(e) => {
            tracing::warn!(error = %e, "cannot check the legacy daemon lock");
            None
        }
    }
}

async fn wait_legacy_lock(path: &Path) -> Option<InstanceLock> {
    let mut delay = Duration::from_millis(5);
    for _ in 0..12 {
        if let Some(l) = take_legacy_lock(path).await {
            return Some(l);
        }
        // Backoff schedule while the stopped daemon exits.
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_millis(500));
    }
    None
}

/// Whether a legacy session directory belongs to this machine (see the module docs):
/// `Some(live)` when it does.
async fn claimable(dir: &Path) -> Option<bool> {
    match wasit_shim::probe(dir).await {
        Ok(ShimState::Exited(_)) => Some(false),
        Ok(ShimState::Running) => matches!(
            tokio::time::timeout(Duration::from_secs(3), ShimClient::connect(dir)).await,
            Ok(Ok(_))
        )
        .then_some(true),
        Ok(ShimState::Dead) => {
            let journal = dir.join(wasit_shim::JOURNAL_FILE);
            let recent = tokio::fs::metadata(&journal)
                .await
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .is_some_and(|age| age < RECENT);
            (!recent).then_some(false)
        }
        Err(_) => None,
    }
}

/// A live shim of an earlier build keeps its lock and socket in its (moved) data directory:
/// point this machine's runtime directory for the session at them, so the runtime finds it
/// where it looks for every session (and the socket path stays short).
#[cfg(unix)]
async fn link_runtime(layout: &Layout, name: &str, data: &Path) {
    let run = layout.run.join("sessions").join(name);
    if let Err(e) = tokio::fs::create_dir_all(&run).await {
        tracing::warn!(session = %name, error = %e, "cannot create {}", run.display());
        return;
    }
    for f in [wasit_shim::LOCK_FILE, "ctl.sock"] {
        if let Err(e) = tokio::fs::symlink(data.join(f), run.join(f)).await {
            tracing::warn!(session = %name, error = %e, "cannot link {f}");
        }
    }
}
