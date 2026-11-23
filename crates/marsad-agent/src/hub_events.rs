//! The typed, content-free diagnostics of the hub's and the host's background work (`src` `hub`
//! and `daemon`): what the git watcher, the pty services, the workstream actor, the launch of a
//! workstream and the adoption of an earlier data layout record about themselves, in place of
//! free-form log lines.
//!
//! Every field is one of `wasit-diag`'s value types (see [`crate::diag`]): ids are [`Ident`]s,
//! reasons are fixed [`Code`]s, the data directories are [`HomePath`]s, and an error is either an
//! I/O error ([`diag::io`]) or a text from outside this code ([`diag::foreign`]: code, length and
//! fingerprint, never the text). Names a person typed (workstream, node, session display names,
//! file names in a project) and project directories are never recorded here.

use std::fmt::Display;
use std::io;
use std::path::Path;

use tokio::sync::mpsc::error::TrySendError;
use wasit_diag::{Code, ErrorInfo, HomePath, Ident};

use crate::diag;
use crate::migrate::Report;

wasit_diag::diag_events! {
    /// Events of the hub's and the host's background services (`src` `hub` or `daemon`).
    pub enum HubEvent {
        /// A terminal's shell could not be killed when the terminal was closed (the shell may
        /// have exited on its own at the same moment).
        #[diag(ev = "pty.kill_failed", lvl = debug)]
        PtyKillFailed { pty: Ident, error: ErrorInfo },
        /// A terminal's writer thread could not be started: its input goes nowhere.
        #[diag(ev = "pty.writer_failed", lvl = error)]
        PtyWriterFailed { error: ErrorInfo },
        /// A git summary was not scheduled: the watcher's queue was full (the directory is
        /// refreshed by the next touch) or closed (the watcher is gone). `code` says which.
        #[diag(ev = "git.touch_dropped", lvl = debug)]
        GitTouchDropped { error: ErrorInfo },
        /// A git summary task failed (it panicked or was cancelled).
        #[diag(ev = "git.summary_failed", lvl = error)]
        GitSummaryFailed { error: ErrorInfo },
        /// The drivers of a host could not be resolved from its inventory topic (the answer
        /// stays what the host reported first).
        #[diag(ev = "spawn.inventory_unavailable", lvl = debug)]
        InventoryUnavailable { host: Ident, error: ErrorInfo },
        /// A session started for a spawned node could not be stopped (it may have ended on its
        /// own already).
        #[diag(ev = "spawn.session_stop_failed", lvl = info)]
        SpawnedSessionStopFailed {
            host: Ident,
            session: Ident,
            error: ErrorInfo,
        },
        /// A fleet event could not be decoded and is dropped.
        #[diag(ev = "workstreams.fleet_undecodable", lvl = error)]
        FleetEventUndecodable { error: ErrorInfo },
        /// The fleet feed could not be followed; it is retried with a backoff.
        #[diag(ev = "workstreams.fleet_follow_failed", lvl = warn)]
        FleetFollowFailed { error: ErrorInfo },
        /// A workstream's topology could not be pushed to a host (the actor records the failure
        /// and pushes again later).
        #[diag(ev = "workstreams.topology_push_failed", lvl = warn)]
        TopologyPushFailed {
            workstream: Ident,
            host: Ident,
            error: ErrorInfo,
        },
        /// A host node's feed of tasks or edges ended; the workstream stops following it.
        #[diag(ev = "workstreams.node_feed_ended", lvl = warn)]
        NodeFeedEnded {
            host: Ident,
            topic: Code,
            error: ErrorInfo,
        },
        /// An event of a host node's feed could not be decoded and is dropped.
        #[diag(ev = "workstreams.node_event_undecodable", lvl = warn)]
        NodeEventUndecodable {
            host: Ident,
            topic: Code,
            error: ErrorInfo,
        },
        /// An always-allow decision on an edge could not be recorded in the workstream.
        #[diag(ev = "workstreams.always_allow_failed", lvl = error)]
        AlwaysAllowFailed { workstream: Ident, error: ErrorInfo },
        /// A topology push failed with a retryable error; it is tried again on a backoff.
        #[diag(ev = "workstreams.topology_push_retry", lvl = debug)]
        TopologyPushRetry { host: Ident, error: ErrorInfo },
        /// A launched node's session could not be renamed after its node.
        #[diag(ev = "launch.node_name_failed", lvl = warn)]
        NodeNameFailed { node: Ident, error: ErrorInfo },
        /// A node of a workstream could not be launched (the error is also in the reply).
        #[diag(ev = "launch.node_failed", lvl = warn)]
        NodeLaunchFailed {
            workstream: Ident,
            node: Ident,
            error: ErrorInfo,
        },
        /// A workstream's topology could not be pushed to a host at launch (it is retried).
        #[diag(ev = "launch.topology_push_failed", lvl = warn)]
        LaunchTopologyPushFailed {
            workstream: Ident,
            host: Ident,
            error: ErrorInfo,
        },
        /// The first prompt of a launched node was not delivered to its session.
        #[diag(ev = "launch.first_prompt_failed", lvl = warn)]
        FirstPromptFailed {
            workstream: Ident,
            node: Ident,
            error: ErrorInfo,
        },
        /// The earlier flat data layout was adopted (`stopped_daemon`: an earlier build's daemon
        /// on this machine was asked to exit; `foreign_daemon`: the layout belongs to another
        /// machine's daemon and was left alone). `claimed` and `left` count the session
        /// directories that were moved here and that were left where they were.
        #[diag(ev = "migrate.adopted", lvl = info)]
        MigrateAdopted {
            foreign_daemon: bool,
            stopped_daemon: bool,
            claimed: u64,
            left: u64,
        },
        /// The earlier data layout belongs to another machine's daemon: it is left alone.
        #[diag(ev = "migrate.foreign_daemon", lvl = info)]
        MigrateForeignDaemon { root: HomePath },
        /// The sessions directory of this machine could not be created; nothing is adopted.
        #[diag(ev = "migrate.sessions_dir_failed", lvl = warn)]
        MigrateSessionsDirFailed { path: HomePath, error: ErrorInfo },
        /// The old path of a live legacy session could not be linked to its moved directory.
        #[diag(ev = "migrate.session_link_failed", lvl = warn)]
        MigrateSessionLinkFailed { session: Ident, error: ErrorInfo },
        /// A legacy session directory could not be moved here (it stays where it was).
        #[diag(ev = "migrate.session_claim_failed", lvl = warn)]
        MigrateSessionClaimFailed { session: Ident, error: ErrorInfo },
        /// A legacy node journal or hub store could not be moved here (it stays where it was).
        #[diag(ev = "migrate.move_failed", lvl = warn)]
        MigrateMoveFailed { path: HomePath, error: ErrorInfo },
        /// The legacy daemon's lock could not be checked; it is taken as held.
        #[diag(ev = "migrate.legacy_lock_failed", lvl = warn)]
        MigrateLegacyLockFailed { error: ErrorInfo },
        /// The runtime directory of an adopted live session could not be created.
        #[diag(ev = "migrate.runtime_dir_failed", lvl = warn)]
        MigrateRuntimeDirFailed {
            session: Ident,
            path: HomePath,
            error: ErrorInfo,
        },
        /// A file of an adopted live session could not be linked into the runtime directory;
        /// `file` names it (one of the session's fixed file names).
        #[diag(ev = "migrate.runtime_link_failed", lvl = warn)]
        MigrateRuntimeLinkFailed {
            session: Ident,
            file: Code,
            error: ErrorInfo,
        },
    }
}

