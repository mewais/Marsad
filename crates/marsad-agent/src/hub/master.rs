//! A host's shared ssh connection (Linux and macOS hubs).
//!
//! The hub keeps ONE authenticated ssh connection open per ssh host, the "master", and every
//! command that talks to the host rides it ([`super::transport::SshRoute`]). The user signs in
//! once; the connection belongs to nobody but ssh (detached into the background with
//! `ControlPersist=yes`), so it outlives the hub daemon and the app: a restart finds it again
//! with `ssh -O check` and reconnects without a prompt. It stays open until the user signs out
//! ([`super::hosts::Registry::signout`]), the host is removed or its route is edited (both end it
//! here, [`Masters::exit`]), or the connection itself dies (a keep-alive timeout, the other end
//! closing it, a kill): the next attempt of a key host signs in again by itself, one of a host
//! that needs the user waits for them. While it is open, any program of the same local user can
//! run commands on the host without signing in again (the same trust as an ssh agent; the
//! owner accepted it).
//!
//! Files: `<run>/ssh/<16 hex>` is the control socket, named by [`control_socket`] from the
//! host's id and route (so an edited route never rides the old connection), and
//! `<run>/ssh/<16 hex>.log` the connection's own log (`ssh -E`): what the detached ssh writes
//! when it ends (a keep-alive timeout, for one). The log is recreated at every sign-in, so it
//! only ever holds the life of the last connection; it is read, bounded ([`LOG_TAIL_BYTES`],
//! [`LOG_TAIL_LINES`]), when the connection is found ended, and deleted with the connection.
//! ssh only logs problems at its default level, so a live connection's log stays small; one
//! that grows past [`LOG_MAX_BYTES`] is cut to nothing the next time the connection is found
//! alive.
//!
//! Nothing here waits for a time to pass: asking the connection (`-O check`, `-O exit`) is a
//! local command on a Unix socket whose answer is the process's exit, bounded by wasit-control's
//! [`CLOSED_EXIT_WAIT`] (the bound it uses for any child that is about to exit) only so that a
//! wedged master turns into a recorded failure instead of a hang.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use marsad_proto::diag::{HostsOp, MarsadEvent, MasterExitOutcome, MasterExitWhy, MasterState};
use marsad_proto::hosts::{HostEntry, HostKind};
use marsad_proto::HostId;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use wasit_control::{CommandSpec, CommandTransport, CLOSED_EXIT_WAIT};
use wasit_diag::Ident;

use super::transport::{
    control_path_fits, control_socket, master_command_argv, master_log, socket_path_limit,
    MasterCommand, SOCKET_TEMP_SUFFIX,
};
use crate::diag::{self, emit};

/// Bytes read from the end of a connection's log when it is found ended.
pub const LOG_TAIL_BYTES: u64 = 4096;
/// Lines of them that are kept (the last ones).
pub const LOG_TAIL_LINES: usize = 3;
/// Characters of the kept lines in all.
pub const LOG_TAIL_CHARS: usize = 400;
/// A log of a live connection is cut to nothing when it is larger than this.
pub const LOG_MAX_BYTES: u64 = 64 * 1024;

/// What a look at a host's shared connection found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// It is open and answered.
    Alive,
    /// There is none (no socket, or a socket nobody listens on, which was removed).
    Absent,
    /// The check itself failed or did not answer; ssh's words.
    Unanswered(String),
}

#[derive(Default)]
struct HostMaster {
    /// The connection was seen open (a check answered, or a sign-in ended with it open) and has
    /// not been seen ended since: finding it gone is news.
    alive: bool,
    /// The user signed out: no attempt may sign in again until they ask (`hosts.reconnect`).
    signed_out: bool,
    /// Why the connection was found ended, in ssh's words (empty: ssh wrote nothing), until the
    /// next sign-in begins.
    ended: Option<String>,
    /// A counter bumped at every sign-out: a sign-in that began under an older generation ends
    /// its own master instead of keeping it.
    generation: u64,
}

struct Inner {
    /// `<run>/ssh`.
    dir: PathBuf,
    hosts: Mutex<HashMap<HostId, HostMaster>>,
}

