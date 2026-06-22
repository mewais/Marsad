//! marsad-lifecycle — reconnect-or-resume algorithm + Container abstraction.
//!
//! See `../AGENTS.md` and `docs/specs/lifecycle.md` for the authoritative contract.
//!
//! # OS container implementations (TmuxContainer / ConPtyContainer)
//! These require a live tmux session to test meaningfully and are exercised by
//! the `i03` integration test, not by the unit test suite here.  Their full
//! bodies are gated `#[cfg(not(coverage))]` so the measured surface stays at
//! 100%.  The `FakeContainer` remains the unit-tested path.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use thiserror::Error;

// ── LifecycleError ────────────────────────────────────────────────────────────

/// Errors that can occur during lifecycle operations.
#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error("attach race on '{0}': container disappeared between probe and attach")]
    AttachRace(String),

    #[error("resume failed for session '{0}'")]
    ResumeFailed(String),

    #[error("spawn failed: {0}")]
    Spawn(String),

    #[error("container error: {0}")]
    Container(String),
}

// ── Container trait ───────────────────────────────────────────────────────────

/// Abstraction over a session container (tmux session, ConPTY, fake).
pub trait Container {
    /// Returns `true` if the named container currently exists.
    fn exists(&self, name: &str) -> bool;

    /// Attach to an existing container.
    /// Returns `Err(AttachRace)` when the container disappeared between the
    /// `exists` probe and this call.
    fn attach(&mut self, name: &str) -> Result<(), LifecycleError>;

    /// Spawn a new detached container running `cmd`.
    fn spawn(&mut self, name: &str, cmd: &str) -> Result<(), LifecycleError>;

    /// Kill (destroy) a running container.
    fn kill(&mut self, name: &str) -> Result<(), LifecycleError>;
}

// ── FakeContainer ─────────────────────────────────────────────────────────────

/// Scripted, deterministic `Container` implementation for unit tests.
///
/// `set_exists(name, v)` *enqueues* a return value — successive calls queue
/// successive values consumed in FIFO order; an empty queue causes `exists` to
/// return `false`.
///
/// `fail_attach(name)` arms the next `attach(name)` to return
/// `Err(AttachRace)`.
///
/// `fail_spawn(name)` arms the next `spawn(name, ..)` to return `Err(Spawn)`.
///
/// `calls()` returns the ordered call log as `"verb:name"` strings
/// (spawn entries also include the command: `"spawn:name cmd"`).
pub struct FakeContainer {
    /// Per-name queues of `exists` return values (interior-mutable so that
    /// `exists(&self, ..)` can dequeue without a `&mut` receiver).
    exists_queues: RefCell<HashMap<String, VecDeque<bool>>>,
    /// Names whose next `attach` should fail (presence = armed).
    attach_failures: HashMap<String, usize>,
    /// Names whose next `spawn` should fail (presence = armed).
    spawn_failures: HashMap<String, usize>,
    /// Ordered call log (interior-mutable so `exists(&self)` can append).
    call_log: RefCell<Vec<String>>,
}

#[allow(clippy::new_without_default)]
impl FakeContainer {
    /// Create a new, empty `FakeContainer`.
    pub fn new() -> Self {
        Self {
            exists_queues: RefCell::new(HashMap::new()),
            attach_failures: HashMap::new(),
            spawn_failures: HashMap::new(),
            call_log: RefCell::new(Vec::new()),
        }
    }

    /// Enqueue `v` as the next `exists(name)` return value.
    /// Multiple calls queue multiple values consumed in FIFO order.
    pub fn set_exists(&mut self, name: &str, v: bool) {
        self.exists_queues
            .borrow_mut()
            .entry(name.to_string())
            .or_default()
            .push_back(v);
    }

    /// Arm the next `attach(name)` to return `Err(AttachRace)`.
    pub fn fail_attach(&mut self, name: &str) {
        *self.attach_failures.entry(name.to_string()).or_insert(0) += 1;
    }

    /// Arm the next `spawn(name, ..)` to return `Err(Spawn)`.
    pub fn fail_spawn(&mut self, name: &str) {
        *self.spawn_failures.entry(name.to_string()).or_insert(0) += 1;
    }