/// Records `event` in this process's sink (never waits; see [`crate::diag::emit`], which takes
/// only `MarsadEvent`s).
fn record(event: HubEvent) {
    wasit_diag::emit(&event);
}

/// An id as a record field (validated: anything that is not an identifier shows as its length).
fn ident(id: &impl Display) -> Ident {
    Ident::new(&id.to_string())
}

/// An error whose text came from outside this code (a peer, a store, a decoder, a lock file):
/// its code, length and fingerprint only.
fn foreign(code: &'static str, e: &impl Display) -> ErrorInfo {
    diag::foreign(code, &e.to_string())
}

/// The terminal `pty`'s shell could not be killed.
pub fn pty_kill_failed(pty: &str, e: &io::Error) {
    record(HubEvent::PtyKillFailed {
        pty: Ident::new(pty),
        error: diag::io(e),
    });
}

/// A terminal's writer thread could not be started.
pub fn pty_writer_failed(e: &io::Error) {
    record(HubEvent::PtyWriterFailed { error: diag::io(e) });
}

/// A git summary touch was dropped; the queue was full or closed.
pub fn git_touch_dropped<T>(e: &TrySendError<T>) {
    let code = match e {
        TrySendError::Full(_) => "queue_full",
        TrySendError::Closed(_) => "closed",
    };
    record(HubEvent::GitTouchDropped {
        error: foreign(code, e),
    });
}

