//! `host.exec.*`: one-off shell commands with their output streamed.
//!
//! # Lifecycle
//!
//! `start` checks the working directory, spawns the command (POSIX `sh -c`, or PowerShell on
//! Windows) in its own process group, and returns its id at once. A task per command owns the
//! child and ends it in one of three ways, each recorded as exactly one `exited` event:
//!
//! 1. the command exits (its pipes are then drained, so every byte it wrote is sent first);
//! 2. the time limit passes (one deadline per command, set when it starts): the group is killed
//!    and the event carries [`ExecEndReason::TimeLimit`];
//! 3. `cancel` (or daemon shutdown) is requested: the group is killed and the event carries
//!    [`ExecEndReason::Cancelled`].
//!
//! A command that leaves a background job holding its output pipes keeps the execution running
//! (its `exited` waits for the pipes to close, so no output is lost) until the job exits or the
//! time limit ends the group. Pipes still open a bounded time after the group was killed (a
//! process that left the group) are abandoned, and the output then is what was read.
//!
//! # Output
//!
//! Output is retained, as events, up to [`MAX_RETAINED`] bytes across both streams. Past that,
//! the command keeps running and its output is drained and dropped; `exited` says so. Chunks
//! are at most [`CHUNK`] bytes of text; a UTF-8 sequence split by a read is held until its
//! remaining bytes arrive.
//!
//! Finished executions stay readable (their retained output and `exited`) until
//! [`MAX_FINISHED`] newer ones have finished.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::stream::StreamExt;
use marsad_proto::exec::{ExecEndReason, ExecEvent, ExecStarted, ExecStream};
use marsad_proto::MarsadError;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;
use wasit_control::{CloseReason, Cursor, EventStream};

/// How long a command may run before the host ends its process group.
pub const TIME_LIMIT: Duration = Duration::from_secs(60 * 60);
/// Output kept per execution, across both streams.
pub const MAX_RETAINED: usize = 1024 * 1024;
/// The most text one `output` event carries.
pub const CHUNK: usize = 8 * 1024;
/// Finished executions kept readable.
pub const MAX_FINISHED: usize = 32;
const LIVE_QUEUE: usize = 512;
/// How long a killed group's pipes may take to close before they are abandoned.
const DRAIN_WAIT: Duration = Duration::from_secs(10);
/// How long `cancel` (and shutdown) waits for a command to be recorded as ended.
const END_WAIT: Duration = Duration::from_secs(10);

/// An event with its sequence number (its cursor).
type Live = (u64, ExecEvent);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Only plain data behind these locks; a poisoned one is still consistent.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a command's output log holds, under one lock.
struct Log {
    /// Retained events, in order: output chunks, then the `exited` event.
    events: Vec<Live>,
    /// Sequence number of the next event (the first is 1).
    next_seq: u64,
    /// Bytes of output retained so far, across both streams.
    retained: usize,
    /// Bytes of output produced so far, across both streams, retained or not.
    produced: u64,
    truncated: bool,
    exited: bool,
}

struct Exec {
    id: String,
    log: Mutex<Log>,
    live: broadcast::Sender<Live>,
    /// Requests the command's end (cancel, shutdown).
    cancel: CancellationToken,
    /// `true` once the `exited` event has been recorded.
    ended: watch::Sender<bool>,
}

impl Exec {
    /// Retain as much of an `n`-byte read as the cap allows; returns how many bytes to keep.
    fn account(&self, n: usize) -> usize {
        let mut log = lock(&self.log);
        log.produced += n as u64;
        let keep = n.min(MAX_RETAINED - log.retained);
        log.retained += keep;
        if keep < n {
            log.truncated = true;
        }
        keep
    }

    /// Record `text` from `stream`, in chunks of at most [`CHUNK`] bytes.
    fn output(&self, stream: ExecStream, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut log = lock(&self.log);
        for piece in chunks(text) {
            self.push(
                &mut log,
                ExecEvent::Output {
                    stream,
                    text: piece.to_string(),
                },
            );
        }
    }