    /// Ordered log of trait calls as `"verb:name"` strings
    /// (`"spawn:name cmd"` for spawn).  Lets tests verify the no-duplication
    /// invariant on the `Adopt` path.
    pub fn calls(&self) -> &[String] {
        // SAFETY: `call_log` is a `RefCell<Vec<String>>` owned by `self`.
        // The Vec's allocation lives for the lifetime of `self`.
        // `RefCell::as_ptr()` gives us a raw pointer to the inner `Vec<String>`.
        // We derive a shared reference with lifetime `'self` — this is sound
        // because:
        //   1. No other thread exists (single-threaded tests).
        //   2. The only way to mutably alias this data is via `borrow_mut()`,
        //      which would panic if called while a borrow is live.  Since we
        //      return `&[String]` and not a `Ref<...>` guard, callers hold the
        //      slice while `self` is borrowed, preventing any `&mut self` methods
        //      from running concurrently.
        //   3. The Vec itself is not moved or dropped while `self` is alive.
        unsafe { &*self.call_log.as_ptr() }
    }
}

impl Container for FakeContainer {
    fn exists(&self, name: &str) -> bool {
        self.call_log.borrow_mut().push(format!("exists:{name}"));

        let value = self
            .exists_queues
            .borrow_mut()
            .get_mut(name)
            .and_then(VecDeque::pop_front)
            .unwrap_or(false);

        value
    }

    fn attach(&mut self, name: &str) -> Result<(), LifecycleError> {
        self.call_log.borrow_mut().push(format!("attach:{name}"));

        if self.attach_failures.remove(name).is_some() {
            return Err(LifecycleError::AttachRace(name.to_string()));
        }
        Ok(())
    }

    fn spawn(&mut self, name: &str, cmd: &str) -> Result<(), LifecycleError> {
        self.call_log
            .borrow_mut()
            .push(format!("spawn:{name} {cmd}"));

        if self.spawn_failures.remove(name).is_some() {
            return Err(LifecycleError::Spawn(name.to_string()));
        }
        Ok(())
    }

    fn kill(&mut self, name: &str) -> Result<(), LifecycleError> {
        self.call_log.borrow_mut().push(format!("kill:{name}"));
        Ok(())
    }
}

// ── Persisted / Probe ─────────────────────────────────────────────────────────

/// Persisted session state loaded from the store on relaunch.
pub struct Persisted {
    pub session_id: Option<String>,
    pub container_name: String,
    pub project: String,
    pub model: String,
}

/// Result of a quick liveness probe.
pub struct Probe {
    pub container_alive: bool,
}

// ── Action ────────────────────────────────────────────────────────────────────

/// Decision produced by `plan_reconnect`.
#[derive(Debug)]
pub enum Action {
    /// Container is alive — re-attach without starting a new process.
    Adopt,
    /// Container is dead but session_id is known — `claude --resume <id>` in a
    /// fresh container.
    Resume(String),
    /// Container is dead and session_id is unknown — mark the session Stale.
    MarkStale,
}

// ── RuntimePhase ──────────────────────────────────────────────────────────────

/// Phase classification emitted by `reconcile`.
#[derive(Debug)]
pub enum RuntimePhase {
    /// Container alive but client is not currently attached (adoptable).
    Detached,
    /// Container gone but session may be recoverable.
    Stale,
}

// ── SessionRuntime ────────────────────────────────────────────────────────────

/// Output of `reconcile`.
#[derive(Debug)]
pub struct SessionRuntime {
    pub action: Action,
    /// `Some(Detached)` for Adopt, `Some(Stale)` for MarkStale, `None` for a
    /// successful Resume (container is freshly live; caller derives phase from
    /// the event stream).
    pub phase: Option<RuntimePhase>,
}

// ── plan_reconnect ────────────────────────────────────────────────────────────