/// A git summary task failed.
pub fn git_summary_failed(e: &impl Display) {
    record(HubEvent::GitSummaryFailed {
        error: foreign("task_failed", e),
    });
}

/// The inventory of `host` could not be followed to resolve its drivers.
pub fn inventory_unavailable(host: &impl Display, e: &impl Display) {
    record(HubEvent::InventoryUnavailable {
        host: ident(host),
        error: foreign("inventory_unavailable", e),
    });
}

/// A spawned session could not be stopped.
pub fn spawned_session_stop_failed(host: &impl Display, session: &impl Display, e: &impl Display) {
    record(HubEvent::SpawnedSessionStopFailed {
        host: ident(host),
        session: ident(session),
        error: foreign("session_stop", e),
    });
}

/// A fleet event could not be decoded.
pub fn fleet_event_undecodable(e: &impl Display) {
    record(HubEvent::FleetEventUndecodable {
        error: foreign("undecodable", e),
    });
}

/// The fleet feed could not be followed.
pub fn fleet_follow_failed(e: &impl Display) {
    record(HubEvent::FleetFollowFailed {
        error: foreign("fleet_follow", e),
    });
}

/// A topology push to `host` failed for workstream `workstream`.
pub fn topology_push_failed(workstream: &impl Display, host: &impl Display, e: &impl Display) {
    record(HubEvent::TopologyPushFailed {
        workstream: ident(workstream),
        host: ident(host),
        error: foreign("topology_push", e),
    });
}

/// A host node's feed of `topic` ended.
pub fn node_feed_ended(host: &impl Display, topic: &'static str, e: &impl Display) {
    record(HubEvent::NodeFeedEnded {
        host: ident(host),
        topic: Code::st(topic),
        error: foreign("node_feed", e),
    });
}

/// An event of a host node's feed of `topic` could not be decoded.
pub fn node_event_undecodable(host: &impl Display, topic: &'static str, e: &impl Display) {
    record(HubEvent::NodeEventUndecodable {
        host: ident(host),
        topic: Code::st(topic),
        error: foreign("undecodable", e),
    });
}

/// An always-allow decision could not be recorded in `workstream`.
pub fn always_allow_failed(workstream: &impl Display, e: &impl Display) {
    record(HubEvent::AlwaysAllowFailed {
        workstream: ident(workstream),
        error: foreign("always_allow", e),
    });
}

/// A topology push to `host` failed with a retryable error.
pub fn topology_push_retry(host: &impl Display, e: &impl Display) {
    record(HubEvent::TopologyPushRetry {
        host: ident(host),
        error: foreign("topology_push_retry", e),
    });
}

/// The session of launched `node` could not be renamed.
pub fn node_name_failed(node: &impl Display, e: &impl Display) {
    record(HubEvent::NodeNameFailed {
        node: ident(node),
        error: foreign("session_rename", e),
    });
}