/// The shared connections of the hub's ssh hosts. Cheap to clone.
#[derive(Clone)]
pub struct Masters {
    inner: Arc<Inner>,
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a local ssh command said.
struct Ran {
    exit: Option<i32>,
    stderr: String,
}

/// Runs `argv` (a local ssh command: `-O check` / `-O exit`) and waits for it to end. `Err`:
/// it could not be started, or did not end in time.
async fn run_local(argv: Vec<String>) -> Result<Ran, String> {
    let mut spec = CommandSpec::new(argv);
    // ssh prints errno texts: ask for English, as the words below are matched.
    spec.env = vec![("LC_ALL".into(), "C".into())];
    let (_io, child) = CommandTransport::spawn(&spec).map_err(|e| e.to_string())?;
    match child.wait_exit(CLOSED_EXIT_WAIT).await {
        Some(x) => Ok(Ran {
            exit: x.code,
            stderr: child.stderr_tail(),
        }),
        // Dropping the child kills it.
        None => Err("ssh did not answer".to_owned()),
    }
}

fn fs_failed(host: &HostId, e: &std::io::Error) {
    emit(MarsadEvent::HostsFailed {
        host: Some(Ident::new(host.as_str())),
        op: HostsOp::SharedConnection,
        error: diag::io(e),
    });
}

impl Masters {
    /// Whether this hub shares connections at all (a Windows hub's ssh client cannot).
    pub const AVAILABLE: bool = cfg!(unix);