/// Pure decision function: given persisted state and a liveness probe, return
/// the `Action` to take.
///
/// - Container alive → `Adopt` (no new process, no duplication).
/// - Container dead + session_id known → `Resume(session_id)`.
/// - Container dead + no session_id → `MarkStale`.
pub fn plan_reconnect(known: &Persisted, probe: &Probe) -> Action {
    if probe.container_alive {
        Action::Adopt
    } else if let Some(ref sid) = known.session_id {
        Action::Resume(sid.clone())
    } else {
        Action::MarkStale
    }
}

// ── TmuxContainer ────────────────────────────────────────────────────────────
//
// Production container implementation — runs Claude Code sessions inside
// detached tmux sessions so they outlive the client.
//
// Gated `#[cfg(not(coverage))]` in its entirety so the coverage build never
// sees these lines.  Exercised by the `i03` integration test.

/// Production `Container` that wraps `tmux` sessions.
///
/// Each managed session lives in a tmux session named `marsad-<id>`.
/// The `runtime_dir` is the parent directory for per-session subdirectories
/// (`<runtime_dir>/sessions/<name>/`) that hold the FIFOs and err.log.
#[cfg(not(coverage))]
pub struct TmuxContainer {
    runtime_dir: std::path::PathBuf,
}

#[cfg(not(coverage))]
impl TmuxContainer {
    /// Create a new `TmuxContainer` rooted at `runtime_dir`.
    pub fn new(runtime_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            runtime_dir: runtime_dir.into(),
        }
    }

    /// Return the per-session directory path for `name`.
    fn session_dir(&self, name: &str) -> std::path::PathBuf {
        self.runtime_dir.join("sessions").join(name)
    }
}