/// `node` of `workstream` could not be launched.
pub fn node_launch_failed(workstream: &impl Display, node: &impl Display, e: &impl Display) {
    record(HubEvent::NodeLaunchFailed {
        workstream: ident(workstream),
        node: ident(node),
        error: foreign("node_launch", e),
    });
}

/// The topology of `workstream` could not be pushed to `host` at launch.
pub fn launch_topology_push_failed(
    workstream: &impl Display,
    host: &impl Display,
    e: &impl Display,
) {
    record(HubEvent::LaunchTopologyPushFailed {
        workstream: ident(workstream),
        host: ident(host),
        error: foreign("topology_push", e),
    });
}

/// The first prompt of `node` of `workstream` was not delivered.
pub fn first_prompt_failed(workstream: &impl Display, node: &impl Display, e: &impl Display) {
    record(HubEvent::FirstPromptFailed {
        workstream: ident(workstream),
        node: ident(node),
        error: foreign("first_prompt", e),
    });
}

/// The earlier data layout was adopted (or was looked at and found to hold something).
pub fn migrate_adopted(r: &Report) {
    record(HubEvent::MigrateAdopted {
        foreign_daemon: r.foreign_daemon,
        stopped_daemon: r.stopped_daemon,
        claimed: r.claimed.len() as u64,
        left: r.left.len() as u64,
    });
}

/// The earlier layout at `root` belongs to another machine's daemon.
pub fn migrate_foreign_daemon(root: &Path) {
    record(HubEvent::MigrateForeignDaemon {
        root: HomePath::new(root),
    });
}

/// The sessions directory `path` could not be created.
pub fn migrate_sessions_dir_failed(path: &Path, e: &io::Error) {
    record(HubEvent::MigrateSessionsDirFailed {
        path: HomePath::new(path),
        error: diag::io(e),
    });
}

/// The old path of legacy session `session` could not be linked.
pub fn migrate_session_link_failed(session: &impl Display, e: &io::Error) {
    record(HubEvent::MigrateSessionLinkFailed {
        session: ident(session),
        error: diag::io(e),
    });
}

/// Legacy session `session` could not be claimed.
pub fn migrate_session_claim_failed(session: &impl Display, e: &io::Error) {
    record(HubEvent::MigrateSessionClaimFailed {
        session: ident(session),
        error: diag::io(e),
    });
}

/// The legacy path `path` could not be moved here.
pub fn migrate_move_failed(path: &Path, e: &io::Error) {
    record(HubEvent::MigrateMoveFailed {
        path: HomePath::new(path),
        error: diag::io(e),
    });
}

/// The legacy daemon's lock could not be checked.
pub fn migrate_legacy_lock_failed(e: &impl Display) {
    record(HubEvent::MigrateLegacyLockFailed {
        error: foreign("lock_check", e),
    });
}

/// The runtime directory `path` of adopted session `session` could not be created.
pub fn migrate_runtime_dir_failed(session: &impl Display, path: &Path, e: &io::Error) {
    record(HubEvent::MigrateRuntimeDirFailed {
        session: ident(session),
        path: HomePath::new(path),
        error: diag::io(e),
    });
}

