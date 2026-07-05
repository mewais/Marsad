//! pty.rs — WAVE 3b: the interactive raw-shell PTY escape hatch (Session
//! Level 3 "Terminal" tab, `docs/design/DESIGN_HANDOFF.md`).
//!
//! This is deliberately a SEPARATE channel from the claude driver
//! (`driver.rs`/`reader.rs`): it spawns a REAL pty running a plain
//! interactive shell — never claude, never `--print`/stream-json — so the
//! rest of the crate's "never parse rendered terminal output" rule
//! (`docs/specs/agent.md`) does not apply here; this module's entire job IS
//! a raw terminal.
//!
//! - **Local** sessions: `portable-pty` opens a real OS pty and spawns the
//!   user's shell (`$SHELL`, falling back to `/bin/sh` on Unix / `cmd.exe`
//!   on Windows) directly in it.
//! - **Remote** sessions: the SAME pty machinery spawns the session's
//!   existing SSH/WSL transport (`remote::exec_prefix_for`), but with real
//!   pty allocation requested (`-tt` for ssh, `-t` for plink; `wsl.exe`
//!   already launches an interactive default shell) instead of the one-shot
//!   script transport's `-T`/no-pty flags — so the remote shell is a REAL
//!   interactive TTY session, not a piped script.
//!
//! ## Coverage discipline
//! `default_shell`/`pty_exec_argv` are pure (no IO) and always compiled +
//! unit-tested for 100% coverage. The real pty IO (`spawn_pty`/`resize_pty`,
//! the per-session registry, and the broadcast-pump/teardown wiring) is
//! gated `#[cfg(not(coverage))]` — the same "real IO like `TmuxContainer`/
//! `ClaudeFifoTransport`" disposition as the rest of this crate's real-IO
//! surface (see `AGENTS.md`). UNLIKE `TmuxContainer` (which needs a live
//! `tmux` + `claude` and is therefore only exercised by the `i03` live
//! integration test), spawning a local pty running a plain shell has no
//! external dependency beyond a pty device + a shell binary — both present
//! in this sandbox — so it IS genuinely unit-tested here, test-first, in
//! `crates/marsad-agent/tests/pty.rs`. That file is `#![cfg(not(coverage))]`
//! at the top so it compiles to zero tests (not a build error) under
//! `cargo llvm-cov`, which sets `--cfg coverage` and therefore does not see
//! the `#[cfg(not(coverage))]` API it calls. See `docs/GAPS.md`.

use std::io::{Read, Write};

// ---------------------------------------------------------------------------
// PtyError — only ever constructed by the real-IO functions below, so it is
// gated the same way they are (`#[cfg(not(coverage))]`): under coverage
// instrumentation nothing would ever construct or Display it, which would
// otherwise show as permanently-uncovered dead code.
// ---------------------------------------------------------------------------

#[cfg(not(coverage))]
#[derive(Debug)]
pub enum PtyError {
    Spawn(String),
    Io(String),
}

#[cfg(not(coverage))]
impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PtyError::Spawn(s) => write!(f, "pty spawn error: {s}"),
            PtyError::Io(s) => write!(f, "pty io error: {s}"),
        }
    }
}

#[cfg(not(coverage))]
impl std::error::Error for PtyError {}

// ---------------------------------------------------------------------------
// default_shell / pty_exec_argv — pure logic, always compiled + unit-tested
// ---------------------------------------------------------------------------

/// Resolve the shell to run for a LOCAL pty from an explicit `$SHELL` value
/// (`None`/empty means "not set") — pure, no environment reads, so both
/// branches are directly testable without mutating global process state
/// (env-var tests are otherwise racy: Rust runs a crate's tests in parallel
/// threads within one process, sharing the environment).
///
/// Returns `shell_env` verbatim if set and non-empty, else a
/// POSIX-guaranteed fallback (`cmd.exe` on Windows, `/bin/sh` elsewhere).
pub fn shell_for(shell_env: Option<&str>) -> String {
    if let Some(s) = shell_env {
        if !s.trim().is_empty() {
            return s.to_string();
        }
    }
    fallback_shell()
}

/// The POSIX-guaranteed fallback shell, `#[cfg]`-selected at COMPILE time
/// (not a runtime `if cfg!(windows)`) so a single-platform CI/coverage run
/// never sees the other platform's literal as an unreachable branch.
#[cfg(windows)]
fn fallback_shell() -> String {
    "cmd.exe".to_string()
}

#[cfg(not(windows))]
fn fallback_shell() -> String {
    "/bin/sh".to_string()
}

/// Production entry point: resolve the shell to run for a LOCAL pty from the
/// REAL `$SHELL` environment variable (see `shell_for` for the testable
/// logic this wraps).
pub fn default_shell() -> String {
    shell_for(std::env::var("SHELL").ok().as_deref())
}