    fn push(&self, log: &mut Log, ev: ExecEvent) {
        let seq = log.next_seq;
        log.next_seq += 1;
        log.events.push((seq, ev.clone()));
        // No subscribers is fine.
        let _ = self.live.send((seq, ev));
    }

    /// Record the `exited` event; nothing follows it.
    fn finish(&self, code: Option<i32>, signal: Option<i32>, reason: Option<ExecEndReason>) {
        let mut log = lock(&self.log);
        let ev = ExecEvent::Exited {
            code,
            signal,
            output_truncated: log.truncated,
            output_bytes: log.produced,
            reason,
        };
        self.push(&mut log, ev);
        log.exited = true;
        drop(log);
        self.ended.send_replace(true);
    }
}

/// Pieces of `text` of at most [`CHUNK`] bytes, each ending on a character boundary.
fn chunks(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut end = rest.len().min(CHUNK);
        // A char is at most 4 bytes, so this stops within 3 steps.
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (piece, tail) = rest.split_at(end);
        rest = tail;
        Some(piece)
    })
}

/// Decode the complete UTF-8 at the front of `pending`, removing it. Invalid bytes become
/// U+FFFD; an incomplete sequence at the very end is kept for the next read.
fn decode_prefix(pending: &mut Vec<u8>) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(s) => {
                out.push_str(s);
                pending.clear();
                return out;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(&String::from_utf8_lossy(&pending[..valid]));
                match e.error_len() {
                    Some(bad) => {
                        out.push('\u{FFFD}');
                        pending.drain(..valid + bad);
                    }
                    None => {
                        pending.drain(..valid);
                        return out;
                    }
                }
            }
        }
    }
}

/// Read one pipe to its end, recording its output in order.
async fn pump<R: AsyncRead + Unpin>(exec: Arc<Exec>, stream: ExecStream, mut reader: R) {
    let mut buf = vec![0u8; CHUNK];
    let mut pending = Vec::new();
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        // The bytes past the cap are read (so the command never blocks on a full pipe) and
        // dropped.
        let keep = exec.account(n);
        pending.extend_from_slice(&buf[..keep]);
        exec.output(stream, &decode_prefix(&mut pending));
    }
    exec.output(stream, &String::from_utf8_lossy(&pending));
}

/// Resolves when the command must be ended by the host: its time limit has passed (the
/// deadline is set when this future is first polled, and this future is polled once per
/// command), or its end was requested.
async fn must_end(limit: Duration, cancel: CancellationToken) -> ExecEndReason {
    tokio::select! {
        _ = tokio::time::sleep(limit) => ExecEndReason::TimeLimit,
        _ = cancel.cancelled() => ExecEndReason::Cancelled,
    }
}

