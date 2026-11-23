//! `pty.*`: interactive shells (portable-pty) with a byte-offset output topic and bounded
//! scrollback replay.
//!
//! # Who owns a terminal, and when it ends
//!
//! A terminal belongs to the daemon of its host, and a session has at most ONE: it is identified
//! by the session. `start` for a session that already has a live terminal returns that terminal
//! (`created: false`); clients attach to it with `pty.output:<id>` and detach by unsubscribing,
//! which touches nothing here. The shell is the user's, not the agent's: it keeps running while
//! the session is ended (agent process gone, session kept). A terminal ends when
//!
//! 1. its shell exits (the user typed `exit`, it crashed): the entry then stays, readable (the
//!    scrollback, the exit status, and an `exited` event at the end of its stream, replayed to
//!    any later subscriber), until it is closed or replaced by a new terminal for that session
//!    (`start` on an exited slot). So a session holds at most one entry, live or exited;
//! 2. a client closes it (`close`): the entry leaves the table, the shell is hung up as closing
//!    a terminal window does (and killed, with what runs in the foreground, if it ignores that),
//!    and the call returns once the shell has been reaped. What a shell ran in the background
//!    and set to ignore hang-ups is not tracked, as with any terminal;
//! 3. its session is deleted: [`Ptys::follow_sessions`] reads the runtime's session feed (the
//!    `Removed` event, and a reconciliation against the manager after a `Reset`) and closes it;
//! 4. the daemon shuts down ([`Ptys::shutdown`]): every shell is ended and reaped before the
//!    daemon exits.
//!
//! Terminals started for a directory instead of a session have no owner but their creator:
//! there is no slot, each `start` makes a new one (live or exited, until closed), and the host
//! refuses the start beyond [`MAX_DIRECTORY_TERMINALS`] with `limit_reached`. Only tests and
//! other clients use them; the desktop app opens session terminals.
//!
//! # Mechanics
//!
//! Each terminal has one reader thread (blocking reads of the pty master), one writer thread
//! (fed by a bounded channel) and one thread waiting for the shell, so no async task ever
//! blocks on the pty. The scrollback and the live broadcast are updated under one short
//! memory-only lock, which is also taken to start a subscription, so replay and live output join
//! with no gap and no duplicate. The shell exiting is reported last: after it is reaped and its
//! output drained, under that same lock, as the final event of every stream.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use futures::stream::StreamExt;
use marsad_proto::pty::{PtyEvent, PtyInfo, PtyStarted};
use marsad_proto::MarsadError;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{broadcast, mpsc, watch};
use wasit_control::{CloseReason, Cursor, EventStream};
use wasit_runtime::wire::HostEvent;
use wasit_runtime::{SessionManager, StreamEnd};

use crate::hub_events;

/// Scrollback kept per terminal.
pub const SCROLLBACK: usize = 256 * 1024;
/// Terminals not tied to a session (live or exited, until closed) a host keeps at most.
pub const MAX_DIRECTORY_TERMINALS: usize = 8;
const INPUT_QUEUE: usize = 64;
const LIVE_QUEUE: usize = 512;
const INPUT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a shell has to exit after it is hung up (what closing a terminal window does)
/// before it and what runs in the foreground are killed. A shell takes milliseconds; this is for
/// one that ignores the hang-up.
const HANG_UP_GRACE: Duration = Duration::from_secs(2);
/// How long a killed shell may take to be reaped. The kill is unconditional; this only bounds a
/// shell the kernel cannot end.
const KILL_WAIT: Duration = Duration::from_secs(10);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Only plain data behind these locks; a poisoned one is still consistent.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Scroll {
    /// Offset of `data[0]`.
    start: u64,
    data: VecDeque<u8>,
    /// Set once, when the shell has been reaped and its output drained: its status, if known.
    exited: Option<Option<i32>>,
}

impl Scroll {
    fn end(&self) -> u64 {
        self.start + self.data.len() as u64
    }
}

type Live = (u64, PtyEvent);