/// Build the argv for the pty's child command for a session's `host_target`.
///
/// - `None`/`"local"`/`"localhost"` (i.e. `remote::exec_prefix_for` returns
///   `None`) → run `shell` directly, no wrapper.
/// - Otherwise, reuse `remote::exec_prefix_for`'s host resolution (the SAME
///   argv the agent already builds for that host's one-shot scripts /
///   durable session transport) but swap its "no pty" flags for "force a
///   real pty" ones:
///   - `ssh` (native or WSL-routed): `-T` (no pty, correct for the one-shot
///     script transport, which never needs a TTY) → `-tt` (force pty, even
///     though our own stdin isn't itself a tty — this makes the REMOTE shell
///     fully interactive).
///   - `plink`: carries no `-T`/no-pty flag to swap (PuTTY's plink defaults
///     differ), so `-t` (force pty) is inserted explicitly for robustness.
///   - bare `wsl:<distro>` (no nested ssh): `wsl.exe -d <distro>` already
///     launches an interactive default shell on its own — no flag to touch.
pub fn pty_exec_argv(host_target: Option<&str>, shell: &str) -> Vec<String> {
    match super::remote::exec_prefix_for(host_target) {
        None => vec![shell.to_string()],
        Some(prefix) => {
            if prefix.first().map(String::as_str) == Some("plink") {
                let mut argv = vec![prefix[0].clone(), "-t".to_string()];
                argv.extend(prefix.into_iter().skip(1));
                argv
            } else {
                prefix
                    .into_iter()
                    .map(|a| if a == "-T" { "-tt".to_string() } else { a })
                    .collect()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Real pty IO — production builds only (see module doc's "Coverage
// discipline" section)
// ---------------------------------------------------------------------------

/// A live pty: the writable master handle (for resize), the pty's stdin
/// writer, and the child process handle (for exit-status/kill).
#[cfg(not(coverage))]
pub struct PtyEntry {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
}

#[cfg(not(coverage))]
impl std::fmt::Debug for PtyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PtyEntry")
    }
}

/// A per-session live-pty registry entry, shared between the reader/pump
/// thread (which creates it and removes it on exit) and the IPC command
/// thread (`handle_pty_input`/`handle_pty_resize`/`handle_stop_session`,
/// which look it up to write/resize/kill).
#[cfg(not(coverage))]
pub(crate) type SharedPtyEntry = std::sync::Arc<std::sync::Mutex<PtyEntry>>;

/// The full per-session pty registry, keyed by Marsad session id. Mirrors
/// `session_writers`'s `Arc<Mutex<HashMap<..>>>` shape exactly.
#[cfg(not(coverage))]
pub(crate) type PtyRegistry =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, SharedPtyEntry>>>;

/// Open a real OS pty and spawn `argv` (`argv[0]` is the program, the rest
/// its arguments) into it, sized `cols`x`rows`, with an optional working
/// directory. Returns the live entry plus a cloned reader for the caller to
/// pump on its own thread (the entry itself holds the writer/master/child —
/// NOT the reader, which portable-pty only allows to be taken once but can
/// be cloned independently of the writer/master).
#[cfg(not(coverage))]
pub fn spawn_pty(
    argv: &[String],
    cwd: Option<&str>,
    cols: u16,
    rows: u16,
) -> Result<(PtyEntry, Box<dyn Read + Send>), PtyError> {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};

    let bin = argv
        .first()
        .ok_or_else(|| PtyError::Spawn("empty argv".to_string()))?;

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| PtyError::Spawn(format!("openpty: {e}")))?;

    let mut cmd = CommandBuilder::new(bin);
    cmd.args(&argv[1..]);
    if let Some(dir) = cwd {
        cmd.cwd(dir);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| PtyError::Spawn(format!("spawn {bin}: {e}")))?;
    // Drop the slave in the parent so the master's reader end sees EOF once
    // the child (the only remaining slave-fd holder) exits.
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| PtyError::Io(format!("clone reader: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| PtyError::Io(format!("take writer: {e}")))?;

    Ok((
        PtyEntry {
            master: pair.master,
            writer,
            child,
        },
        reader,
    ))
}

/// Resize a live pty's viewport (updates the kernel's winsize and signals
/// the child, per `portable_pty::MasterPty::resize`'s contract).
#[cfg(not(coverage))]
pub fn resize_pty(entry: &PtyEntry, cols: u16, rows: u16) -> Result<(), PtyError> {
    entry
        .master
        .resize(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| PtyError::Io(format!("resize: {e}")))
}

// ---------------------------------------------------------------------------
// Agent-facing wiring: spawn + broadcast pump + teardown
// ---------------------------------------------------------------------------

/// Handle a `PtyStart` request: spawn the pty on its own thread (so the IPC
/// command loop is never blocked on the child's own startup), register it in
/// `entries`, then pump its output as `PtyOutput`/`PtyExit` broadcasts to
/// every subscriber — the same broadcast list `spawn_session_reader` (in
/// `reader.rs`) uses for `AgentEvent`s.
///
/// Called from `serve.rs`'s accept-thread reply handling (mirrors how a new
/// `LaunchSession`'s FIFO reader is spawned there) rather than directly from
/// `ipc.rs`'s `handle_pty_start`: only `serve()`'s scope owns the
/// `subscribers` broadcast list, so the spawn is signalled back through
/// `AgentReply.pty_start` (parsed straight off the inbound `PtyStart` line,
/// same "parse the request, not just the reply" pattern already used for
/// `launch_first_prompt`).
///
/// Idempotent: if a pty is already live for `sid` (the user re-opened the
/// Terminal tab), this is a no-op — the existing pty keeps running
/// (tmux-pane-style detach/reattach; the frontend simply won't see
/// historical scrollback on re-attach, a documented gap — see
/// `docs/GAPS.md`).
#[cfg(not(coverage))]
pub(crate) fn spawn_pty_session(
    sid: String,
    cols: u16,
    rows: u16,
    project: String,
    host_target: Option<String>,
    subs: std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<String>>>>,
    entries: PtyRegistry,
) {
    std::thread::spawn(move || {
        if entries.lock().unwrap().contains_key(&sid) {
            eprintln!("marsad-agent: session {sid} PtyStart — pty already live, no-op");
            return;
        }

        let shell = default_shell();
        let argv = pty_exec_argv(host_target.as_deref(), &shell);
        // Local sessions start in the session's project dir. A remote pty's
        // cwd is whatever the remote login shell/wsl distro defaults to —
        // there is no `cd` step here (unlike the one-shot `run_shell`
        // transport): doing so would mean shell-quoting a `cd` into argv,
        // which conflicts with the pty-allocation flags `pty_exec_argv`
        // just computed. Documented gap (BUILT-NOT-VERIFIED remote path,
        // no live remote host in this sandbox) — see `docs/GAPS.md`.
        let cwd = if host_target.is_none() {
            Some(project.as_str())
        } else {
            None
        };

        match spawn_pty(&argv, cwd, cols, rows) {
            Ok((entry, reader)) => {
                let shared = std::sync::Arc::new(std::sync::Mutex::new(entry));
                entries
                    .lock()
                    .unwrap()
                    .insert(sid.clone(), std::sync::Arc::clone(&shared));
                eprintln!("marsad-agent: session {sid} pty started ({cols}x{rows})");
                pump_pty_output(sid, reader, subs, entries);
            }
            Err(e) => {
                eprintln!("marsad-agent: session {sid} pty spawn failed: {e}");
                let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::PtyExit {
                    session_id: sid,
                    code: -1,
                });
                let subs = subs.lock().unwrap();
                subs.iter().for_each(|s| {
                    let _ = s.send(line.clone());
                });
            }
        }
    });
}

/// Read loop: pump raw bytes off the pty's reader end, base64-encode each
/// chunk (see `marsad_proto::IpcMsg::PtyOutput`'s doc comment for why), and
/// broadcast as `PtyOutput` to every subscriber. On EOF/read-error (the
/// child exited or the pty tore down), reap the child for its real exit
/// code, remove the registry entry, and broadcast `PtyExit`.
#[cfg(not(coverage))]
fn pump_pty_output(
    sid: String,
    mut reader: Box<dyn Read + Send>,
    subs: std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<String>>>>,
    entries: PtyRegistry,
) {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;

    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let data = BASE64.encode(&buf[..n]);
                let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::PtyOutput {
                    session_id: sid.clone(),
                    data,
                });
                let subs_guard = subs.lock().unwrap();
                subs_guard.iter().for_each(|s| {
                    let _ = s.send(line.clone());
                });
            }
        }
    }

    // Reap the child for its real exit code. If `StopSession` already tore
    // this entry down concurrently (killed it directly), the registry slot
    // is already gone — report a benign 0 rather than re-deriving a code
    // from a child we no longer own (idempotent teardown race, both paths
    // converge safely; see `teardown_session`).
    let code = match entries.lock().unwrap().remove(&sid) {
        Some(entry_arc) => {
            let mut e = entry_arc.lock().unwrap();
            e.child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1)
        }
        None => 0,
    };
    eprintln!("marsad-agent: session {sid} pty exited (code {code})");
    let line = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::PtyExit {
        session_id: sid,
        code,
    });
    let subs_guard = subs.lock().unwrap();
    subs_guard.iter().for_each(|s| {
        let _ = s.send(line.clone());
    });
}

/// Clean teardown for `StopSession`: kill + reap any live pty for `sid` and
/// remove it from the registry. Idempotent (a no-op if no pty is live, or if
/// the pty already exited on its own and `pump_pty_output` already removed
/// the entry).
#[cfg(not(coverage))]
pub(crate) fn teardown_session(sid: &str, entries: &PtyRegistry) {
    let removed = entries.lock().unwrap().remove(sid);
    if let Some(entry_arc) = removed {
        let mut e = entry_arc.lock().unwrap();
        let _ = e.child.kill();
        let _ = e.child.wait();
        eprintln!("marsad-agent: session {sid} pty killed (StopSession)");
    }
}