/// Kill the process group led by `pid` (the command's shell), with everything in it.
fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(unix)]
    wasit_process::kill_process_group(pid);
    #[cfg(windows)]
    {
        // `/T` ends the whole process tree; the command was not given a process group.
        let _ = wasit_process::std_command("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn exit_signal(status: &std::process::ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

/// Owns one command: its output, its end, and the group it runs in.
async fn run(inner: Arc<Inner>, exec: Arc<Exec>, mut child: Child, limit: Duration) {
    let pid = child.id();
    let mut readers: Vec<JoinHandle<()>> = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(tokio::spawn(pump(exec.clone(), ExecStream::Stdout, out)));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(tokio::spawn(pump(exec.clone(), ExecStream::Stderr, err)));
    }
    let aborts: Vec<AbortHandle> = readers.iter().map(|h| h.abort_handle()).collect();
    let mut drain = pin!(futures::future::join_all(readers));
    let mut stop = pin!(must_end(limit, exec.cancel.clone()));
    let mut reason: Option<ExecEndReason> = None;

    // Phase 1: the shell exits on its own, or the host ends the group.
    let waited = tokio::select! {
        status = child.wait() => Some(status),
        r = &mut stop => {
            reason = Some(r);
            None
        }
    };
    let status = match waited {
        Some(status) => status.ok(),
        None => {
            // Killed before it is reaped, so the group id cannot have been reused.
            kill_group(pid);
            child.wait().await.ok()
        }
    };

    // Phase 2: the output is drained. A background job may still hold the pipes after the shell
    // has exited; then the same limit applies to it.
    let mut drained = false;
    if reason.is_none() {
        tokio::select! {
            _ = &mut drain => drained = true,
            r = &mut stop => reason = Some(r),
        }
        if reason.is_some() && !drained {
            kill_group(pid);
        }
    }
    if !drained {
        drained = tokio::time::timeout(DRAIN_WAIT, &mut drain).await.is_ok();
    }
    if !drained {
        for a in &aborts {
            a.abort();
        }
    }

    let (code, signal) = match &status {
        Some(s) => (s.code(), exit_signal(s)),
        None => (None, None),
    };
    exec.finish(code, signal, reason);
    lock(&inner.table).retire(&exec.id);
}

#[derive(Default)]
struct Table {
    by_id: HashMap<String, Arc<Exec>>,
    /// Finished executions, oldest first.
    finished: VecDeque<String>,
    /// The daemon is shutting down: nothing starts any more.
    shut: bool,
}

impl Table {
    /// Note that `id` has ended, and forget the oldest finished executions beyond the cap.
    fn retire(&mut self, id: &str) {
        self.finished.push_back(id.to_string());
        while self.finished.len() > MAX_FINISHED {
            if let Some(old) = self.finished.pop_front() {
                self.by_id.remove(&old);
            }
        }
    }
}

struct Inner {
    table: Mutex<Table>,
    limit: Duration,
}

/// All `host.exec` executions of this daemon. Cheap to clone.
#[derive(Clone)]
pub struct Execs {
    inner: Arc<Inner>,
}

impl Default for Execs {
    fn default() -> Self {
        Execs::with_time_limit(TIME_LIMIT)
    }
}

fn new_id() -> String {
    const A: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    (0..10).map(|_| A[fastrand::usize(..32)] as char).collect()
}

/// The shell command line `line` runs in: `sh -c` on Unix, PowerShell on Windows.
fn shell_command(line: &str) -> Command {
    #[cfg(unix)]
    {
        let mut cmd = wasit_process::command("/bin/sh");
        cmd.arg("-c").arg(line);
        // Its own group: `kill_group` ends the command with everything it started.
        cmd.process_group(0);
        cmd
    }
    #[cfg(windows)]
    {
        let mut cmd = wasit_process::command("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", line]);
        cmd
    }
}

impl Execs {
    /// Executions that end after `limit` (the daemon uses [`TIME_LIMIT`]; tests use less).
    pub fn with_time_limit(limit: Duration) -> Self {
        Execs {
            inner: Arc::new(Inner {
                table: Mutex::new(Table::default()),
                limit,
            }),
        }
    }

    fn get(&self, id: &str) -> Result<Arc<Exec>, MarsadError> {
        lock(&self.inner.table)
            .by_id
            .get(id)
            .cloned()
            .ok_or_else(|| MarsadError::NotFound {
                what: format!("execution {id}"),
            })
    }