    /// `dir` is `<run>/ssh`.
    pub fn new(dir: PathBuf) -> Masters {
        Masters {
            inner: Arc::new(Inner {
                dir,
                hosts: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Whether `entry`'s connection is shared: an ssh host on a hub that can.
    pub fn applies(entry: &HostEntry) -> bool {
        Self::AVAILABLE && matches!(entry.kind, HostKind::Ssh { .. })
    }

    /// The control socket of `entry`'s connection, or why there can be none: ssh refuses a
    /// path it cannot add its temporary suffix to ([`control_path_fits`]), which only an
    /// unusually long runtime directory (`MARSAD_RUNTIME_DIR`) causes.
    pub fn path(&self, entry: &HostEntry) -> Result<PathBuf, String> {
        let p = control_socket(&self.inner.dir, entry.id.as_str(), &entry.kind);
        let limit = socket_path_limit();
        if control_path_fits(&p, limit) {
            Ok(p)
        } else {
            Err(format!(
                "the shared connection's socket path is too long for this system ({} bytes, ssh needs {SOCKET_TEMP_SUFFIX} more and the limit is {}): use a shorter runtime directory ({})",
                p.as_os_str().len(),
                limit - 1,
                p.display()
            ))
        }
    }

    /// Makes sure `<run>/ssh` exists, private to the user.
    pub async fn prepare(&self) -> std::io::Result<()> {
        let dir = self.inner.dir.clone();
        tokio::task::spawn_blocking(move || wasit_runtime::machine::ensure_private_dir(&dir))
            .await
            .map_err(std::io::Error::other)?
    }

    fn with<T>(&self, host: &HostId, f: impl FnOnce(&mut HostMaster) -> T) -> T {
        f(locked(&self.inner.hosts).entry(host.clone()).or_default())
    }

    /// The host is gone or its route changed: nothing is remembered about its connection.
    pub fn forget(&self, host: &HostId) {
        locked(&self.inner.hosts).remove(host);
    }

    /// The user signed out of `host` (or asked to connect again: `false`).
    pub fn set_signed_out(&self, host: &HostId, on: bool) {
        self.with(host, |h| h.signed_out = on);
    }

    /// The user signed out: record it and bump the host's generation so that in-flight sign-ins
    /// started before it end their own master.
    pub fn mark_signed_out(&self, host: &HostId) {
        self.with(host, |h| {
            h.signed_out = true;
            h.generation = h.generation.wrapping_add(1);
            h.alive = false;
        });
    }

    pub fn signed_out(&self, host: &HostId) -> bool {
        locked(&self.inner.hosts)
            .get(host)
            .is_some_and(|h| h.signed_out)
    }

    /// Why the connection of `host` was found ended, as the user is told (ssh's words in
    /// parentheses when it wrote any); `None` when it was not found ended since the last
    /// sign-in.
    pub fn ended_words(&self, host: &HostId) -> Option<String> {
        locked(&self.inner.hosts)
            .get(host)
            .and_then(|h| h.ended.as_ref())
            .map(|r| {
                if r.is_empty() {
                    "the shared connection to this host ended".to_owned()
                } else {
                    format!("the shared connection to this host ended ({r})")
                }
            })
    }

    /// A sign-in step is about to start: the log of the connection before it is no longer
    /// wanted and what was found about its end is told. A log that cannot be removed is
    /// recorded, and the step goes on (ssh appends to what is there).
    pub async fn begin_signin(&self, entry: &HostEntry, control: &Path) -> u64 {
        let generation = self.with(&entry.id, |h| {
            h.ended = None;
            h.alive = false;
            h.generation
        });
        remove_file(&entry.id, &master_log(control)).await;
        generation
    }

    /// A sign-in step ended with the connection open.
    pub fn signed_in(&self, host: &HostId, started: u64) -> bool {
        self.with(host, |h| {
            if h.generation == started {
                h.alive = true;
                h.signed_out = false;
                true
            } else {
                false
            }
        })
    }

    /// What a failed sign-in step wrote to the connection's log (ssh's messages go there, not to
    /// its stderr), bounded like a transport command's stderr; the log is removed.
    pub async fn signin_failure_text(&self, control: &Path) -> String {
        let log = master_log(control);
        let text = read_tail(&log, 16 * 1024).await.unwrap_or_default();
        let _ = tokio::fs::remove_file(&log).await;
        text
    }

    /// Looks for the connection of `entry` (`ssh -O check`: it only talks to the local
    /// socket). A socket left by a killed master is removed (ssh would refuse to listen on it).
    /// An answer other than "open" is also when the connection is found to have ended, if it
    /// was seen open before (or its log has something to say): that is recorded with ssh's
    /// reason. `attempt`: the connect attempt asking, `None` for the supervisor's question
    /// whether the host must wait for the user.
    pub async fn check(&self, entry: &HostEntry, control: &Path, attempt: Option<u64>) -> Check {
        let began = Instant::now();
        let (state, error, result) = self.look(entry, control).await;
        emit(MarsadEvent::SshMasterCheck {
            host: Ident::new(entry.id.as_str()),
            n: attempt,
            state,
            ms: diag::ms(began),
            error: error.as_deref().map(diag::transport),
        });
        match &result {
            Check::Alive => {
                self.with(&entry.id, |h| h.alive = true);
                self.trim_log(&entry.id, control).await;
            }
            Check::Absent => self.found_ended(entry, control, attempt).await,
            Check::Unanswered(_) => {}
        }
        result
    }

    async fn look(
        &self,
        entry: &HostEntry,
        control: &Path,
    ) -> (MasterState, Option<String>, Check) {
        match tokio::fs::symlink_metadata(control).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (MasterState::Absent, None, Check::Absent)
            }
            Err(e) => {
                let words = format!("{}: {e}", control.display());
                return (
                    MasterState::Unanswered,
                    Some(words.clone()),
                    Check::Unanswered(words),
                );
            }
        }
        let Some(argv) = master_command_argv(&entry.kind, control, MasterCommand::Check) else {
            return (MasterState::Absent, None, Check::Absent);
        };
        match run_local(argv).await {
            Ok(Ran { exit: Some(0), .. }) => (MasterState::Alive, None, Check::Alive),
            // Nothing listens: the master was killed and left its socket.
            Ok(Ran { stderr, .. }) if stderr.contains("Connection refused") => {
                if let Err(e) = tokio::fs::remove_file(control).await {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        fs_failed(&entry.id, &e);
                    }
                }
                (MasterState::Stale, None, Check::Absent)
            }
            Ok(Ran { exit, stderr }) => {
                let words = if stderr.is_empty() {
                    format!("ssh -O check ended with {exit:?}")
                } else {
                    stderr
                };
                (
                    MasterState::Unanswered,
                    Some(words.clone()),
                    Check::Unanswered(words),
                )
            }
            Err(words) => (
                MasterState::Unanswered,
                Some(words.clone()),
                Check::Unanswered(words),
            ),
        }
    }

    /// No connection is there. If one was seen open, or its log holds something, say it ended
    /// and why; otherwise nothing existed that could end.
    async fn found_ended(&self, entry: &HostEntry, control: &Path, attempt: Option<u64>) {
        let was_alive = self.with(&entry.id, |h| std::mem::take(&mut h.alive));
        let log = master_log(control);
        let tail = read_tail(&log, LOG_TAIL_BYTES)
            .await
            .map(|t| last_lines(&t))
            .filter(|t| !t.is_empty());
        if !was_alive && tail.is_none() {
            return;
        }
        emit(MarsadEvent::SshMasterEnded {
            host: Ident::new(entry.id.as_str()),
            n: attempt,
            reason: tail.as_deref().map(diag::transport),
        });
        self.with(&entry.id, |h| h.ended = Some(tail.unwrap_or_default()));
        remove_file(&entry.id, &log).await;
    }

    /// A live connection's log that has grown past [`LOG_MAX_BYTES`] is cut to nothing (ssh
    /// appends, so a cut cannot hurt it).
    async fn trim_log(&self, host: &HostId, control: &Path) {
        let log = master_log(control);
        let Ok(meta) = tokio::fs::metadata(&log).await else {
            return;
        };
        if meta.len() > LOG_MAX_BYTES {
            let cut = tokio::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&log)
                .await;
            if let Err(e) = cut {
                fs_failed(host, &e);
            }
        }
    }