struct Pty {
    id: String,
    cwd: String,
    session: Option<String>,
    size: Mutex<(u16, u16)>,
    scroll: Mutex<Scroll>,
    live: broadcast::Sender<Live>,
    /// Taken (so the writer thread ends) once the shell has exited.
    input: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    /// Taken (closed) once the shell has exited.
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// The shell's process id, which is also its process group (it leads its session).
    #[cfg(unix)]
    pid: Option<u32>,
    /// `true` once the shell has been reaped and its exit recorded.
    gone: watch::Sender<bool>,
}

impl Pty {
    fn info(&self) -> PtyInfo {
        let (cols, rows) = *lock(&self.size);
        let s = lock(&self.scroll);
        PtyInfo {
            id: self.id.clone(),
            cwd: self.cwd.clone(),
            session: self.session.clone(),
            cols,
            rows,
            offset: s.end(),
            exited: s.exited.is_some(),
            exit_code: s.exited.flatten(),
        }
    }

    fn is_live(&self) -> bool {
        lock(&self.scroll).exited.is_none()
    }

    /// Hang the shell up, as closing its terminal window does: it exits, hangs up its jobs, and
    /// the kernel hangs up what runs in the foreground. Returns at once.
    fn hang_up(&self) {
        // The shell may exit on its own at the same moment: then there is nothing to hang up.
        if !self.is_live() {
            return;
        }
        // SIGHUP on Unix; the process is terminated on Windows.
        if let Err(e) = lock(&self.killer).kill() {
            hub_events::pty_kill_failed(&self.id, &e);
        }
    }

    /// Kill what ignored the hang-up: the process group in the terminal's foreground (what the
    /// user is running) and the shell's own group. Returns at once.
    #[cfg(unix)]
    fn kill_hard(&self) {
        if !self.is_live() {
            return;
        }
        let foreground = lock(&self.master)
            .as_ref()
            .and_then(|m| m.process_group_leader())
            .and_then(|g| u32::try_from(g).ok());
        for group in foreground.into_iter().chain(self.pid) {
            wasit_process::kill_process_group(group);
        }
    }

    /// End the shell and wait for it: hang it up and, if it has not exited within
    /// [`HANG_UP_GRACE`], kill it. `true` once it has been reaped, `false` if it still has not
    /// been [`KILL_WAIT`] after the kill.
    async fn end(&self) -> bool {
        self.hang_up();
        if tokio::time::timeout(HANG_UP_GRACE, self.gone())
            .await
            .is_ok()
        {
            return true;
        }
        #[cfg(unix)]
        self.kill_hard();
        tokio::time::timeout(KILL_WAIT, self.gone()).await.is_ok()
    }

    /// Resolves once the shell has been reaped (at once if it already was).
    async fn gone(&self) {
        let mut rx = self.gone.subscribe();
        // The sender lives as long as `self`.
        let _ = rx.wait_for(|gone| *gone).await;
    }
}

#[derive(Default)]
struct Table {
    by_id: HashMap<String, Arc<Pty>>,
    /// The daemon is shutting down: nothing starts any more.
    shut: bool,
}

impl Table {
    fn of_session(&self, session: &str) -> Option<Arc<Pty>> {
        self.by_id
            .values()
            .find(|p| p.session.as_deref() == Some(session))
            .cloned()
    }

    fn directory_terminals(&self) -> usize {
        self.by_id.values().filter(|p| p.session.is_none()).count()
    }
}

#[derive(Default)]
struct Inner {
    table: Mutex<Table>,
    /// Held across a whole `start`: the slot and cap decisions and the insertion are one step,
    /// so two opens of a session's terminal never start two shells.
    starting: tokio::sync::Mutex<()>,
}

/// All terminals of this daemon. Cheap to clone.
#[derive(Clone, Default)]
pub struct Ptys {
    inner: Arc<Inner>,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/bin/sh".into())
    }
}

fn new_id() -> String {
    const A: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    (0..10).map(|_| A[fastrand::usize(..32)] as char).collect()
}