    /// Start `line` in `cwd` (which must be an existing directory) and return its id.
    pub async fn start(&self, cwd: &str, line: &str) -> Result<ExecStarted, MarsadError> {
        if line.trim().is_empty() {
            return Err(MarsadError::invalid("the command line is empty"));
        }
        let dir = crate::util::expand_tilde(cwd)?;
        check_dir(&dir).await?;
        let mut cmd = shell_command(line);
        cmd.current_dir(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Daemon shutdown drops the task that owns the child; the shell then goes too.
            .kill_on_drop(true);
        if lock(&self.inner.table).shut {
            return Err(MarsadError::io("the daemon is shutting down"));
        }
        let child = cmd
            .spawn()
            .map_err(|e| MarsadError::io(format!("start {line:?}: {e}")))?;
        let id = new_id();
        let exec = Arc::new(Exec {
            id: id.clone(),
            log: Mutex::new(Log {
                events: Vec::new(),
                next_seq: 1,
                retained: 0,
                produced: 0,
                truncated: false,
                exited: false,
            }),
            live: broadcast::channel(LIVE_QUEUE).0,
            cancel: CancellationToken::new(),
            ended: watch::channel(false).0,
        });
        {
            let mut t = lock(&self.inner.table);
            if t.shut {
                drop(t);
                // Not even registered: the group is ended here, and the shell by `kill_on_drop`.
                kill_group(child.id());
                return Err(MarsadError::io("the daemon is shutting down"));
            }
            t.by_id.insert(id.clone(), exec.clone());
        }
        let inner = self.inner.clone();
        let limit = self.inner.limit;
        tokio::spawn(run(inner, exec, child, limit));
        Ok(ExecStarted { exec_id: id })
    }

    /// End the command and return once it is recorded as ended. Ending one that has already
    /// ended is fine.
    pub async fn cancel(&self, id: &str) -> Result<(), MarsadError> {
        let exec = self.get(id)?;
        exec.cancel.cancel();
        wait_ended(&exec, id).await
    }

    /// The command's events from `from` on (a cursor: the sequence number last seen), then live
    /// ones, ending after `exited`. A subscriber that falls behind gets `Lagged` and resubscribes
    /// from its cursor.
    pub fn subscribe(&self, id: &str, from: Option<u64>) -> Result<EventStream, MarsadError> {
        let exec = self.get(id)?;
        let from = from.unwrap_or(0);
        let (initial, rx, done) = {
            let log = lock(&exec.log);
            let last = log.next_seq - 1;
            if from > last {
                return Err(MarsadError::invalid(format!(
                    "cursor {from} is beyond the output of execution {id}"
                )));
            }
            let initial: Vec<Live> = log
                .events
                .iter()
                .filter(|(seq, _)| *seq > from)
                .cloned()
                .collect();
            // Subscribed under the same lock as the snapshot: nothing is missed or repeated.
            (initial, exec.live.subscribe(), log.exited)
        };
        let head = futures::stream::iter(initial.into_iter().map(Ok));
        let live = futures::stream::unfold((rx, done), |(mut rx, done)| async move {
            if done {
                return None;
            }
            match rx.recv().await {
                Ok((seq, ev)) => {
                    let fin = matches!(ev, ExecEvent::Exited { .. });
                    Some((Ok((seq, ev)), (rx, fin)))
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    Some((Err(CloseReason::Lagged), (rx, true)))
                }
                Err(broadcast::error::RecvError::Closed) => None,
            }
        });
        Ok(head
            .chain(live)
            .map(|item| match item {
                Ok((seq, ev)) => serde_json::to_value(&ev)
                    .map(|v| (Cursor::from_u64(seq), v))
                    .map_err(|e| CloseReason::Error {
                        error: wasit_control::RpcError::internal(e.to_string()),
                    }),
                Err(e) => Err(e),
            })
            .boxed())
    }

    /// End every running command and refuse new ones; returns once they are recorded as ended
    /// (each bounded).
    pub async fn shutdown(&self) {
        let running: Vec<(String, Arc<Exec>)> = {
            let mut t = lock(&self.inner.table);
            t.shut = true;
            t.by_id
                .iter()
                .map(|(id, e)| (id.clone(), e.clone()))
                .collect()
        };
        futures::future::join_all(running.iter().map(|(id, e)| {
            e.cancel.cancel();
            wait_ended(e, id)
        }))
        .await;
    }
}

/// Wait until `exec` is recorded as ended, bounded by [`END_WAIT`].
async fn wait_ended(exec: &Exec, id: &str) -> Result<(), MarsadError> {
    let mut ended = exec.ended.subscribe();
    let waited = tokio::time::timeout(END_WAIT, ended.wait_for(|e| *e)).await;
    match waited {
        Ok(_) => Ok(()),
        Err(_) => Err(MarsadError::Timeout {
            what: format!("execution {id} did not end"),
        }),
    }
}

async fn check_dir(dir: &Path) -> Result<(), MarsadError> {
    if tokio::fs::metadata(dir)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(MarsadError::NotFound {
            what: format!("directory {}", dir.display()),
        })
    }
}
