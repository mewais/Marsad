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
        // Idempotency guard: adopt without spawning if already running.
        if self.exists(name) {
            return Ok(());
        }

        let dir = self.session_dir(name);
        std::fs::create_dir_all(&dir)
            .map_err(|e| LifecycleError::Spawn(format!("mkdir {dir:?}: {e}")))?;

        let in_fifo = dir.join("in.fifo");
        let out_fifo = dir.join("out.fifo");

        // Use the `mkfifo` utility (POSIX, dependency-free from Rust's std).
        // Only create each FIFO if it does not already exist.
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