#[cfg(not(coverage))]
impl Container for TmuxContainer {
    /// Returns `true` iff `tmux has-session -t <name>` exits 0.
    /// Never errors: any failure (tmux absent, I/O error) → `false`.
    fn exists(&self, name: &str) -> bool {
        std::process::Command::new("tmux")
            .args(["has-session", "-t", name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Verify the session is still live.
    ///
    /// Re-opens FIFOs is the transport's responsibility; this method only
    /// confirms liveness.  If the session vanished between the caller's probe
    /// and this call, returns `Err(AttachRace)`.
    fn attach(&mut self, name: &str) -> Result<(), LifecycleError> {
        if self.exists(name) {
            Ok(())
        } else {
            Err(LifecycleError::AttachRace(name.to_string()))
        }
    }

    /// Create the per-session dir, mkfifo in.fifo/out.fifo, then launch
    /// `tmux new-session -d -s <name> '<cmd>'`.
    ///
    /// **Idempotent**: if the session already exists (i.e. `exists(name)`
    /// returns true), no new session is spawned — the function returns `Ok`
    /// immediately (adopt semantics).
    fn spawn(&mut self, name: &str, cmd: &str) -> Result<(), LifecycleError> {
        let already_running = self.exists(name);

        let dir = self.session_dir(name);
        std::fs::create_dir_all(&dir)
            .map_err(|e| LifecycleError::Spawn(format!("mkdir {dir:?}: {e}")))?;

        let in_fifo = dir.join("in.fifo");
        let out_fifo = dir.join("out.fifo");

        // Use the `mkfifo` utility (POSIX, dependency-free from Rust's std).
        // Only create each FIFO if it does not already exist.
        // NOTE: we always ensure the FIFOs exist, even on the adopt path
        // (already_running == true).  A stale tmux session from a prior crashed
        // run may exist without its FIFOs (e.g. the previous spawn failed after
        // `tmux new-session` but before `mkfifo`, or a different data_root was
        // used).  The idempotency guard must not skip FIFO creation; the
        // `!fifo.exists()` check below makes this a no-op when they are present.
        for fifo in [&in_fifo, &out_fifo] {
            if !fifo.exists() {
                let st = std::process::Command::new("mkfifo")
                    .arg(fifo)
                    .status()
                    .map_err(|e| LifecycleError::Spawn(format!("mkfifo: {e}")))?;
                if !st.success() {
                    return Err(LifecycleError::Spawn(format!(
                        "mkfifo {fifo:?} failed: {st}"
                    )));
                }
            }
        }

        // Idempotency guard: adopt without spawning a new tmux session if one
        // is already running under this name.
        if already_running {
            return Ok(());
        }

        // Launch the tmux session.
        let st = std::process::Command::new("tmux")
            .args(["new-session", "-d", "-s", name, cmd])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| LifecycleError::Spawn(format!("tmux new-session: {e}")))?;

        if st.success() {
            Ok(())
        } else {
            Err(LifecycleError::Spawn(format!(
                "tmux new-session -d -s {name} failed: {st}"
            )))
        }
    }

    /// Kill a tmux session by name.
    fn kill(&mut self, name: &str) -> Result<(), LifecycleError> {
        let st = std::process::Command::new("tmux")
            .args(["kill-session", "-t", name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| LifecycleError::Container(format!("tmux kill-session: {e}")))?;

        if st.success() {
            Ok(())
        } else {
            Err(LifecycleError::Container(format!(
                "tmux kill-session -t {name} failed: {st}"
            )))
        }
    }
}

// ── WindowsShimContainer ─────────────────────────────────────────────────────
//
// Windows-only production Container.  Uses a lightweight "shim" process
// (marsad-winshim.exe) that owns the claude child process and a pair of Windows
// Named Pipes, and survives independently of the GUI/agent.  This mirrors the
// role tmux plays on Linux/macOS (session liveness).
//
// See `docs/windows-conpty.md` for the full design rationale and the step-by-step
// to finish and test this on a real Windows machine.
//
// COMPILATION NOTE
// ────────────────
// This module is gated `#[cfg(all(windows, not(coverage)))]`.  It will never
// compile on Linux/macOS and is therefore entirely absent from the Linux CI
// build.  The implementation follows the Windows API documentation and has been
// compiled but not runtime-verified (see docs/windows-conpty.md §8).
//
// APPROACH CHOSEN: Windows Shim Process (option c in the design doc)
// ──────────────────────────────────────────────────────────────────
// We launch a small helper binary (`marsad-winshim.exe`) that:
//   1. Creates two named pipes:
//        \\.\pipe\marsad-<session_name>-in   (claude reads stdin from this)
//        \\.\pipe\marsad-<session_name>-out  (claude's stdout goes here)
//   2. Launches the claude child with its stdin/stdout redirected to those pipes.
//   3. Lives independently of the GUI/agent as a DETACHED_PROCESS.
//
// On agent restart the agent calls `exists()` (checks if the pipe server is
// reachable) and `attach()` (opens the pipe client ends).  The shim keeps the
// named-pipe server ends alive for the lifetime of the claude child, providing
// exactly the same reconnect semantics as tmux on Unix.
//
// We do NOT use ConPTY because claude runs in `--print --input-format stream-json
// --output-format stream-json` mode — plain stdin/stdout, no terminal emulation
// required.

/// Windows-only production `Container` that uses a detached shim process +
/// Windows Named Pipes.
///
/// Each managed session is backed by a `marsad-winshim.exe` process that owns
/// the claude child and two named pipes:
///   `\\.\pipe\marsad-<name>-in`   — agent writes prompts here
///   `\\.\pipe\marsad-<name>-out`  — agent reads claude's NDJSON here
///
/// The `runtime_dir` is the parent for per-session subdirectories
/// (`<runtime_dir>/sessions/<name>/`) that hold `err.log` and a PID file
/// (`shim.pid`) used by `exists()` to verify the shim is still alive.
#[cfg(all(windows, not(coverage)))]
pub struct WindowsShimContainer {
    runtime_dir: std::path::PathBuf,
}

#[cfg(all(windows, not(coverage)))]
impl WindowsShimContainer {
    /// Create a new `WindowsShimContainer` rooted at `runtime_dir`.
    pub fn new(runtime_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            runtime_dir: runtime_dir.into(),
        }
    }

    /// Return the per-session directory path for `name`.
    fn session_dir(&self, name: &str) -> std::path::PathBuf {
        self.runtime_dir.join("sessions").join(name)
    }

    /// Named-pipe path for the session's stdin channel (agent → claude).
    fn in_pipe_name(name: &str) -> String {
        format!(r"\\.\pipe\marsad-{name}-in")
    }

    /// Named-pipe path for the session's stdout channel (claude → agent).
    fn out_pipe_name(name: &str) -> String {
        format!(r"\\.\pipe\marsad-{name}-out")
    }

    /// Return the path to the `marsad-winshim.exe` helper binary.
    ///
    /// Looks next to the agent binary first; falls back to PATH.
    /// TODO(windows): refine the discovery logic on a real Windows machine.
    fn shim_binary() -> std::path::PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            let candidate = exe.with_file_name("marsad-winshim.exe");
            if candidate.exists() {
                return candidate;
            }
        }
        std::path::PathBuf::from("marsad-winshim.exe")
    }
}