    /// Ends the connection of `entry` (`ssh -O exit`), for `why`. Every outcome is recorded: it
    /// ended, there was none, or the request failed (the text is ssh's). The connection's log
    /// goes with it.
    pub async fn exit(&self, entry: &HostEntry, why: MasterExitWhy) -> MasterExitOutcome {
        let host = Ident::new(entry.id.as_str());
        let record = |outcome, exit, stderr: Option<&str>| {
            emit(MarsadEvent::SshMasterExit {
                host: host.clone(),
                why,
                outcome,
                exit,
                stderr: stderr.filter(|s| !s.is_empty()).map(diag::transport),
            })
        };
        let control = match self.path(entry) {
            Ok(p) => p,
            Err(words) => {
                record(MasterExitOutcome::Failed, None, Some(&words));
                return MasterExitOutcome::Failed;
            }
        };
        let outcome = match tokio::fs::symlink_metadata(&control).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                record(MasterExitOutcome::NoMaster, None, None);
                MasterExitOutcome::NoMaster
            }
            _ => match master_command_argv(&entry.kind, &control, MasterCommand::Exit) {
                None => MasterExitOutcome::NoMaster,
                Some(argv) => match run_local(argv).await {
                    Ok(Ran { exit: Some(0), .. }) => {
                        record(MasterExitOutcome::Ended, Some(0), None);
                        MasterExitOutcome::Ended
                    }
                    Ok(Ran { stderr, .. }) if stderr.contains("Connection refused") => {
                        // A socket nobody listens on: there was no connection.
                        if let Err(e) = tokio::fs::remove_file(&control).await {
                            if e.kind() != std::io::ErrorKind::NotFound {
                                fs_failed(&entry.id, &e);
                            }
                        }
                        record(MasterExitOutcome::NoMaster, None, None);
                        MasterExitOutcome::NoMaster
                    }
                    Ok(Ran { exit, stderr }) => {
                        record(MasterExitOutcome::Failed, exit, Some(&stderr));
                        MasterExitOutcome::Failed
                    }
                    Err(words) => {
                        record(MasterExitOutcome::Failed, None, Some(&words));
                        MasterExitOutcome::Failed
                    }
                },
            },
        };
        if outcome != MasterExitOutcome::Failed {
            self.with(&entry.id, |h| {
                h.alive = false;
                h.ended = None;
            });
            remove_file(&entry.id, &master_log(&control)).await;
        }
        outcome
    }
}

/// Removes `path`; a file that is not there is fine, anything else is recorded.
async fn remove_file(host: &HostId, path: &Path) {
    if let Err(e) = tokio::fs::remove_file(path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            fs_failed(host, &e);
        }
    }
}

/// The last `max` bytes of the file at `path` as text; `None` when it cannot be read or is
/// empty.
async fn read_tail(path: &Path, max: u64) -> Option<String> {
    let mut f = tokio::fs::File::open(path).await.ok()?;
    let len = f.metadata().await.ok()?.len();
    f.seek(std::io::SeekFrom::Start(len.saturating_sub(max)))
        .await
        .ok()?;
    let mut buf = Vec::new();
    f.take(max).read_to_end(&mut buf).await.ok()?;
    let text = String::from_utf8_lossy(&buf).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The last [`LOG_TAIL_LINES`] lines of `text`, joined with ` / `, at most [`LOG_TAIL_CHARS`]
/// characters (the end of them).
fn last_lines(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let from = lines.len().saturating_sub(LOG_TAIL_LINES);
    let joined = lines[from..].join(" / ");
    let n = joined.chars().count();
    if n > LOG_TAIL_CHARS {
        let tail: String = joined.chars().skip(n - LOG_TAIL_CHARS).collect();
        format!("…{tail}")
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reason_is_the_end_of_the_log_and_bounded() {
        assert_eq!(last_lines(""), "");
        assert_eq!(
            last_lines("a\n\n b \r\nc\nd\n"),
            "b / c / d",
            "the last three lines, trimmed, no blanks"
        );
        let long = format!("{}\nend", "x".repeat(1000));
        let t = last_lines(&long);
        assert!(t.starts_with('…') && t.ends_with("end"), "{t}");
        assert_eq!(t.chars().count(), LOG_TAIL_CHARS + 1);
    }
}