/// File `file` of adopted session `session` could not be linked into the runtime directory.
pub fn migrate_runtime_link_failed(session: &impl Display, file: &'static str, e: &io::Error) {
    record(HubEvent::MigrateRuntimeLinkFailed {
        session: ident(session),
        file: Code::st(file),
        error: diag::io(e),
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::time::Duration;

    use wasit_diag::{Sink, SinkConfig};

    use super::*;

    const HEADER: [&str; 7] = ["b", "seq", "t", "m", "src", "lvl", "ev"];

    /// Every variant, once, with its expected `ev`, `lvl` and declared fields. Adding a variant
    /// means adding its case here.
    fn cases() -> Vec<(
        HubEvent,
        &'static str,
        &'static str,
        &'static [&'static str],
    )> {
        let io_err = || io::Error::from(io::ErrorKind::PermissionDenied);
        let foreign_text = || diag::foreign("x", "SECRET PROMPT TEXT");
        vec![
            (
                HubEvent::PtyKillFailed {
                    pty: Ident::st("pty-1"),
                    error: diag::io(&io_err()),
                },
                "pty.kill_failed",
                "debug",
                &["pty", "error"],
            ),
            (
                HubEvent::PtyWriterFailed {
                    error: diag::io(&io_err()),
                },
                "pty.writer_failed",
                "error",
                &["error"],
            ),
            (
                HubEvent::GitTouchDropped {
                    error: foreign_text(),
                },
                "git.touch_dropped",
                "debug",
                &["error"],
            ),
            (
                HubEvent::GitSummaryFailed {
                    error: foreign_text(),
                },
                "git.summary_failed",
                "error",
                &["error"],
            ),
            (
                HubEvent::InventoryUnavailable {
                    host: Ident::st("host-1"),
                    error: foreign_text(),
                },
                "spawn.inventory_unavailable",
                "debug",
                &["host", "error"],
            ),
            (
                HubEvent::SpawnedSessionStopFailed {
                    host: Ident::st("host-1"),
                    session: Ident::st("sess-1"),
                    error: foreign_text(),
                },
                "spawn.session_stop_failed",
                "info",
                &["host", "session", "error"],
            ),
            (
                HubEvent::FleetEventUndecodable {
                    error: foreign_text(),
                },
                "workstreams.fleet_undecodable",
                "error",
                &["error"],
            ),
            (
                HubEvent::FleetFollowFailed {
                    error: foreign_text(),
                },
                "workstreams.fleet_follow_failed",
                "warn",
                &["error"],
            ),
            (
                HubEvent::TopologyPushFailed {
                    workstream: Ident::st("ws-1"),
                    host: Ident::st("host-1"),
                    error: foreign_text(),
                },
                "workstreams.topology_push_failed",
                "warn",
                &["workstream", "host", "error"],
            ),
            (
                HubEvent::NodeFeedEnded {
                    host: Ident::st("host-1"),
                    topic: Code::st("wasl.tasks"),
                    error: foreign_text(),
                },
                "workstreams.node_feed_ended",
                "warn",
                &["host", "topic", "error"],
            ),
            (
                HubEvent::NodeEventUndecodable {
                    host: Ident::st("host-1"),
                    topic: Code::st("wasl.edges"),
                    error: foreign_text(),
                },
                "workstreams.node_event_undecodable",
                "warn",
                &["host", "topic", "error"],
            ),
            (
                HubEvent::AlwaysAllowFailed {
                    workstream: Ident::st("ws-1"),
                    error: foreign_text(),
                },
                "workstreams.always_allow_failed",
                "error",
                &["workstream", "error"],
            ),
            (
                HubEvent::TopologyPushRetry {
                    host: Ident::st("host-1"),
                    error: foreign_text(),
                },
                "workstreams.topology_push_retry",
                "debug",
                &["host", "error"],
            ),
            (
                HubEvent::NodeNameFailed {
                    node: Ident::st("node-1"),
                    error: foreign_text(),
                },
                "launch.node_name_failed",
                "warn",
                &["node", "error"],
            ),
            (
                HubEvent::NodeLaunchFailed {
                    workstream: Ident::st("ws-1"),
                    node: Ident::st("node-1"),
                    error: foreign_text(),
                },
                "launch.node_failed",
                "warn",
                &["workstream", "node", "error"],
            ),
            (
                HubEvent::LaunchTopologyPushFailed {
                    workstream: Ident::st("ws-1"),
                    host: Ident::st("host-1"),
                    error: foreign_text(),
                },
                "launch.topology_push_failed",
                "warn",
                &["workstream", "host", "error"],
            ),
            (
                HubEvent::FirstPromptFailed {
                    workstream: Ident::st("ws-1"),
                    node: Ident::st("node-1"),
                    error: foreign_text(),
                },
                "launch.first_prompt_failed",
                "warn",
                &["workstream", "node", "error"],
            ),
            (
                HubEvent::MigrateAdopted {
                    foreign_daemon: false,
                    stopped_daemon: true,
                    claimed: 2,
                    left: 1,
                },
                "migrate.adopted",
                "info",
                &["foreign_daemon", "stopped_daemon", "claimed", "left"],
            ),
            (
                HubEvent::MigrateForeignDaemon {
                    root: HomePath::new(&PathBuf::from("/home/u/.marsad")),
                },
                "migrate.foreign_daemon",
                "info",
                &["root"],
            ),
            (
                HubEvent::MigrateSessionsDirFailed {
                    path: HomePath::new(&PathBuf::from("/home/u/.marsad/sessions")),
                    error: diag::io(&io_err()),
                },
                "migrate.sessions_dir_failed",
                "warn",
                &["path", "error"],
            ),
            (
                HubEvent::MigrateSessionLinkFailed {
                    session: Ident::st("sess-1"),
                    error: diag::io(&io_err()),
                },
                "migrate.session_link_failed",
                "warn",
                &["session", "error"],
            ),
            (
                HubEvent::MigrateSessionClaimFailed {
                    session: Ident::st("sess-1"),
                    error: diag::io(&io_err()),
                },
                "migrate.session_claim_failed",
                "warn",
                &["session", "error"],
            ),
            (
                HubEvent::MigrateMoveFailed {
                    path: HomePath::new(&PathBuf::from("/home/u/.marsad/wasl")),
                    error: diag::io(&io_err()),
                },
                "migrate.move_failed",
                "warn",
                &["path", "error"],
            ),
            (
                HubEvent::MigrateLegacyLockFailed {
                    error: foreign_text(),
                },
                "migrate.legacy_lock_failed",
                "warn",
                &["error"],
            ),
            (
                HubEvent::MigrateRuntimeDirFailed {
                    session: Ident::st("sess-1"),
                    path: HomePath::new(&PathBuf::from("/home/u/.marsad/run/sessions/sess-1")),
                    error: diag::io(&io_err()),
                },
                "migrate.runtime_dir_failed",
                "warn",
                &["session", "path", "error"],
            ),
            (
                HubEvent::MigrateRuntimeLinkFailed {
                    session: Ident::st("sess-1"),
                    file: Code::st("shim.lock"),
                    error: diag::io(&io_err()),
                },
                "migrate.runtime_link_failed",
                "warn",
                &["session", "file", "error"],
            ),
        ]
    }

    #[test]
    fn every_event_writes_the_header_and_its_declared_fields_only() {
        let cases = cases();
        let dir = std::env::temp_dir().join(format!("marsad-hub-events-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = "events.ndjson";
        let _ = std::fs::remove_file(dir.join(file));
        let cfg = SinkConfig::new(Code::st("daemon"), Ident::st("b1"), dir.clone(), file);
        let sink = Sink::start(cfg).unwrap();
        for (e, _, _, _) in &cases {
            sink.emit(e);
        }
        assert!(sink.shutdown(Duration::from_secs(30)));
        let text = std::fs::read_to_string(dir.join(file)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            lines.len(),
            cases.len() + 1,
            "header and one record each: {text}"
        );
        for (rec, (_, ev, lvl, fields)) in lines[1..].iter().zip(&cases) {
            let keys: BTreeSet<&str> = rec
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            let want: BTreeSet<&str> = HEADER.iter().chain(fields.iter()).copied().collect();
            assert_eq!(keys, want, "{rec}");
            assert_eq!(
                (rec["ev"].as_str(), rec["lvl"].as_str()),
                (Some(*ev), Some(*lvl)),
                "{rec}"
            );
            assert!(!rec.to_string().contains("SECRET PROMPT TEXT"), "{rec}");
        }
    }
}