fn not_found(id: &str) -> MarsadError {
    MarsadError::NotFound {
        what: format!("terminal {id}"),
    }
}

impl Ptys {
    fn get(&self, id: &str) -> Result<Arc<Pty>, MarsadError> {
        lock(&self.inner.table)
            .by_id
            .get(id)
            .cloned()
            .ok_or_else(|| not_found(id))
    }

    pub fn list(&self) -> Vec<PtyInfo> {
        let mut v: Vec<PtyInfo> = lock(&self.inner.table)
            .by_id
            .values()
            .map(|p| p.info())
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// The terminal of `session`, or a new shell in `cwd` when it has none live (an exited one
    /// is replaced). Without a session, always a new shell, within the cap.
    pub async fn start(
        &self,
        cwd: &Path,
        session: Option<String>,
        cols: u16,
        rows: u16,
        shell: Option<String>,
    ) -> Result<PtyStarted, MarsadError> {
        if cols == 0 || rows == 0 {
            return Err(MarsadError::invalid("cols and rows must be positive"));
        }
        let _one_start = self.inner.starting.lock().await;
        {
            let t = lock(&self.inner.table);
            if t.shut {
                return Err(MarsadError::io("the daemon is shutting down"));
            }
            match &session {
                Some(s) => {
                    if let Some(p) = t.of_session(s).filter(|p| p.is_live()) {
                        return Ok(PtyStarted {
                            pty: p.info(),
                            created: false,
                        });
                    }
                }
                None if t.directory_terminals() >= MAX_DIRECTORY_TERMINALS => {
                    return Err(MarsadError::LimitReached {
                        message: format!(
                            "this host keeps at most {MAX_DIRECTORY_TERMINALS} terminals that \
                             are not tied to a session; close one first"
                        ),
                    });
                }
                None => {}
            }
        }
        if !tokio::fs::metadata(cwd)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            return Err(MarsadError::NotFound {
                what: format!("directory {}", cwd.display()),
            });
        }
        let pty = self.spawn(cwd, session, cols, rows, shell).await?;
        let info = pty.info();
        let mut t = lock(&self.inner.table);
        if t.shut {
            drop(t);
            // Hung up like any other; reaped by its wait thread.
            pty.hang_up();
            return Err(MarsadError::io("the daemon is shutting down"));
        }
        // The slot's previous terminal, which is exited (a live one returned above), is
        // replaced; nothing else of this session can be in the table (one start at a time).
        if let Some(old) = pty.session.as_deref().and_then(|s| t.of_session(s)) {
            t.by_id.remove(&old.id);
        }
        t.by_id.insert(pty.id.clone(), pty);
        Ok(PtyStarted {
            pty: info,
            created: true,
        })
    }