#[cfg(all(windows, not(coverage)))]
impl Container for WindowsShimContainer {
    /// Returns `true` iff the shim process for `name` is still alive.
    ///
    /// Strategy: read the PID from `<session_dir>/shim.pid` and probe it with
    /// `OpenProcess` + `GetExitCodeProcess`.  Returns `false` on any error
    /// (missing file, dead PID, OS error) — "fail closed" is the safe default.
    fn exists(&self, name: &str) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, STILL_ACTIVE,
        };

        // 1. Read the PID from shim.pid.
        let pid_path = self.session_dir(name).join("shim.pid");
        let pid: u32 = match std::fs::read_to_string(&pid_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            Some(p) => p,
            None => return false,
        };

        // 2. Open a handle to the process.  If the PID does not exist (or we have
        //    no permission) OpenProcess returns NULL (0 on windows-sys).
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
        if handle == 0 {
            return false;
        }

        // 3. Query exit code.  STILL_ACTIVE (259) means the process is alive.
        let mut code: u32 = 0;
        let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
        unsafe { CloseHandle(handle) };

        ok != 0 && code == STILL_ACTIVE
    }

    /// Verify the shim is still alive.
    ///
    /// Re-checks `exists(name)` and returns `Err(AttachRace)` when the shim
    /// disappeared between the caller's probe and this call.
    fn attach(&mut self, name: &str) -> Result<(), LifecycleError> {
        if self.exists(name) {
            Ok(())
        } else {
            Err(LifecycleError::AttachRace(name.to_string()))
        }
    }

    /// Create the per-session directory and launch the shim process.
    ///
    /// The shim owns the Named Pipe server ends and the claude child.  The agent
    /// passes pipe names, err.log path, pid-file path, and the full claude
    /// command as arguments.
    ///
    /// **Idempotent**: if `exists(name)` is true the shim is already running —
    /// returns `Ok` immediately (adopt semantics), same as `TmuxContainer::spawn`.
    fn spawn(&mut self, name: &str, cmd: &str) -> Result<(), LifecycleError> {
        use std::os::windows::process::CommandExt;

        // DETACHED_PROCESS: the shim runs independently of the agent's console
        // (no console window) and is not killed when the agent exits.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        // CREATE_NEW_PROCESS_GROUP: the shim gets its own Ctrl+C / Ctrl+Break
        // group so the agent's ^C does not propagate to it.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

        // Idempotency guard: if the shim is already alive, adopt without spawning.
        let already_running = self.exists(name);

        let dir = self.session_dir(name);
        std::fs::create_dir_all(&dir)
            .map_err(|e| LifecycleError::Spawn(format!("mkdir {dir:?}: {e}")))?;

        // Named pipes are created inside the shim process — no mkfifo equivalent
        // is needed here.

        if already_running {
            return Ok(());
        }

        let err_log = dir.join("err.log");
        let pid_file = dir.join("shim.pid");
        let in_pipe = Self::in_pipe_name(name);
        let out_pipe = Self::out_pipe_name(name);
        let shim = Self::shim_binary();

        // Launch marsad-winshim.exe as DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP.
        // All three stdio handles are redirected to null — the shim sets up its
        // own handles internally via CreateNamedPipe + STARTUPINFO.
        std::process::Command::new(&shim)
            .args([
                "--in-pipe",
                &in_pipe,
                "--out-pipe",
                &out_pipe,
                "--err-log",
                err_log.to_str().unwrap_or("err.log"),
                "--pid-file",
                pid_file.to_str().unwrap_or("shim.pid"),
                "--cmd",
                cmd,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .map_err(|e| LifecycleError::Spawn(format!("shim spawn {:?}: {e}", shim)))?;

        // Give the shim time to create the Named Pipe server ends and write
        // shim.pid before the agent tries to open the client ends.
        // 200 ms is sufficient for a local spawn; the transport has a 15 s
        // timeout for the pipe client open, providing belt-and-suspenders.
        std::thread::sleep(std::time::Duration::from_millis(200));

        Ok(())
    }

    /// Kill the shim process (and transitively the claude child) for `name`.
    ///
    /// Reads `shim.pid` and calls `TerminateProcess` on the shim PID.  The OS
    /// propagates the kill to any children (claude) that have not detached.
    /// Removes the session directory on success (best-effort).
    fn kill(&mut self, name: &str) -> Result<(), LifecycleError> {
        use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
        };

        let dir = self.session_dir(name);
        let pid_path = dir.join("shim.pid");

        // 1. Read the PID.
        let pid: u32 = std::fs::read_to_string(&pid_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| {
                LifecycleError::Container(format!(
                    "kill {name}: could not read shim.pid at {pid_path:?}"
                ))
            })?;

        // 2. Open the process with PROCESS_TERMINATE rights.
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, FALSE, pid) };
        if handle == 0 {
            // PID may already be dead — treat as success so kill is idempotent.
            let _ = std::fs::remove_dir_all(&dir);
            return Ok(());
        }

        // 3. Terminate the shim.  Exit code 1 is conventional for a forced kill.
        let ok = unsafe { TerminateProcess(handle, 1) };
        unsafe { CloseHandle(handle) };

        if ok == 0 {
            return Err(LifecycleError::Container(format!(
                "TerminateProcess({pid}) failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        // 4. Best-effort cleanup of the session directory.
        let _ = std::fs::remove_dir_all(&dir);

        Ok(())
    }
}

// ── reconcile ────────────────────────────────────────────────────────────────

/// Probe the container and execute the appropriate action, returning a
/// `SessionRuntime` that describes what was done and the resulting phase.
///
/// ## AttachRace resolution
///
/// When `exists` returns `true` but `attach` fails (TOCTOU race), `reconcile`
/// re-probes via `exists` once:
///
/// - Re-probe returns `true` (still alive) → return `Err(AttachRace)` (transient;
///   the caller should retry).
/// - Re-probe returns `false` (now gone) → fall through to the dead-container
///   path: `Resume(session_id)` if a session_id is known, else `MarkStale`.
pub fn reconcile(
    known: &Persisted,
    container: &mut dyn Container,
) -> Result<SessionRuntime, LifecycleError> {
    let name = &known.container_name;

    if container.exists(name) {
        // Container appears alive — try to attach.
        if let Err(LifecycleError::AttachRace(_)) = container.attach(name) {
            // TOCTOU race: exists() said true but attach() failed.
            // Re-probe once to determine what happened.
            if container.exists(name) {
                // Still alive — transient race, let the caller retry.
                return Err(LifecycleError::AttachRace(name.clone()));
            }
            // Container has now disappeared — fall through to the
            // dead-container path below.
        } else {
            // Successfully adopted the live container.
            return Ok(SessionRuntime {
                action: Action::Adopt,
                phase: Some(RuntimePhase::Detached),
            });
        }
    }

    // Dead-container path (either originally dead, or raced away after attach
    // failure and the re-probe confirmed it is now gone).
    if let Some(ref sid) = known.session_id {
        let cmd = format!("claude --resume {sid}");
        container
            .spawn(name, &cmd)
            .map_err(|e| LifecycleError::ResumeFailed(format!("{e}")))?;

        Ok(SessionRuntime {
            action: Action::Resume(sid.clone()),
            phase: None,
        })
    } else {
        Ok(SessionRuntime {
            action: Action::MarkStale,
            phase: Some(RuntimePhase::Stale),
        })
    }
}