    /// Open a pty, start the shell on it and the threads serving it.
    async fn spawn(
        &self,
        cwd: &Path,
        session: Option<String>,
        cols: u16,
        rows: u16,
        shell: Option<String>,
    ) -> Result<Arc<Pty>, MarsadError> {
        let shell = shell
            .filter(|s| !s.is_empty())
            .unwrap_or_else(default_shell);
        let cwd_s = cwd.display().to_string();
        let cwd2 = cwd.to_path_buf();
        // Opening a pty and spawning are blocking syscalls.
        let spawned = tokio::task::spawn_blocking(move || {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| MarsadError::io(format!("openpty: {e}")))?;
            let mut cmd = CommandBuilder::new(&shell);
            // The host's base environment (the user's login environment), like every other
            // process the daemon starts.
            if let Some(base) = wasit_process::base_env() {
                cmd.env_clear();
                for (k, v) in base.iter() {
                    cmd.env(k, v);
                }
            }
            cmd.cwd(&cwd2);
            cmd.env("TERM", "xterm-256color");
            let child = pair
                .slave
                .spawn_command(cmd)
                .map_err(|e| MarsadError::io(format!("spawn {shell}: {e}")))?;
            // The child holds the only slave handle: the reader sees EOF when it exits.
            drop(pair.slave);
            let reader = pair
                .master
                .try_clone_reader()
                .map_err(|e| MarsadError::io(format!("pty reader: {e}")));
            let writer = pair
                .master
                .take_writer()
                .map_err(|e| MarsadError::io(format!("pty writer: {e}")));
            match (reader, writer) {
                (Ok(reader), Ok(writer)) => {
                    Ok::<_, MarsadError>((pair.master, child, reader, writer))
                }
                (r, w) => {
                    // A shell was started: it must not outlive the failed start.
                    let mut child = child;
                    if let Err(e) = child.kill() {
                        hub_events::pty_kill_failed("starting", &e);
                    }
                    // Reaped here: nothing else will ever wait for it.
                    if let Err(e) = child.wait() {
                        hub_events::pty_kill_failed("starting", &e);
                    }
                    Err(r
                        .err()
                        .or(w.err())
                        .unwrap_or_else(|| MarsadError::io("pty")))
                }
            }
        })
        .await
        .map_err(|e| MarsadError::io(format!("pty task: {e}")))??;
        let (master, mut child, reader, writer) = spawned;
        let id = new_id();
        let (input, input_rx) = mpsc::channel(INPUT_QUEUE);
        let pty = Arc::new(Pty {
            id: id.clone(),
            cwd: cwd_s,
            session,
            size: Mutex::new((cols, rows)),
            scroll: Mutex::new(Scroll {
                start: 0,
                data: VecDeque::new(),
                exited: None,
            }),
            live: broadcast::channel(LIVE_QUEUE).0,
            input: Mutex::new(Some(input)),
            master: Mutex::new(Some(master)),
            killer: Mutex::new(child.clone_killer()),
            #[cfg(unix)]
            pid: child.process_id(),
            gone: watch::channel(false).0,
        });
        // A failure from here on ends the shell, which is reaped by whichever thread owns the
        // child, or by `fail` when none does yet.
        let fail = |mut child: Box<dyn portable_pty::Child + Send + Sync>, e: MarsadError| {
            if let Err(k) = child.kill() {
                hub_events::pty_kill_failed(&id, &k);
            }
            if let Err(w) = child.wait() {
                hub_events::pty_kill_failed(&id, &w);
            }
            e
        };
        if let Err(e) = spawn_writer(writer, input_rx) {
            hub_events::pty_writer_failed(&e);
            return Err(fail(
                child,
                MarsadError::io(format!("pty writer thread: {e}")),
            ));
        }
        let p = pty.clone();
        let read = match std::thread::Builder::new()
            .name(format!("pty-read-{id}"))
            .spawn(move || pump(&p, reader))
        {
            Ok(h) => h,
            Err(e) => {
                return Err(fail(
                    child,
                    MarsadError::io(format!("pty reader thread: {e}")),
                ))
            }
        };
        // The exit is the shell's, not the reader's EOF: a ConPTY keeps its output pipe open
        // until the pseudo console is closed. So wait for the shell, close the master (which
        // ends the reader once it has drained everything), then report the exit last.
        let p = pty.clone();
        let waiting = std::thread::Builder::new()
            .name(format!("pty-wait-{id}"))
            .spawn(move || {
                // A failed wait leaves the status unknown: the exit is then reported without
                // a code, which the wire says (`exit_code` is optional).
                let code = child.wait().ok().map(|s| s.exit_code() as i32);
                drop(lock(&p.master).take());
                // The reader ended at EOF; a panic in it is already reported by the runtime.
                let _ = read.join();
                drop(lock(&p.input).take());
                let mut s = lock(&p.scroll);
                s.exited = Some(code);
                let end = s.end();
                // No subscribers is fine.
                let _ = p.live.send((end + 1, PtyEvent::Exited { code }));
                drop(s);
                p.gone.send_replace(true);
            });
        if let Err(e) = waiting {
            // The shell is hung up; nothing can reap it (the thread that would own the child
            // never started), so it stays a zombie until the daemon exits.
            pty.hang_up();
            return Err(MarsadError::io(format!("pty wait thread: {e}")));
        }
        Ok(pty)
    }

    pub async fn input(&self, id: &str, data: String) -> Result<(), MarsadError> {
        let p = self.get(id)?;
        let tx = lock(&p.input)
            .clone()
            .ok_or_else(|| MarsadError::invalid(format!("terminal {id} has exited")))?;
        match tokio::time::timeout(INPUT_TIMEOUT, tx.send(data.into_bytes())).await {
            Err(_) => Err(MarsadError::Timeout {
                what: format!("terminal {id} is not reading input"),
            }),
            Ok(Err(_)) => Err(MarsadError::invalid(format!("terminal {id} has exited"))),
            Ok(Ok(())) => Ok(()),
        }
    }

    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), MarsadError> {
        if cols == 0 || rows == 0 {
            return Err(MarsadError::invalid("cols and rows must be positive"));
        }
        let p = self.get(id)?;
        let p2 = p.clone();
        tokio::task::spawn_blocking(move || {
            lock(&p2.master)
                .as_ref()
                .ok_or_else(|| MarsadError::invalid("the terminal has exited"))?
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| MarsadError::io(format!("resize: {e}")))
        })
        .await
        .map_err(|e| MarsadError::io(format!("resize task: {e}")))??;
        *lock(&p.size) = (cols, rows);
        Ok(())
    }

    /// End the terminal: forget it, end the shell (see [`Pty::end`]) and return once it has been
    /// reaped, so the caller knows it is gone. Subscribers see `exited`.
    pub async fn close(&self, id: &str) -> Result<(), MarsadError> {
        let p = lock(&self.inner.table)
            .by_id
            .remove(id)
            .ok_or_else(|| not_found(id))?;
        if p.end().await {
            Ok(())
        } else {
            Err(MarsadError::Timeout {
                what: format!("terminal {id} did not end"),
            })
        }
    }

    /// Forget the terminal of `session`, if it has one, and end its shell in the background.
    /// Needs the runtime.
    pub fn close_session(&self, session: &str) {
        let p = {
            let mut t = lock(&self.inner.table);
            let Some(p) = t.of_session(session) else {
                return;
            };
            t.by_id.remove(&p.id);
            p
        };
        tokio::spawn(async move {
            if !p.end().await {
                hub_events::pty_not_ended(&p.id);
            }
        });
    }

    /// Close the terminals of the sessions `alive` says no longer exist.
    fn close_sessions_not(&self, alive: impl Fn(&str) -> bool) {
        let gone: Vec<String> = lock(&self.inner.table)
            .by_id
            .values()
            .filter_map(|p| p.session.clone())
            .filter(|s| !alive(s))
            .collect();
        for s in gone {
            self.close_session(&s);
        }
    }

    /// End every shell and refuse new ones; returns once they are all reaped (each bounded).
    pub async fn shutdown(&self) {
        let all: Vec<Arc<Pty>> = {
            let mut t = lock(&self.inner.table);
            t.shut = true;
            t.by_id.drain().map(|(_, p)| p).collect()
        };
        futures::future::join_all(all.iter().map(|p| async {
            if !p.end().await {
                hub_events::pty_not_ended(&p.id);
            }
        }))
        .await;
    }

    /// Close a session's terminal when the session is deleted, for as long as the daemon runs.
    ///
    /// The signal is the runtime's session feed: its `Removed` event, and after a `Reset` (the
    /// first event, or one after the feed lost its place) each terminal's session is checked
    /// against the manager itself, which removes a session before the feed hears of it.
    /// A subscriber that fell behind resubscribes from its last cursor at once.
    pub fn follow_sessions(&self, manager: SessionManager) -> tokio::task::JoinHandle<()> {
        let ptys = self.clone();
        tokio::spawn(async move {
            let alive = |id: &str| {
                marsad_proto::SessionId::parse(id).is_some_and(|id| manager.snapshot(&id).is_ok())
            };
            let mut last: Option<String> = None;
            loop {
                let mut feed = manager.subscribe_host(last.as_deref());
                let mut lagged = false;
                while let Some(item) = feed.next().await {
                    match item {
                        Ok((cursor, ev)) => {
                            last = Some(cursor);
                            match ev {
                                HostEvent::Removed { id } => ptys.close_session(&id.0),
                                HostEvent::Reset { .. } => ptys.close_sessions_not(alive),
                                HostEvent::Upsert { .. } => {}
                            }
                        }
                        Err(StreamEnd::Lagged) => {
                            lagged = true;
                            break;
                        }
                        Err(StreamEnd::Error(e)) => {
                            // The feed has only one failure, falling behind: anything else is
                            // not something resubscribing fixes.
                            hub_events::pty_session_feed_failed(&e);
                            return;
                        }
                    }
                }
                if !lagged {
                    // The feed ended: the manager is gone, the daemon is stopping.
                    return;
                }
            }
        })
    }

    /// The output topic: replay after `from` (from the scrollback), then live, then `exited`.
    pub fn subscribe(&self, id: &str, from: Option<u64>) -> Result<EventStream, MarsadError> {
        let p = self.get(id)?;
        let (initial, rx, done, mut last) = {
            let s = lock(&p.scroll);
            let from = from.unwrap_or(0);
            let end = s.end();
            // Output cursors reach `end`; `exited` is the one after it.
            let seen_to = if s.exited.is_some() { end + 1 } else { end };
            if from > seen_to {
                return Err(MarsadError::invalid(format!(
                    "cursor {from} is beyond the output of terminal {id}"
                )));
            }
            let mut initial: Vec<Live> = Vec::new();
            if from < end {
                let at = from.max(s.start);
                let skip = (at - s.start) as usize;
                let bytes: Vec<u8> = s.data.iter().skip(skip).copied().collect();
                initial.push((
                    end,
                    PtyEvent::Output {
                        offset: at,
                        data: b64(&bytes),
                    },
                ));
            }
            if let Some(code) = s.exited {
                if from <= end {
                    initial.push((end + 1, PtyEvent::Exited { code }));
                }
            }
            (initial, p.live.subscribe(), s.exited.is_some(), from)
        };
        let head = futures::stream::iter(initial.into_iter().map(Ok));
        let live = futures::stream::unfold((rx, done), move |(mut rx, done)| async move {
            if done {
                return None;
            }
            match rx.recv().await {
                Ok(ev) => {
                    let fin = matches!(ev.1, PtyEvent::Exited { .. });
                    Some((Ok(ev), (rx, fin)))
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    Some((Err(CloseReason::Lagged), (rx, true)))
                }
                Err(broadcast::error::RecvError::Closed) => None,
            }
        });
        Ok(head
            .chain(live)
            .filter_map(move |item| {
                let out = match item {
                    Ok((cursor, ev)) if cursor > last => {
                        last = cursor;
                        Some(
                            serde_json::to_value(&ev)
                                .map(|v| (Cursor::from_u64(cursor), v))
                                .map_err(|e| CloseReason::Error {
                                    error: wasit_control::RpcError::internal(e.to_string()),
                                }),
                        )
                    }
                    Ok(_) => None,
                    Err(e) => Some(Err(e)),
                };
                futures::future::ready(out)
            })
            .boxed())
    }
}

fn spawn_writer(
    mut writer: Box<dyn Write + Send>,
    mut rx: mpsc::Receiver<Vec<u8>>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("pty-write".into())
        .spawn(move || {
            while let Some(bytes) = rx.blocking_recv() {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        })
        .map(|_| ())
}

fn pump(p: &Pty, mut reader: Box<dyn Read + Send>) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let mut s = lock(&p.scroll);
        let offset = s.end();
        s.data.extend(&buf[..n]);
        let excess = s.data.len().saturating_sub(SCROLLBACK);
        if excess > 0 {
            s.data.drain(..excess);
            s.start += excess as u64;
        }
        let end = s.end();
        // No subscribers is fine.
        let _ = p.live.send((
            end,
            PtyEvent::Output {
                offset,
                data: b64(&buf[..n]),
            },
        ));
    }
}
