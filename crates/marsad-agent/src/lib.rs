//! marsad-agent — detached supervisor daemon.
//!
//! See `../AGENTS.md` and `docs/specs/agent.md` for the authoritative contract.

use std::collections::HashMap;
use std::fmt;

use marsad_lifecycle::{FakeContainer, Persisted};
use marsad_notify::{CaptureNotifier, Notifier};
use marsad_store::{MemoryBackend, Store};

// ── AgentError ────────────────────────────────────────────────────────────────

/// Errors returned by the agent.
#[derive(Debug)]
pub enum AgentError {
    Listen(String),
    Spawn(String),
    Transport(String),
    Protocol(String),
    Lifecycle(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Listen(s) => write!(f, "listen error: {s}"),
            AgentError::Spawn(s) => write!(f, "spawn error: {s}"),
            AgentError::Transport(s) => write!(f, "transport error: {s}"),
            AgentError::Protocol(s) => write!(f, "protocol error: {s}"),
            AgentError::Lifecycle(s) => write!(f, "lifecycle error: {s}"),
        }
    }
}

impl std::error::Error for AgentError {}

// ── ListenSpec ────────────────────────────────────────────────────────────────

/// How the agent listens for incoming IPC connections.
#[derive(Debug)]
pub enum ListenSpec {
    UnixSocket(String),
    NamedPipe(String),
    Stdio,
}

// ── AgentConfig ───────────────────────────────────────────────────────────────

/// Configuration for the agent daemon.
#[derive(Debug)]
pub struct AgentConfig {
    pub listen: ListenSpec,
    pub data_root: String,
}

// ── Session registry ──────────────────────────────────────────────────────────

/// Agent-internal record for a live session.
#[allow(dead_code)]
#[derive(Debug)]
struct SessionRecord {
    marsad_id: String,
}

// ── Agent ─────────────────────────────────────────────────────────────────────

// Macro that generates a newtype wrapper with a placeholder `Debug` impl.
// Using a macro means the `fn fmt` body is attributed to the macro call site
// by LLVM's coverage instrumentation, not to a separate uncoverable line.
macro_rules! opaque_debug_wrap {
    ($vis:vis $name:ident ($inner:ty)) => {
        $vis struct $name($inner);
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(stringify!($name))
            }
        }
    };
}

opaque_debug_wrap!(NotifierWrap(Box<dyn Notifier>));
opaque_debug_wrap!(StoreWrap(Box<Store>));
opaque_debug_wrap!(ContainerWrap(Box<FakeContainer>));

// Real-IO container wrapper — production builds only.
// Uses the same opaque Debug pattern so `derive(Debug)` on `Agent` compiles.
#[cfg(not(coverage))]
struct RealContainerWrap(Option<Box<dyn marsad_lifecycle::Container>>);

#[cfg(not(coverage))]
impl fmt::Debug for RealContainerWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RealContainerWrap")
    }
}

// A per-session writer to claude's stdin, shared between the session's reader
// thread (which creates it) and the serve thread (handle_send_prompt /
// handle_answer_gate). Boxed + shared because remote writers (ssh/wsl child
// stdin) can't be `try_clone`d like a local FIFO `File`.
#[cfg(not(coverage))]
type SharedWriter = std::sync::Arc<std::sync::Mutex<Box<dyn std::io::Write + Send>>>;

// Per-session writer registry wrapper — production builds only.
// Gated out of the coverage build so no uncovered lines appear.
#[cfg(not(coverage))]
struct SessionWritersWrap(std::sync::Arc<std::sync::Mutex<HashMap<String, SharedWriter>>>);

#[cfg(not(coverage))]
impl fmt::Debug for SessionWritersWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionWritersWrap")
    }
}

// Per-session remote child registry wrapper — production builds only.
// Durable remote sessions spawn TWO ssh/wsl children per connection:
//   • a WRITER child (`cat >> in.fifo`) whose stdin is the agent's pipe, used to
//     forward prompts and gate responses to the detached remote claude;
//   • a READER child (`tail -n0 -f out.log`) whose stdout is the agent's pipe,
//     streaming new claude events to subscribers.
// Local sessions never populate this map (they use tmux/shim containers).
// On stop, a kill script is run over ssh instead of killing the child; the
// map still holds writer children so they can be reaped on agent shutdown.
// Gated out of the coverage build so no uncovered lines appear.
#[cfg(not(coverage))]
struct SessionChildrenWrap(std::sync::Arc<std::sync::Mutex<HashMap<String, std::process::Child>>>);

#[cfg(not(coverage))]
impl fmt::Debug for SessionChildrenWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionChildrenWrap")
    }
}

// Per-session remote run-command stash — production builds only.
// A remote launch computes the claude run command (setup + env +
// `cd … && exec <claude> <args>`) from the full LaunchSpec inside
// `launch_with_id` (the only place the posture + mode are known). The serve loop
// pops it by session_id after `handle_ipc` and hands it to the reader thread via
// `AgentReply.run_command`. Local launches insert nothing. Gated out of coverage.
#[cfg(not(coverage))]
struct SessionRunCmdsWrap(std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>);

#[cfg(not(coverage))]
impl fmt::Debug for SessionRunCmdsWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionRunCmdsWrap")
    }
}

/// A `can_use_tool` permission request claude is blocked on, awaiting an answer.
/// Keyed by session_id (claude asks one tool at a time per session and blocks
/// until the control_response arrives).
#[cfg(not(coverage))]
#[derive(Clone)]
struct PendingGate {
    request_id: String,
    input: serde_json::Value,
    /// True if this gate answers an MCP `elicitation` request (answered with
    /// `{action}`) rather than a `can_use_tool` request (`{behavior,
    /// updatedInput}`). See `handle_answer_gate`.
    elicitation: bool,
}

/// Per-session pending-gate registry. Shared (Arc) between the FIFO reader
/// threads (which insert when a `can_use_tool` arrives) and the agent command
/// loop (`handle_answer_gate`, which reads + clears when the GUI answers).
#[cfg(not(coverage))]
struct PendingGatesWrap(std::sync::Arc<std::sync::Mutex<HashMap<String, PendingGate>>>);

#[cfg(not(coverage))]
impl fmt::Debug for PendingGatesWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PendingGatesWrap")
    }
}

/// The detached supervisor daemon.
///
/// Always stores a `FakeContainer` — for production use `Agent::new` supplies an
/// empty/default `FakeContainer`; for tests `Agent::with_fakes` supplies a
/// pre-configured one.  This design keeps one code path (no dead branches) and
/// lets `container_calls()` work without unsafe code.
///
/// Under `cfg(not(coverage))` an optional real container can override the fake
/// for production use via `Agent::with_real_io`.
#[derive(Debug)]
pub struct Agent {
    #[allow(dead_code)]
    config: AgentConfig,
    sessions: HashMap<String, SessionRecord>,
    next_id: u64,
    /// The injected (or default) fake container (always present for unit tests).
    container: ContainerWrap,
    /// The injected (or default) notifier.
    notifier: NotifierWrap,
    /// Persistence store.
    store: StoreWrap,
    /// Production real container — when inner is `Some`, overrides `container`
    /// for all Container trait method calls.  Gated out of the coverage build
    /// so no uncovered lines appear.
    #[cfg(not(coverage))]
    real_container: RealContainerWrap,
    /// Per-session FIFO writers: keyed by session_id, holds the write end of
    /// `in.fifo` so prompts can be forwarded to claude after launch.
    /// Production builds only — gated out of the coverage build.
    #[cfg(not(coverage))]
    session_writers: SessionWritersWrap,
    /// Per-session pending permission gates awaiting a GUI answer.
    /// Production builds only — gated out of the coverage build.
    #[cfg(not(coverage))]
    pending_gates: PendingGatesWrap,
    /// Per-session remote child processes (single-connection ssh/plink/wsl
    /// sessions): keyed by session_id, holds the live `Child` so `kill_container`
    /// can terminate it on stop. Local sessions never populate this map.
    /// Production builds only — gated out of the coverage build.
    #[cfg(not(coverage))]
    session_children: SessionChildrenWrap,
    /// Per-session run command for remote single-connection sessions, computed at
    /// launch and consumed by the serve loop. Production builds only.
    #[cfg(not(coverage))]
    session_run_cmds: SessionRunCmdsWrap,
}

impl Agent {
    /// Construct from config using default (empty) fakes.
    /// No binding or socket opening occurs at construction time.
    pub fn new(cfg: AgentConfig) -> Result<Agent, AgentError> {
        // Validate listen spec for obviously invalid paths (null byte).
        if let ListenSpec::UnixSocket(ref path) = cfg.listen {
            if path.contains('\0') {
                return Err(AgentError::Listen(format!("invalid socket path: {path:?}")));
            }
        }
        Self::with_fakes(
            cfg,
            Box::new(FakeContainer::new()),
            Box::new(CaptureNotifier::new()),
            Box::new(Store::new(Box::new(MemoryBackend::new()))),
        )
    }

    /// Construct with injected fakes for unit testing.
    pub fn with_fakes(
        cfg: AgentConfig,
        container: Box<FakeContainer>,
        notifier: Box<dyn Notifier>,
        store: Box<Store>,
    ) -> Result<Agent, AgentError> {
        Ok(Agent {
            config: cfg,
            sessions: HashMap::new(),
            next_id: 1,
            container: ContainerWrap(container),
            notifier: NotifierWrap(notifier),
            store: StoreWrap(store),
            #[cfg(not(coverage))]
            real_container: RealContainerWrap(None),
            #[cfg(not(coverage))]
            session_writers: SessionWritersWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            #[cfg(not(coverage))]
            pending_gates: PendingGatesWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            #[cfg(not(coverage))]
            session_children: SessionChildrenWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            #[cfg(not(coverage))]
            session_run_cmds: SessionRunCmdsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
        })
    }

    /// Construct with real OS IO: `TmuxContainer` (Unix) / `WindowsShimContainer`
    /// (Windows), `DbusNotifier`, `FsBackend`.
    ///
    /// Gated `#[cfg(not(coverage))]` so the coverage build never sees these
    /// lines.  Called by `main` in production; integration tests call it
    /// directly.
    #[cfg(not(coverage))]
    pub fn with_real_io(cfg: AgentConfig) -> Result<Agent, AgentError> {
        use marsad_notify::DbusNotifier;
        use marsad_store::{FsBackend, Store};

        let data_root = std::path::PathBuf::from(&cfg.data_root);

        // Select the session container based on the current platform.
        // On Unix we use tmux (proven, handles liveness + reconnect via FIFOs).
        // On Windows we use the marsad-winshim process (Named Pipe shim, same
        // liveness/reconnect contract — see docs/windows-conpty.md).
        #[cfg(unix)]
        let real_c: Box<dyn marsad_lifecycle::Container> = {
            use marsad_lifecycle::TmuxContainer;
            Box::new(TmuxContainer::new(data_root.clone()))
        };
        #[cfg(windows)]
        let real_c: Box<dyn marsad_lifecycle::Container> = {
            use marsad_lifecycle::WindowsShimContainer;
            Box::new(WindowsShimContainer::new(data_root.clone()))
        };

        let notifier = Box::new(DbusNotifier::new());
        let store = Box::new(Store::new(Box::new(FsBackend::new(data_root))));

        Ok(Agent {
            config: cfg,
            sessions: HashMap::new(),
            next_id: 1,
            // Fake container slot: unused in real-io mode but kept to satisfy the
            // type (container_calls() is a no-op here; integration tests assert
            // session existence directly, not via container_calls()).
            container: ContainerWrap(Box::new(FakeContainer::new())),
            notifier: NotifierWrap(notifier),
            store: StoreWrap(store),
            real_container: RealContainerWrap(Some(real_c)),
            session_writers: SessionWritersWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            pending_gates: PendingGatesWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            session_children: SessionChildrenWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            session_run_cmds: SessionRunCmdsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
        })
    }

    /// Return a mutable reference to whichever container is active.
    ///
    /// Production builds route through `real_container` when set; coverage and
    /// test builds always route through the fake.
    #[cfg(not(coverage))]
    fn active_container(&mut self) -> &mut dyn marsad_lifecycle::Container {
        if let Some(ref mut rc) = self.real_container.0 {
            rc.as_mut()
        } else {
            self.container.0.as_mut()
        }
    }

    #[cfg(coverage)]
    fn active_container(&mut self) -> &mut dyn marsad_lifecycle::Container {
        self.container.0.as_mut()
    }

    /// Return the ordered call log of the injected `FakeContainer`.
    pub fn container_calls(&self) -> &[String] {
        self.container.0.calls()
    }

    fn next_session_id(&mut self) -> String {
        let id = format!("session-{}", self.next_id);
        self.next_id += 1;
        id
    }

    /// Build the shell command string passed to `Container::spawn`.
    ///
    /// - Coverage / unit-test build: returns `argv.join(" ")` (the fake container
    ///   ignores the command body; what matters is that spawn is called).
    /// - Production build: if a real container is active, returns the full
    ///   FIFO-wired tmux invocation per the realio.md spec:
    ///   `cd <project> && cat in.fifo | claude <args> > out.fifo 2>err.log`
    ///   Otherwise falls back to `argv.join(" ")`.
    ///
    /// NOTE ON `cat in.fifo |` INSTEAD OF `< in.fifo` (found via live i03 testing
    /// against real tmux 3.1c + claude 2.1.198): redirecting claude's stdin
    /// directly from a *named* FIFO (`claude ... < in.fifo`) makes claude exit 0
    /// with **zero** output and no session transcript whenever the process has a
    /// controlling terminal — which every tmux pane has, even fully detached
    /// (`tmux new-session -d`) ones with no client attached. The exact same
    /// invocation works perfectly when claude's stdin is instead an *anonymous*
    /// pipe (confirmed: `echo ... | claude ...` inside the same tmux pane works;
    /// only `claude ... < named_fifo` inside tmux is broken). Piping the FIFO
    /// through `cat` converts claude's stdin fd from a named pipe to an
    /// anonymous one, sidestepping the bug entirely — verified against the real
    /// binaries, not simulated.  Ruled out during diagnosis: PATH, HOME,
    /// CLAUDECODE/CLAUDE_CODE_* env leakage, and controlling-terminal
    /// detachment via `setsid` (none of these changed the outcome). See
    /// docs/specs/realio.md for the updated shape.
    #[cfg(coverage)]
    fn build_spawn_cmd(
        &self,
        _container_name: &str,
        _project: &str,
        argv: &[String],
        _setup: Option<&str>,
        _env: &[(String, String)],
    ) -> String {
        argv.join(" ")
    }

    #[cfg(not(coverage))]
    fn build_spawn_cmd(
        &self,
        container_name: &str,
        project: &str,
        argv: &[String],
        setup: Option<&str>,
        env: &[(String, String)],
    ) -> String {
        if self.real_container.0.is_some() {
            // Platform-specific command construction.
            //
            // On Unix: build a FIFO-wired shell invocation.  The session dir
            // contains in.fifo and out.fifo; the tmux shell does the redirection.
            //
            // On Windows: the marsad-winshim.exe process receives the claude
            // command via `--cmd` and handles stdin/stdout redirection itself via
            // STARTUPINFO handles.  We pass only the raw claude invocation — no
            // shell, no FIFO paths.  The shim wraps it in `cmd.exe /C <cmd>`.
            #[cfg(unix)]
            {
                let session_dir = std::path::PathBuf::from(&self.config.data_root)
                    .join("sessions")
                    .join(container_name);
                let in_fifo = session_dir.join("in.fifo");
                let out_fifo = session_dir.join("out.fifo");
                let err_log = session_dir.join("err.log");
                // argv[0] is the configured claude bin; the rest are its arguments.
                // We build (with optional setup + env exports prepended):
                //   <setup>
                //   export 'K'='V'
                //   cd <project> && cat in.fifo | <bin> <args> > out.fifo 2>err.log
                //
                // `cat in.fifo |` (rather than `< in.fifo`) is load-bearing — see
                // the NOTE on the doc comment above `build_spawn_cmd`.
                let bin = argv.first().map(String::as_str).unwrap_or("claude");
                let claude_args = &argv[1..];
                let mut prefix = String::new();
                if let Some(s) = setup.filter(|s| !s.is_empty()) {
                    prefix.push_str(s);
                    prefix.push('\n');
                }
                for (k, v) in env {
                    prefix.push_str(&format!("export {}={}\n", shell_quote(k), shell_quote(v)));
                }
                format!(
                    "{}cd {} && cat {} | {} {} > {} 2>{}",
                    prefix,
                    shell_quote(project),
                    shell_quote(&in_fifo.display().to_string()),
                    shell_quote(bin),
                    claude_args
                        .iter()
                        .map(|a| shell_quote(a))
                        .collect::<Vec<_>>()
                        .join(" "),
                    out_fifo.display(),
                    err_log.display(),
                )
            }

            #[cfg(windows)]
            {
                // The shim handles I/O redirection.  We pass the raw claude
                // invocation so the shim can exec it via `cmd.exe /C <cmd>`.
                // argv[0] is "claude"; the rest are its arguments.
                // We build: cd /d "<project>" && claude <args>
                // (/d switches the drive on Windows cmd.exe).
                //
                // QUOTING: Windows paths must NOT use Rust's {:?} Debug format —
                // that escapes backslashes (e.g. "C:\foo" becomes "C:\\foo"),
                // which cmd.exe interprets as literal double-backslashes.
                // Windows paths never contain double-quote characters, so wrapping
                // in double quotes is always safe and produces correct cmd.exe syntax.
                let _ = container_name; // suppress unused-variable on Windows
                                        // Best-effort env + setup prepend for the cmd.exe shim:
                                        //   set K=V& <setup>& cd /d "proj" && <bin> args
                let bin = argv.first().map(String::as_str).unwrap_or("claude");
                let claude_args = &argv[1..];
                let mut prefix = String::new();
                for (k, v) in env {
                    prefix.push_str(&format!("set {k}={v}& "));
                }
                if let Some(s) = setup.filter(|s| !s.is_empty()) {
                    prefix.push_str(&format!("{s}& "));
                }
                format!(
                    "{}cd /d \"{}\" && {} {}",
                    prefix,
                    project,
                    bin,
                    claude_args.join(" ")
                )
            }
        } else {
            argv.join(" ")
        }
    }

    /// Spawn the session container — locally, or on the session's remote host.
    ///
    /// Coverage/test build: always the injected fake (host_target ignored), so the
    /// existing container-call assertions hold and the measured surface is stable.
    #[cfg(coverage)]
    fn spawn_container(
        &mut self,
        container_name: &str,
        spec: &marsad_proto::LaunchSpec,
    ) -> Result<(), AgentError> {
        let argv = claude_argv(spec);
        let cmd = self.build_spawn_cmd(
            container_name,
            &spec.project,
            &argv,
            spec.setup.as_deref(),
            &spec.env,
        );
        self.active_container()
            .spawn(container_name, &cmd)
            .map_err(|e| AgentError::Spawn(format!("{e}")))
    }

    /// Real-IO build: local hosts use the platform container; remote hosts
    /// (`exec_prefix_for` → `Some`) launch a DURABLE detached session on the
    /// remote: `mkfifo in.fifo`, keepalive holder (keeps write end open), and
    /// `setsid` + `nohup` claude reading from the FIFO and appending to `out.log`.
    /// The FIFO + log survive agent restarts — the reader thread connects by
    /// spawning a `cat >> in.fifo` writer child and a `tail -n0 -f out.log` reader
    /// child over a fresh ssh/wsl connection.
    #[cfg(not(coverage))]
    fn spawn_container(
        &mut self,
        container_name: &str,
        spec: &marsad_proto::LaunchSpec,
    ) -> Result<(), AgentError> {
        match exec_prefix_for(spec.host_target.as_deref()) {
            None => {
                let argv = claude_argv(spec);
                let cmd = self.build_spawn_cmd(
                    container_name,
                    &spec.project,
                    &argv,
                    spec.setup.as_deref(),
                    &spec.env,
                );
                self.active_container()
                    .spawn(container_name, &cmd)
                    .map_err(|e| AgentError::Spawn(format!("{e}")))
            }
            Some(_) => {
                // Extract the session_id from the container name ("marsad-<sid>").
                let sid = container_name
                    .strip_prefix("marsad-")
                    .unwrap_or(container_name);
                let run_cmd = self
                    .session_run_cmds
                    .0
                    .lock()
                    .unwrap()
                    .get(sid)
                    .cloned()
                    .unwrap_or_default();
                let script = remote_durable_launch_script(sid, &run_cmd);
                let host = spec.host_target.as_deref().unwrap_or("");
                eprintln!(
                    "marsad-agent: remote session {container_name} on {host:?} — launching durable session (FIFO + setsid)"
                );
                // run_remote_script returns None on connection failure (spawn error).
                run_remote_script(host, &script).ok_or_else(|| {
                    AgentError::Spawn(format!(
                        "remote durable launch failed for {container_name} on {host:?}"
                    ))
                })?;
                Ok(())
            }
        }
    }

    /// Look up a stored session's resolved remote target (for reconcile/kill of
    /// remote containers). Real-IO build only.
    #[cfg(not(coverage))]
    fn session_host_target(&mut self, session_id: &str) -> Option<String> {
        self.store
            .0
            .restore()
            .ok()?
            .sessions
            .into_iter()
            .find(|s| s.id == session_id)
            .and_then(|s| s.host_target)
    }

    /// Look up a stored session's `(project, claude_uuid)` so the reader thread
    /// can fetch its remote transcript `.jsonl`. The uuid is `None` for a
    /// `--continue` session (no specific id) — the fetch then picks the most
    /// recent conversation in the project dir. Real-IO build only.
    #[cfg(not(coverage))]
    fn session_transcript_project_uuid(
        &mut self,
        session_id: &str,
    ) -> Option<(String, Option<String>)> {
        self.store
            .0
            .restore()
            .ok()?
            .sessions
            .into_iter()
            .find(|s| s.id == session_id)
            .map(|s| (s.project, s.session_id))
    }

    /// Kill a session's container — locally or on its remote host.
    #[cfg(coverage)]
    fn kill_container(&mut self, _session_id: &str, container_name: &str) {
        let _ = self.active_container().kill(container_name);
    }

    #[cfg(not(coverage))]
    fn kill_container(&mut self, session_id: &str, container_name: &str) {
        match exec_prefix_for(self.session_host_target(session_id).as_deref()) {
            None => {
                let _ = self.active_container().kill(container_name);
            }
            Some(_) => {
                // Durable remote session: the detached claude + keepalive holder
                // live independently of this agent process. Killing them requires
                // running a kill script on the remote host (reads the stored PID
                // files and sends SIGTERM, then removes the session dir).
                // Also drop + reap the local writer child (the `cat >> in.fifo`
                // connection) if it's still in the registry — its EOF on the remote
                // doesn't kill claude (the keepalive holds the write end open), but
                // we clean it up so the slot is freed.
                let host = self.session_host_target(session_id).unwrap_or_default();
                let script = remote_kill_script(session_id);
                if let Some(out) = run_remote_script(&host, &script) {
                    eprintln!(
                        "marsad-agent: session {session_id} remote kill on {host:?}: {}",
                        out.trim()
                    );
                } else {
                    eprintln!(
                        "marsad-agent: session {session_id} remote kill script failed (host unreachable?)"
                    );
                }
                // Reap the local writer child.
                let child = {
                    let mut children = self.session_children.0.lock().unwrap();
                    children.remove(session_id)
                };
                if let Some(mut child) = child {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }

    /// Launch a new session via the injected container.
    pub fn launch(&mut self, spec: marsad_proto::LaunchSpec) -> Result<String, AgentError> {
        let session_id = self.next_session_id();
        self.launch_with_id(session_id, spec)
    }

    /// Launch using a caller-supplied session id.
    ///
    /// The GUI/Core generates the session id up front (so it can navigate to the
    /// session immediately) and passes it in the LaunchSession message; the agent
    /// MUST reuse that id so the `AgentEvent`s it broadcasts are tagged with the
    /// id Core registered — otherwise Core drops them as an unknown session and
    /// nothing shows up in the UI. `launch` (above) is the no-id variant that
    /// generates one (used by tests and resume).
    pub fn launch_with_id(
        &mut self,
        session_id: String,
        spec: marsad_proto::LaunchSpec,
    ) -> Result<String, AgentError> {
        let container_name = format!("marsad-{session_id}");

        // For a REMOTE session, compute the claude run command now — this is the
        // only point with the full LaunchSpec (posture + mode) in hand.
        // We stash it BEFORE spawn_container so the durable launch script (which
        // spawn_container runs via run_remote_script) can embed it.
        // The serve loop also pops it by session_id to pass to the reader thread.
        // Local sessions stash nothing.
        #[cfg(not(coverage))]
        {
            if exec_prefix_for(spec.host_target.as_deref()).is_some() {
                let run_cmd = build_remote_spawn_cmd(
                    &spec.project,
                    &claude_argv(&spec),
                    spec.setup.as_deref(),
                    &spec.env,
                );
                self.session_run_cmds
                    .0
                    .lock()
                    .unwrap()
                    .insert(session_id.clone(), run_cmd);
            }
        }

        self.spawn_container(&container_name, &spec)?;

        // Use a custom name when provided (non-empty after trimming); otherwise
        // fall back to the session id (the default the agent has always used).
        let persisted_name = spec
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| session_id.clone());

        // For a Resume(uuid) launch, persist the claude session uuid up front:
        // claude `--resume <uuid>` IS that conversation, and the transcript
        // backfill keys off this to fetch the RIGHT remote `.jsonl` (otherwise it
        // falls back to "most recent", which on some hosts finds nothing → the
        // "resumed but no transcript" bug). Fresh/Continue stay None (the real id
        // arrives later via system/init).
        let claude_uuid = match &spec.mode {
            marsad_proto::LaunchMode::Resume(uuid) if !uuid.is_empty() => Some(uuid.clone()),
            _ => None,
        };
        let persisted = marsad_store::PersistedSession {
            id: session_id.clone(),
            session_id: claude_uuid,
            name: persisted_name,
            host_id: spec.host_id.clone(),
            host_target: spec.host_target.clone(),
            claude_bin: spec.claude_bin.clone(),
            setup: spec.setup.clone(),
            env: spec.env.clone(),
            project: spec.project.clone(),
            branch: "main".to_string(),
            model: spec.model.clone(),
            last_phase: "Starting".to_string(),
            posture: serde_json::to_value(&spec.posture)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string)),
            disallowed_tools: spec.disallowed_tools.clone(),
        };
        self.store
            .0
            .upsert_session(&persisted)
            .map_err(|e| AgentError::Lifecycle(format!("persist session: {e:?}")))?;

        self.sessions.insert(
            session_id.clone(),
            SessionRecord {
                marsad_id: session_id.clone(),
            },
        );
        Ok(session_id)
    }

    /// Decode one inbound IPC line and return outbound IPC lines.
    pub fn handle_ipc(&mut self, line: &str) -> Result<Vec<String>, AgentError> {
        if line.is_empty() {
            return Err(AgentError::Protocol("empty IPC line".to_string()));
        }
        let msg = marsad_proto::parse_ipc(line).map_err(|e| AgentError::Protocol(e.to_string()))?;

        match msg {
            marsad_proto::IpcMsg::LaunchSession {
                session_id: wire_session_id,
                project,
                host_id,
                host_target,
                claude_bin,
                setup,
                env,
                model,
                posture,
                first_prompt,
                continue_,
                resume_session_id,
                name,
                disallowed_tools,
                ..
            } => {
                // Derive the launch mode from the wire fields (§5 item 3:
                // replaces the old `parse_launch_mode_value`; mirrors
                // `IpcMsg::launch_mode()`'s precedence, which is exhaustively
                // pinned by marsad-proto's own golden test suite: non-empty
                // `resume_session_id` wins, then `continue_ == true`, else
                // Fresh — empty-string resume is treated as absent).
                let mode = match resume_session_id.filter(|id| !id.is_empty()) {
                    Some(id) => marsad_proto::LaunchMode::Resume(id),
                    None if continue_ == Some(true) => marsad_proto::LaunchMode::Continue,
                    None => marsad_proto::LaunchMode::Fresh,
                };
                self.handle_launch_session(
                    wire_session_id,
                    project,
                    host_id,
                    host_target,
                    claude_bin,
                    setup,
                    env,
                    model,
                    posture,
                    first_prompt,
                    mode,
                    name,
                    disallowed_tools,
                )
            }
            marsad_proto::IpcMsg::ResumeSession {
                session_id,
                first_prompt,
            } => self.handle_resume_session(session_id, first_prompt),
            marsad_proto::IpcMsg::AttachSession { session_id } => {
                self.handle_attach_session(&session_id)
            }
            marsad_proto::IpcMsg::StopSession { session_id } => {
                self.handle_stop_session(session_id)
            }
            marsad_proto::IpcMsg::SendPrompt { session_id, text } => {
                self.handle_send_prompt(&session_id, &text)
            }
            marsad_proto::IpcMsg::Interrupt { session_id } => self.handle_interrupt(&session_id),
            marsad_proto::IpcMsg::AnswerGate {
                session_id,
                allow,
                deny_reason,
                ..
            } => self.handle_answer_gate(&session_id, allow, deny_reason),
            marsad_proto::IpcMsg::AnswerQuestion {
                session_id,
                answers,
                ..
            } => self.handle_answer_question(&session_id, answers),
            marsad_proto::IpcMsg::Subscribe { .. } => self.handle_subscribe(),
            marsad_proto::IpcMsg::Unsubscribe { .. } => Ok(vec![]),
            // NEW explicit arms (typed-ipc.md §5 item 3): the agent has no real
            // handler for either today — same observable reply as before typing
            // (SlashCmd still errors; SetWorkspace is a silent advisory no-op).
            marsad_proto::IpcMsg::SlashCmd { .. } => {
                Err(AgentError::Protocol("SlashCmd not implemented".to_string()))
            }
            marsad_proto::IpcMsg::SetWorkspace { .. } => Ok(vec![]),
            // Every other tag is an agent→GUI/core message (ack/broadcast) that
            // has no business arriving inbound here — same observable Protocol
            // error class as an unrecognized type.
            other => Err(AgentError::Protocol(format!(
                "misdirected IPC message on GUI channel: {other:?}"
            ))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_launch_session(
        &mut self,
        wire_session_id: Option<String>,
        project: String,
        host_id: Option<String>,
        host_target: Option<String>,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
        model: String,
        posture: marsad_proto::Posture,
        first_prompt: Option<String>,
        mode: marsad_proto::LaunchMode,
        name: Option<String>,
        disallowed_tools: Vec<String>,
    ) -> Result<Vec<String>, AgentError> {
        let spec = marsad_proto::LaunchSpec {
            project,
            host_id: host_id.unwrap_or_else(|| "local".to_string()),
            model,
            posture,
            first_prompt,
            // Fresh / continue / resume-by-id, from optional IPC fields.
            mode,
            host_target,
            claude_bin,
            setup,
            env,
            name,
            disallowed_tools,
        };
        // Reuse the caller's session id when supplied (Core generates it so it can
        // navigate immediately and recognise the session's events); otherwise
        // generate one (e.g. a raw client that omits it).
        let session_id = match wire_session_id.filter(|s| !s.is_empty()) {
            Some(id) => self.launch_with_id(id, spec)?,
            None => self.launch(spec)?,
        };
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionLaunched { session_id });
        Ok(vec![event])
    }

    /// Resume a Marsad-tracked session by its Marsad id.
    ///
    /// Looks the session up in the persistence store, then launches a fresh
    /// container that reattaches to the underlying claude conversation:
    /// `claude --resume <claude_session_id>` when that id is known, else
    /// `claude --continue` (most recent conversation in the project dir).
    /// An optional `first_prompt` is delivered after reconnect.
    fn handle_resume_session(
        &mut self,
        session_id: String,
        first_prompt: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        // Look the session up in the store to recover its project + model.
        let restored = self
            .store
            .0
            .restore()
            .map_err(|e| AgentError::Lifecycle(format!("restore for resume: {e:?}")))?;
        let persisted = restored
            .sessions
            .into_iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| {
                AgentError::Protocol(format!("ResumeSession: unknown session {session_id}"))
            })?;

        // Resume by claude session id when we have one; otherwise continue the
        // most recent conversation in the project directory.
        let mode = match persisted.session_id.as_deref() {
            Some(uuid) if !uuid.is_empty() => marsad_proto::LaunchMode::Resume(uuid.to_string()),
            _ => marsad_proto::LaunchMode::Continue,
        };

        // Recover any custom name: PersistedSession.name is the marsad id when no
        // custom name was set (agent default), so treat it as None in that case.
        let persisted_name = if persisted.name != persisted.id {
            Some(persisted.name.clone())
        } else {
            None
        };
        let spec = marsad_proto::LaunchSpec {
            project: persisted.project,
            host_id: persisted.host_id,
            model: persisted.model,
            posture: persisted
                .posture
                .as_deref()
                .and_then(|s| {
                    serde_json::from_value::<marsad_proto::Posture>(serde_json::Value::String(
                        s.to_string(),
                    ))
                    .ok()
                })
                .unwrap_or(marsad_proto::Posture::Ask),
            first_prompt,
            mode,
            host_target: persisted.host_target,
            claude_bin: persisted.claude_bin,
            setup: persisted.setup,
            env: persisted.env,
            name: persisted_name,
            disallowed_tools: persisted.disallowed_tools,
        };
        // Reuse the SAME marsad id so the resumed session re-attaches to the
        // existing UI row. Using a fresh id (the old bug) meant the relaunched
        // session's stream events were tagged with an id core never tracked, so
        // resume silently did nothing on the dashboard.
        let new_id = self.launch_with_id(session_id.clone(), spec)?;
        let event =
            marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionResumed { session_id: new_id });
        Ok(vec![event])
    }

    fn handle_attach_session(&mut self, session_id: &str) -> Result<Vec<String>, AgentError> {
        if !self.sessions.contains_key(session_id) {
            return Err(AgentError::Protocol(format!(
                "AttachSession: unknown session {session_id}"
            )));
        }
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionAttached {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_stop_session(&mut self, session_id: String) -> Result<Vec<String>, AgentError> {
        // Kill the container and drop the live tracking entry. We do NOT require the
        // session to be in the live map: after an agent restart a session is persisted
        // but not yet re-tracked (the real cause of "stop did nothing" — the old code
        // returned an error and killed nothing). `kill_container` resolves the remote
        // host from the store, so the kill works either way. The persisted record is
        // KEPT so the session remains resumable.
        let container_name = format!("marsad-{session_id}");
        self.kill_container(&session_id, &container_name);
        self.sessions.remove(&session_id);
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::SessionStopped { session_id });
        Ok(vec![event])
    }

    fn handle_send_prompt(
        &mut self,
        session_id: &str,
        text: &str,
    ) -> Result<Vec<String>, AgentError> {
        if !self.sessions.contains_key(session_id) {
            return Err(AgentError::Protocol(format!(
                "SendPrompt: unknown session {session_id}"
            )));
        }

        // In production builds, forward the prompt to the session's in.fifo.
        // The writer is registered when the FIFO thread connects after launch.
        // Under cfg(coverage) this block is absent — the fake path returns Ok
        // without IO, which is correct for unit tests.
        #[cfg(not(coverage))]
        {
            use marsad_proto::{encode_user_input, ContentBlock, UserInput};
            use std::io::Write;
            let encoded = encode_user_input(&UserInput {
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
            });
            let writers = self.session_writers.0.lock().unwrap();
            if let Some(shared) = writers.get(session_id) {
                let mut writer = shared.lock().unwrap();
                match writer
                    .write_all(encoded.as_bytes())
                    .and_then(|_| writer.write_all(b"\n"))
                    .and_then(|_| writer.flush())
                {
                    Ok(()) => eprintln!(
                        "marsad-agent: session {session_id} send_prompt delivered ({} chars) to claude stdin",
                        text.len()
                    ),
                    Err(e) => eprintln!(
                        "marsad-agent: session {session_id} send_prompt WRITE FAILED: {e}"
                    ),
                }
            } else {
                // No registered writer — the prompt is dropped on the floor and
                // the UI shows nothing. Surface it instead of failing silently.
                eprintln!(
                    "marsad-agent: session {session_id} send_prompt DROPPED — no writer registered (session not connected?)"
                );
            }
        }
        // Suppress unused-variable warning in coverage builds.
        #[cfg(coverage)]
        let _ = text;

        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::PromptSent {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_interrupt(&mut self, session_id: &str) -> Result<Vec<String>, AgentError> {
        // Write an `interrupt` control_request to claude's stdin (via the session
        // writer) to stop the current turn. claude acks + ends the turn with
        // `result: error_during_execution`. Mirrors handle_answer_gate's writer
        // use. Under cfg(coverage) the IO block is absent (fake path = no-op ack).
        #[cfg(not(coverage))]
        {
            use std::io::Write;
            let encoded = marsad_proto::encode_control_request_interrupt("req_interrupt");
            let writers = self.session_writers.0.lock().unwrap();
            if let Some(shared) = writers.get(session_id) {
                let mut writer = shared.lock().unwrap();
                let _ = writer
                    .write_all(encoded.as_bytes())
                    .and_then(|_| writer.write_all(b"\n"))
                    .and_then(|_| writer.flush());
                eprintln!("marsad-agent: session {session_id} interrupt sent to claude");
            } else {
                eprintln!(
                    "marsad-agent: session {session_id} interrupt DROPPED — no writer registered"
                );
            }
        }
        // If the session is gone, interrupt is a benign no-op.
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::Interrupted {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_answer_gate(
        &mut self,
        session_id: &str,
        allow: bool,
        deny_reason: Option<String>,
    ) -> Result<Vec<String>, AgentError> {
        // In production builds, translate the answer into a real
        // `control_response` and write it to the session's in.fifo so claude
        // unblocks the gated tool. The `allow` decision pairs the original
        // tool input (captured when the `can_use_tool` arrived) back as
        // `updatedInput` — claude USES that value, so a plain approval must
        // echo the original input verbatim.
        //
        // Under cfg(coverage) this block is absent: the fake path returns the
        // GateAnswered ack without IO, which is what the unit tests assert.
        #[cfg(not(coverage))]
        {
            use std::io::Write;

            // Pull (and clear) the pending gate for this session.
            let pending = {
                let mut gates = self.pending_gates.0.lock().unwrap();
                gates.remove(session_id)
            };

            if let Some(gate) = pending {
                // An MCP elicitation is answered with `{action: accept|decline}`,
                // NOT the can_use_tool `{behavior, updatedInput}` shape — sending
                // the wrong shape leaves claude blocked (the MCP host can't read
                // the result). Branch on the kind captured when the gate arrived.
                let encoded = if gate.elicitation {
                    marsad_proto::encode_control_response_elicitation(&gate.request_id, allow)
                } else {
                    let resp = marsad_proto::ControlResponse {
                        request_id: gate.request_id,
                        allow,
                        deny_reason,
                        updated_input: if allow { Some(gate.input) } else { None },
                    };
                    marsad_proto::encode_control_response(&resp)
                };
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    let _ = writer.write_all(encoded.as_bytes());
                    let _ = writer.write_all(b"\n");
                    let _ = writer.flush();
                }
            }
        }
        // Suppress unused-variable warnings in coverage builds (the fake path
        // never reads `allow`/`deny_reason` — no writer IO to translate them into).
        #[cfg(coverage)]
        let (_, _) = (allow, deny_reason);

        // If no pending gate, this is a benign no-op (the tests assert non-Protocol-error).
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::GateAnswered {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_answer_question(
        &mut self,
        session_id: &str,
        answers: serde_json::Value,
    ) -> Result<Vec<String>, AgentError> {
        // In production builds: pull (and clear) the pending gate for this session,
        // then build and write the AskUserQuestion control_response to claude's stdin.
        // Mirrors handle_answer_gate's writer-locking + write/flush pattern.
        #[cfg(not(coverage))]
        {
            use std::io::Write;

            let pending = {
                let mut gates = self.pending_gates.0.lock().unwrap();
                gates.remove(session_id)
            };

            if let Some(gate) = pending {
                let encoded = marsad_proto::encode_control_response_question(
                    &gate.request_id,
                    &gate.input,
                    &answers,
                );
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    let _ = writer.write_all(encoded.as_bytes());
                    let _ = writer.write_all(b"\n");
                    let _ = writer.flush();
                }
            }
        }

        // Coverage builds: no IO — just return the ack.
        #[cfg(coverage)]
        let _ = answers;
        let event = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::QuestionAnswered {
            session_id: session_id.to_string(),
        });
        Ok(vec![event])
    }

    fn handle_subscribe(&mut self) -> Result<Vec<String>, AgentError> {
        let subscriber_id = format!("sub-{}", self.next_id);
        self.next_id += 1;
        let ack = marsad_proto::encode_ipc(&marsad_proto::IpcMsg::Subscribed { subscriber_id });
        Ok(vec![ack])
    }

    /// Ingest a Claude Code hook payload.
    pub fn handle_hook(&mut self, payload: &str) -> Result<(), AgentError> {
        let value: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| AgentError::Protocol(format!("bad hook json: {e}")))?;

        let hook_type = value
            .as_object()
            .and_then(|obj| obj.get("hook"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::Protocol("missing hook discriminant".to_string()))?;

        let hook_event = parse_hook_event(hook_type, &value)?;

        if let marsad_events::HookEvent::Notification { ref message } = hook_event {
            let note = marsad_notify::Notification {
                kind: marsad_notify::NotifKind::Needs,
                title: "Notification".to_string(),
                body: message.clone(),
                host: "local".to_string(),
                priority: marsad_notify::Priority::High,
                actions: vec![],
                session_id: String::new(),
                play_sound: false,
            };
            let _ = self.notifier.0.deliver(&note);
        }

        Ok(())
    }

    /// On daemon startup: read persisted sessions and reconcile each one.
    ///
    /// - Container alive → Adopt (no spawn, no duplication).
    /// - Container dead + session_id → Resume (spawn `claude --resume <id>`).
    /// - Container dead + no session_id → MarkStale.
    pub fn reconcile_all(&mut self) -> Vec<marsad_lifecycle::SessionRuntime> {
        let restored = self
            .store
            .0
            .restore()
            .map(|r| r.sessions)
            .unwrap_or_default();

        let mut runtimes = Vec::new();

        for session in restored {
            let container_name = format!("marsad-{}", session.id);
            let known = Persisted {
                session_id: session.session_id.clone(),
                container_name: container_name.clone(),
                project: session.project.clone(),
                model: session.model.clone(),
            };

            // Reconcile against the right container — local, or the session's
            // remote host (so a remote tmux that outlived the app is adopted).
            let runtime = self.reconcile_one(&known, session.host_target.as_deref());

            runtimes.push(runtime);
        }

        runtimes
    }

    /// Reconcile a single known session against the appropriate container.
    ///
    /// Coverage/test build: always the injected fake (host_target ignored).
    #[cfg(coverage)]
    fn reconcile_one(
        &mut self,
        known: &Persisted,
        _host_target: Option<&str>,
    ) -> marsad_lifecycle::SessionRuntime {
        use marsad_lifecycle::{Action, RuntimePhase, SessionRuntime};
        marsad_lifecycle::reconcile(known, self.active_container()).unwrap_or(SessionRuntime {
            action: Action::MarkStale,
            phase: Some(RuntimePhase::Stale),
        })
    }

    /// Real-IO build: local sessions use the platform container; remote sessions
    /// reconcile against `RemoteTmuxContainer` so a remote tmux that survived the
    /// app restart is adopted rather than re-spawned.
    #[cfg(not(coverage))]
    fn reconcile_one(
        &mut self,
        known: &Persisted,
        host_target: Option<&str>,
    ) -> marsad_lifecycle::SessionRuntime {
        use marsad_lifecycle::{Action, RuntimePhase, SessionRuntime};
        let fallback = SessionRuntime {
            action: Action::MarkStale,
            phase: Some(RuntimePhase::Stale),
        };
        match exec_prefix_for(host_target) {
            None => marsad_lifecycle::reconcile(known, self.active_container()).unwrap_or(fallback),
            Some(prefix) => {
                let mut c = marsad_lifecycle::RemoteTmuxContainer::new(
                    prefix,
                    REMOTE_RUNTIME_DIR.to_string(),
                );
                marsad_lifecycle::reconcile(known, &mut c).unwrap_or(fallback)
            }
        }
    }

    /// Blocking event loop — real socket/pipe IO, excluded from the coverage
    /// build because it requires a live socket/pipe and loops forever.
    /// Called only from `src/main.rs`; since the binary target is not compiled
    /// during `cargo llvm-cov` (autobins = false), this function is absent from
    /// the coverage surface entirely.
    ///
    /// # Concurrency model
    ///
    /// `Agent` owns non-`Send` trait objects, so it cannot be wrapped in
    /// `Arc<Mutex<Agent>>` across threads.  Instead we use a **command channel**
    /// pattern:
    ///
    /// - This serve thread is the single owner of `Agent` and processes IPC
    ///   commands sequentially via `mpsc::Receiver<AgentCmd>`.
    /// - One short-lived reader thread is spawned per accepted client socket.
    ///   It reads inbound lines and forwards them as `AgentCmd` values on the
    ///   shared command sender, blocking on a one-shot reply channel for the
    ///   response.  While waiting for the reply it holds no locks and does no
    ///   IO on the agent.
    /// - Broadcast messages (AgentEvent lines from session FIFO readers) bypass
    ///   the agent thread and go directly to each client's `mpsc::Sender<String>`
    ///   in the subscriber list `Arc<Mutex<Vec<mpsc::Sender<String>>>>`.
    ///
    /// Deadlock analysis:
    ///  - The agent thread holds no subscriber-list lock when processing a command.
    ///  - Session FIFO reader threads lock only the subscriber list (briefly) per
    ///    line — never the agent command channel.
    ///  - Client reader threads lock only the subscriber list on disconnect prune,
    ///    which is also brief.
    ///  - No lock is held while blocking on socket or FIFO reads.
    #[cfg(not(coverage))]
    pub fn serve(mut self) -> Result<(), AgentError> {
        use marsad_transport::LocalIpcListener;
        use std::sync::{mpsc, Arc, Mutex};

        // ── Bind ──────────────────────────────────────────────────────────────
        let socket_path = match &self.config.listen {
            ListenSpec::UnixSocket(path) => path.clone(),
            other => {
                return Err(AgentError::Listen(format!(
                    "unsupported listen spec: {other:?}"
                )));
            }
        };

        let listener = LocalIpcListener::bind(&socket_path)
            .map_err(|e| AgentError::Listen(format!("bind {socket_path}: {e}")))?;

        eprintln!("marsad-agent: listening on {socket_path}");

        // ── Command channel ────────────────────────────────────────────────
        // Each `AgentCmd` carries one inbound IPC line plus a one-shot reply
        // sender.  The serve thread processes commands sequentially and sends
        // the reply back through the one-shot channel.
        //
        // `AgentReply` carries either the list of outbound lines (success) or
        // an error string, plus an optional session_id that signals the serve
        // thread to open a FIFO reader for that session, plus an optional
        // first_prompt to deliver to claude's stdin immediately after launch.
        struct AgentCmd {
            line: String,
            reply_tx: mpsc::SyncSender<AgentReply>,
        }
        struct AgentReply {
            out_lines: Result<Vec<String>, String>,
            /// Set when a LaunchSession/ResumeSession produced a session_id and
            /// a new FIFO reader should be started.
            new_session_id: Option<String>,
            /// The first prompt to write to in.fifo right after the FIFO
            /// connects, so claude receives its initial task without hanging.
            first_prompt: Option<String>,
            /// The session's resolved remote target (`None` = local). Tells the
            /// reader thread whether to open local FIFOs or a remote transport.
            host_target: Option<String>,
            /// For a REMOTE session: the run-command body (setup + env + `cd …
            /// && exec <claude> <args>`) to run as the single piped child. `None`
            /// for local sessions (which use the platform container).
            run_command: Option<String>,
            /// For a REMOTE resume/continue: `(project, claude_uuid)` so the
            /// reader thread fetches the remote `.jsonl` over the session's own
            /// connection and broadcasts a TranscriptBackfill. `None` for local
            /// sessions (core reads their JSONL directly) and fresh sessions
            /// (no prior history to fetch).
            transcript_hint: Option<(String, Option<String>)>,
        }

        // The command sender is cloned into each client thread.
        let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCmd>();

        // ── Broadcast list ─────────────────────────────────────────────────
        // Arc<Mutex<Vec<Sender>>> — every connected client has one slot.
        // Session FIFO reader threads and the client reader thread on disconnect
        // both hold a clone of this Arc.
        let subscribers: Arc<Mutex<Vec<mpsc::Sender<String>>>> = Arc::new(Mutex::new(Vec::new()));

        // ── Session writers registry ───────────────────────────────────────
        // Shared across the serve thread (handle_send_prompt via self) and the
        // FIFO threads (which register the writer after connect).
        // `session_writers_arc` is a clone of the same Arc stored in
        // `self.session_writers.0`, so handle_send_prompt can look up writers
        // by session_id via `self.session_writers.0` while FIFO threads
        // insert via `session_writers_for_fifo`.
        let session_writers_for_fifo = Arc::clone(&self.session_writers.0);

        // ── Session children registry ──────────────────────────────────────
        // Shared across the serve thread (kill_container via self) and the
        // reader threads (which insert the spawned remote child after spawn).
        let session_children_for_reader = Arc::clone(&self.session_children.0);

        // ── Pending-gate registry ──────────────────────────────────────────
        // Shared across the serve thread (handle_answer_gate via self) and the
        // FIFO threads (which insert when claude emits a `can_use_tool`).
        let pending_gates_for_fifo = Arc::clone(&self.pending_gates.0);

        let data_root = self.config.data_root.clone();

        // ── Reconnect sessions that survived an agent restart ──────────────────
        // The agent is a detached daemon; if it died (e.g. killed on reinstall) but
        // a session container is still alive, re-attach a reader to each live
        // session and re-track it so prompt/stop work and the conversation CONTINUES.
        // `first_prompt: None` and `send_init: false` — the claude is already
        // running and initialized; we only re-attach to its stream.
        //
        // LOCAL sessions: reconcile via tmux/shim container as before.
        // REMOTE sessions: durable (FIFO + setsid) — probe liveness via
        // `remote_liveness_script`, then re-attach with writer + tail children.
        {
            use marsad_lifecycle::Action;
            let restored = self
                .store
                .0
                .restore()
                .map(|r| r.sessions)
                .unwrap_or_default();

            // Probe ALL remote sessions' liveness in PARALLEL with a bounded wait,
            // BEFORE touching `self`. The probes are blocking remote calls; doing
            // them sequentially on this (the serve) thread — as the code used to —
            // froze the whole agent if any persisted session lived on a host that
            // blocks (a Duo/MFA host with no live ControlMaster sits at the prompt
            // forever). That manifested as "no sessions start, nothing appears at
            // all": the command loop below never ran. Now each probe runs in its own
            // thread with a hard timeout; any that don't answer in time are treated
            // DEAD and skipped, so startup completes regardless of one bad host.
            let mut live_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            {
                let remote: Vec<_> = restored
                    .iter()
                    .filter(|s| {
                        s.host_target
                            .as_deref()
                            .map(|h| exec_prefix_for(Some(h)).is_some())
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();
                let n = remote.len();
                if n > 0 {
                    let (tx, rx) = mpsc::channel::<(String, bool)>();
                    for s in &remote {
                        let host = s.host_target.clone().unwrap_or_default();
                        let id = s.id.clone();
                        let tx = tx.clone();
                        std::thread::spawn(move || {
                            let live = run_remote_script_bounded(
                                &host,
                                &remote_liveness_script(&id),
                                std::time::Duration::from_secs(6),
                            )
                            .map(|out| out.trim() == "LIVE")
                            .unwrap_or(false);
                            let _ = tx.send((id, live));
                        });
                    }
                    drop(tx);
                    // Each bounded probe self-terminates within ~6s, so the whole
                    // collect finishes in ~6s (parallel) even with many hosts; the
                    // 8s per-recv cap is a belt-and-suspenders backstop.
                    for _ in 0..n {
                        match rx.recv_timeout(std::time::Duration::from_secs(8)) {
                            Ok((id, live)) => {
                                eprintln!(
                                    "marsad-agent: remote session {id} liveness: {}",
                                    if live { "LIVE" } else { "DEAD" }
                                );
                                if live {
                                    live_ids.insert(id);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }

            for s in restored {
                if let Some(host) = s.host_target.as_deref() {
                    if exec_prefix_for(Some(host)).is_some() {
                        // REMOTE session: reconnect if the parallel probe found it live.
                        let live = live_ids.contains(&s.id);
                        if live {
                            self.sessions.insert(
                                s.id.clone(),
                                SessionRecord {
                                    marsad_id: s.id.clone(),
                                },
                            );
                            // On reconnect: no run_command (already running), no
                            // first_prompt (already initialized), no send_init.
                            // transcript_hint: supply for resume so backfill fires.
                            let transcript_hint = s
                                .session_id
                                .as_ref()
                                .filter(|_| true) // always try backfill on reconnect
                                .map(|uuid| (s.project.clone(), Some(uuid.clone())))
                                .or_else(|| Some((s.project.clone(), None)));
                            spawn_session_reader(
                                s.id.clone(),
                                s.host_target.clone(),
                                None,
                                false, // already initialized
                                None,  // run_command: already running
                                transcript_hint,
                                data_root.clone(),
                                Arc::clone(&subscribers),
                                Arc::clone(&self.session_writers.0),
                                Arc::clone(&self.pending_gates.0),
                                Arc::clone(&self.session_children.0),
                            );
                            eprintln!("marsad-agent: reconnected live remote session {}", s.id);
                        }
                        continue;
                    }
                }

                // LOCAL session: reconcile via tmux/shim container.
                let known = Persisted {
                    session_id: s.session_id.clone(),
                    container_name: format!("marsad-{}", s.id),
                    project: s.project.clone(),
                    model: s.model.clone(),
                };
                let action = self.reconcile_one(&known, s.host_target.as_deref()).action;
                eprintln!(
                    "marsad-agent: reconcile {} on {:?} -> {action:?}",
                    s.id, s.host_target
                );
                if matches!(action, Action::Adopt) {
                    self.sessions.insert(
                        s.id.clone(),
                        SessionRecord {
                            marsad_id: s.id.clone(),
                        },
                    );
                    spawn_session_reader(
                        s.id.clone(),
                        s.host_target.clone(),
                        None,
                        false,
                        None,
                        None, // reconnect path is local-only; no remote backfill
                        data_root.clone(),
                        Arc::clone(&subscribers),
                        Arc::clone(&self.session_writers.0),
                        Arc::clone(&self.pending_gates.0),
                        Arc::clone(&self.session_children.0),
                    );
                    eprintln!("marsad-agent: reconnected live session {}", s.id);
                }
            }
        }

        // ── Accept thread ──────────────────────────────────────────────────
        // We spin off the accept loop in a separate thread so that the current
        // thread can run the agent command loop below.  The accept thread
        // never touches the `Agent`.
        let subs_accept = Arc::clone(&subscribers);
        let cmd_tx_accept = cmd_tx.clone();

        std::thread::spawn(move || {
            loop {
                let client = match listener.accept() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("marsad-agent: accept error: {e}");
                        break;
                    }
                };

                // Per-client broadcast channel.
                // bcast_tx_for_reader — given to the reader thread so it can
                //   route ack/reply lines through the writer thread.
                // bcast_tx_for_subs   — pushed into the subscriber list so that
                //   FIFO reader threads can broadcast AgentEvents to this client.
                // Both are Sender clones of the same channel; bcast_rx is drained
                // exclusively by the writer thread.
                let (bcast_tx_for_reader, bcast_rx) = mpsc::channel::<String>();
                let bcast_tx_for_subs = bcast_tx_for_reader.clone();
                {
                    let mut subs = subs_accept.lock().unwrap();
                    subs.push(bcast_tx_for_subs);
                }

                let cmd_tx_client = cmd_tx_accept.clone();
                let subs_client = Arc::clone(&subs_accept);
                let data_root_client = data_root.clone();
                let subs_for_fifo = Arc::clone(&subs_accept);
                let writers_for_fifo = Arc::clone(&session_writers_for_fifo);
                let pending_for_fifo = Arc::clone(&pending_gates_for_fifo);
                let children_for_reader = Arc::clone(&session_children_for_reader);

                // Split the client into independent write and read halves so
                // each thread has exclusive ownership — no Mutex required.
                // All outbound bytes (acks, SessionLaunched, AgentEvents) flow
                // through bcast_tx_for_reader → bcast_rx → writer thread, which
                // is the sole owner of write_half.
                let (write_half, read_half) = match client.into_split() {
                    Some(pair) => pair,
                    None => {
                        eprintln!("marsad-agent: client socket not connected after accept");
                        continue;
                    }
                };

                // Writer thread: sole owner of write_half; drains bcast_rx.
                std::thread::spawn(move || {
                    use std::io::Write;
                    let mut w = write_half;
                    for line in bcast_rx {
                        // Skip empty probe lines used for subscriber-list pruning.
                        if line.is_empty() {
                            continue;
                        }
                        if w.write_all(line.as_bytes()).is_err()
                            || w.write_all(b"\n").is_err()
                            || w.flush().is_err()
                        {
                            break;
                        }
                    }
                });

                // Reader thread: sole owner of read_half; pushes all outbound
                // reply lines via bcast_tx_for_reader so the writer thread
                // serialises them to the socket.
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    let mut r = read_half;
                    loop {
                        let mut raw = String::new();
                        let line_opt = r.read_line(&mut raw);

                        match line_opt {
                            Ok(0) | Err(_) => {
                                // Clean EOF or IO error — disconnect.
                                // Sessions/tmux are intentionally NOT killed.
                                break;
                            }
                            Ok(_) => {
                                // Trim the trailing newline.
                                let line = raw.trim_end_matches('\n').to_string();
                                if line.is_empty() {
                                    continue;
                                }

                                // Send the command to the agent thread and await reply.
                                let (reply_tx, reply_rx) = mpsc::sync_channel::<AgentReply>(0);
                                if cmd_tx_client.send(AgentCmd { line, reply_tx }).is_err() {
                                    // Agent thread gone — daemon shutting down.
                                    break;
                                }
                                let reply = match reply_rx.recv() {
                                    Ok(r) => r,
                                    Err(_) => break,
                                };

                                match reply.out_lines {
                                    Err(msg) => {
                                        let err_line = marsad_proto::encode_ipc(
                                            &marsad_proto::IpcMsg::Error { message: msg },
                                        );
                                        // Route error reply through writer thread.
                                        let _ = bcast_tx_for_reader.send(err_line);
                                    }
                                    Ok(out_lines) => {
                                        // Route ack/reply lines through writer thread.
                                        for out_line in &out_lines {
                                            let _ = bcast_tx_for_reader.send(out_line.clone());
                                        }

                                        // If a new session was launched, start a
                                        // FIFO reader thread for it.
                                        if let Some(session_id) = reply.new_session_id {
                                            spawn_session_reader(
                                                session_id,
                                                reply.host_target,
                                                reply.first_prompt,
                                                true, // launch: send the initialize handshake
                                                reply.run_command,
                                                reply.transcript_hint,
                                                data_root_client.clone(),
                                                Arc::clone(&subs_for_fifo),
                                                Arc::clone(&writers_for_fifo),
                                                Arc::clone(&pending_for_fifo),
                                                Arc::clone(&children_for_reader),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // On disconnect: prune dead subscriber senders.
                    // Sessions/tmux are intentionally left alive (liveness).
                    let mut subs = subs_client.lock().unwrap();
                    subs.retain(|s| s.send(String::new()).is_ok());
                });
            }
        });

        // ── Agent command loop (runs on the serve thread) ──────────────────
        // Owns `self` (the Agent).  Processes one command at a time.
        for cmd in cmd_rx {
            let is_launch = cmd.line.contains("\"LaunchSession\"");
            let is_resume = cmd.line.contains("\"ResumeSession\"");

            // For a launch OR resume, capture the first_prompt BEFORE handle_ipc
            // consumes the JSON — we pass it to the FIFO thread via AgentReply so
            // a resumed/continued session can be handed a new prompt on reconnect.
            let launch_first_prompt: Option<String> = if is_launch || is_resume {
                serde_json::from_str::<serde_json::Value>(&cmd.line)
                    .ok()
                    .and_then(|v| v["first_prompt"].as_str().map(|s| s.to_string()))
            } else {
                None
            };

            let result = self.handle_ipc(&cmd.line);

            let (out_lines, new_session_id, first_prompt) = match result {
                Err(e) => (Err(format!("{e}")), None, None),
                Ok(lines) => {
                    // Extract a new session_id from the response when the
                    // command was a launch or resume — so the accept thread
                    // can open the FIFO reader.
                    let sid = if is_launch || is_resume {
                        lines.first().and_then(|l| {
                            serde_json::from_str::<serde_json::Value>(l)
                                .ok()
                                .and_then(|v| v["session_id"].as_str().map(|s| s.to_string()))
                        })
                    } else {
                        None
                    };

                    // Register the session in the writers registry now that we
                    // know its session_id.  The actual File handle is added by
                    // the FIFO thread once it connects.  We pass first_prompt
                    // via AgentReply so the FIFO thread writes it immediately
                    // after connecting — before the reader loop starts.
                    // The registry entry itself is created by the FIFO thread
                    // once it connects, using the Arc clone passed to it.
                    let prompt = if sid.is_some() {
                        launch_first_prompt
                    } else {
                        None
                    };
                    (Ok(lines), sid, prompt)
                }
            };

            // Resolve the session's remote target (if any) so the reader thread
            // picks the right transport. The session was just persisted by
            // launch_with_id, so the store has its host_target.
            let host_target = new_session_id
                .as_deref()
                .and_then(|s| self.session_host_target(s));

            // Pop the remote run command stashed by launch_with_id (None for a
            // local session). The reader thread spawns it as the single piped
            // child. Serve processes commands sequentially, so this never races.
            let run_command = new_session_id
                .as_deref()
                .and_then(|s| self.session_run_cmds.0.lock().unwrap().remove(s));

            // For a REMOTE resume/continue, tell the reader thread to backfill the
            // transcript from the remote host. `wants_history` is true for a
            // ResumeSession IPC, or a LaunchSession carrying resume_session_id /
            // continue (NOT a fresh launch — that has no prior conversation).
            let wants_history = is_resume
                || serde_json::from_str::<serde_json::Value>(&cmd.line)
                    .ok()
                    .map(|v| {
                        v["resume_session_id"]
                            .as_str()
                            .map(|s| !s.is_empty())
                            .unwrap_or(false)
                            || v["continue"].as_bool().unwrap_or(false)
                    })
                    .unwrap_or(false);
            let transcript_hint = if wants_history && host_target.is_some() {
                new_session_id
                    .as_deref()
                    .and_then(|s| self.session_transcript_project_uuid(s))
            } else {
                None
            };

            // Send reply; if the client thread is gone, ignore the error.
            let _ = cmd.reply_tx.send(AgentReply {
                out_lines,
                new_session_id,
                first_prompt,
                host_target,
                run_command,
                transcript_hint,
            });
        }

        Ok(())
    }
}

// ── shell_quote ───────────────────────────────────────────────────────────────

/// Single-quote a string for POSIX shell.  Replaces `'` with `'\''`.
/// Gated out of the coverage build (only used by `build_spawn_cmd` in the
/// real-io path) and Windows-only builds (where the shim uses cmd.exe quoting).
#[cfg(all(unix, not(coverage)))]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ── remote-host execution helpers ───────────────────────────────────────────────

/// Remote home-relative runtime dir where a remote host keeps Marsad session
/// state (the FIFOs). The remote POSIX shell expands `~`. Only the real-IO path
/// builds remote containers, so this is absent from the coverage build.
#[cfg(not(coverage))]
const REMOTE_RUNTIME_DIR: &str = "$HOME/.marsad";

/// POSIX single-quote for remote shell commands (the remote is always a POSIX
/// `sh`, regardless of the local platform — so this is not `cfg(unix)`-gated).
fn remote_shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Resolve the `plink` executable to a concrete path.
///
/// `exec_prefix_for` returns the sentinel `"plink"` for PuTTY hosts, but PuTTY's
/// installer does not always add itself to `PATH` (and some users only have an
/// old `plink` bundled with another tool, e.g. Xming). So at spawn time we look
/// for `plink.exe` on `PATH` first, then in the standard PuTTY install dirs
/// (preferring a modern PuTTY over an Xming-bundled fallback). Any other binary
/// (`ssh`, `wsl.exe`) is returned unchanged — those are reliably on `PATH`.
/// Gated out of the coverage build (filesystem/env probing, real-IO only).
#[cfg(not(coverage))]
fn resolve_remote_bin(argv0: &str) -> String {
    if argv0 != "plink" {
        return argv0.to_string();
    }
    #[cfg(windows)]
    {
        if let Ok(path) = std::env::var("PATH") {
            for dir in path.split(';').filter(|d| !d.is_empty()) {
                let cand = std::path::Path::new(dir).join("plink.exe");
                if cand.exists() {
                    return cand.to_string_lossy().into_owned();
                }
            }
        }
        for cand in [
            r"C:\Program Files\PuTTY\plink.exe",
            r"C:\Program Files (x86)\PuTTY\plink.exe",
            r"C:\Program Files (x86)\Xming\plink.exe",
        ] {
            if std::path::Path::new(cand).exists() {
                return cand.to_string();
            }
        }
    }
    "plink".to_string()
}

/// Build the remote shell script that prints a session's transcript `.jsonl`.
///
/// Claude names its `projects/<dir>` after the session's REAL working directory
/// (every non-alphanumeric char → `-`), so we must encode the cwd Claude
/// actually used — not the raw `project` string, which may be `~` or relative.
/// The script therefore resolves the cwd ON THE REMOTE (`cd <target> && pwd`)
/// and encodes it there with `sed`, matching `encode_project_dir` exactly. With
/// a known claude uuid it cats that file; for `--continue` (no uuid) it cats the
/// most-recently-modified `.jsonl`. Run via `run_remote_script` (`bash -l -s`),
/// so `$HOME`/`~` resolve via the login shell. Real-IO build only.
#[cfg(not(coverage))]
fn remote_transcript_script(project: &str, uuid: Option<&str>) -> String {
    let cd = cd_target(project);
    // Honor CLAUDE_CONFIG_DIR (wrappers like `genie` relocate Claude's config +
    // transcripts off ~/.claude), falling back to ~/.claude. The login shell
    // (`bash -l -s`) has the wrapper's env, so $CLAUDE_CONFIG_DIR is set there.
    let resolve = format!(
        "base=${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}; \
         cwd=$(cd {cd} 2>/dev/null && pwd -P) || exit 0; \
         enc=$(printf '%s' \"$cwd\" | sed 's/[^a-zA-Z0-9]/-/g'); \
         dir=\"$base/projects/$enc\"; "
    );
    // Cap at the last ~1 MiB: full Claude `.jsonl` transcripts can be tens of MB
    // (embedded tool outputs), too big to ship as one IPC line and render. `tail
    // -c` keeps the most recent turns; the leading partial line is skipped by
    // `parse_transcript` (invalid JSON). Small transcripts are returned whole.
    match uuid {
        Some(u) if !u.is_empty() => {
            format!("{resolve}tail -c 1048576 \"$dir/{u}.jsonl\" 2>/dev/null")
        }
        _ => format!(
            "{resolve}f=$(ls -t \"$dir\"/*.jsonl 2>/dev/null | head -1); \
             [ -n \"$f\" ] && tail -c 1048576 \"$f\""
        ),
    }
}

/// Build the remote shell script that aggregates project-wide per-model token
/// usage across ALL `*.jsonl` files in the claude projects directory for a
/// session's project.  Uses python3 to sum `message.usage` fields grouped by
/// `message.model` across every `assistant`-typed line in every `.jsonl` file,
/// skipping the `<synthetic>` pseudo-model.  Prints one compact JSON object:
/// `{"<model>":{"input":N,"output":N,"cache_read":N,"cache_creation":N},...}`.
///
/// Run via `run_remote_script` on the `is_result` trigger (same branch as the
/// ChangesUpdate probe).  Guard: exit 0 silently if python3 is not available.
/// Real-IO build only.
#[cfg(not(coverage))]
fn remote_usage_script(project: &str) -> String {
    let cd = cd_target(project);
    format!(
        r#"command -v python3 >/dev/null || exit 0
base=${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}
cwd=$(cd {cd} 2>/dev/null && pwd -P) || exit 0
enc=$(printf '%s' "$cwd" | sed 's/[^a-zA-Z0-9]/-/g')
dir="$base/projects/$enc"
python3 - "$dir" << 'MARSAD_USAGE_EOF'
import sys, os, json
d = sys.argv[1]
agg = {{}}
if not os.path.isdir(d):
    print('{{}}')
    sys.exit(0)
for fn in os.listdir(d):
    if not fn.endswith('.jsonl'):
        continue
    try:
        with open(os.path.join(d, fn), encoding='utf-8', errors='replace') as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                except Exception:
                    continue
                if obj.get('type') != 'assistant':
                    continue
                msg = obj.get('message', {{}})
                model = msg.get('model', '')
                if not model or model == '<synthetic>':
                    continue
                u = msg.get('usage', {{}})
                inp = u.get('input_tokens', 0) or 0
                out = u.get('output_tokens', 0) or 0
                cr  = u.get('cache_read_input_tokens', 0) or 0
                cc  = u.get('cache_creation_input_tokens', 0) or 0
                if inp == 0 and out == 0 and cr == 0 and cc == 0:
                    continue
                if model not in agg:
                    agg[model] = {{'input': 0, 'output': 0, 'cache_read': 0, 'cache_creation': 0}}
                agg[model]['input'] += inp
                agg[model]['output'] += out
                agg[model]['cache_read'] += cr
                agg[model]['cache_creation'] += cc
    except Exception:
        pass
print(json.dumps(agg, separators=(',', ':')))
MARSAD_USAGE_EOF
"#,
        cd = cd,
    )
}

/// Build the remote shell script that probes `git status` + `diff --numstat` for
/// a session's project dir on the remote host.
///
/// Prints one TSV line per changed file: `status\tadded\tremoved\tpath`.
/// The porcelain lines supply status+path; numstat supplies added/removed for
/// tracked changes (untracked → 0/0). Run via `run_remote_script`. Real-IO only.
#[cfg(not(coverage))]
fn remote_changes_script(project: &str) -> String {
    let cd = cd_target(project);
    format!(
        r#"cd {cd} 2>/dev/null || exit 0
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 0
git status --porcelain=v1 | while IFS= read -r line; do
  st=$(printf '%s' "$line" | cut -c1-2 | tr -d ' '); f=$(printf '%s' "$line" | cut -c4-)
  printf '%s\t0\t0\t%s\n' "$st" "$f"
done
git diff --numstat
git diff --numstat --cached
"#,
        cd = cd
    )
}

/// Parse the stdout of `remote_changes_script` into a JSON `ChangesUpdate`
/// `changes` array. Lines from `git status --porcelain` contribute the status
/// and path; lines from `git diff --numstat` contribute the added/removed counts.
/// Returns the JSON array value (may be empty). Real-IO only.
#[cfg(not(coverage))]
fn parse_remote_changes(stdout: &str) -> serde_json::Value {
    use std::collections::HashMap;

    // Separate porcelain lines (tab-separated: `status\t0\t0\tpath`) from
    // numstat lines (`added\tremoved\tpath`). We distinguish them by the second
    // column: porcelain lines always have "0" there; numstat lines have a number
    // or "-" (binary). Lines that parse as neither are silently ignored.
    let mut statuses: Vec<(String, String)> = Vec::new(); // (status, path)
    let mut numstats: HashMap<String, (u64, u64)> = HashMap::new(); // path → (added, removed)

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        match parts.as_slice() {
            [st, "0", "0", path] => {
                // Porcelain line from the `while` loop.
                if !st.is_empty() && !path.is_empty() {
                    statuses.push((st.to_string(), path.to_string()));
                }
            }
            [added_s, removed_s, path] => {
                // numstat line: `added\tremoved\tpath`.
                let added = added_s.parse::<u64>().unwrap_or(0);
                let removed = removed_s.parse::<u64>().unwrap_or(0);
                if !path.is_empty() {
                    // Later numstat lines (staged diff) may overwrite earlier ones;
                    // we take the max to surface both unstaged + staged counts.
                    let entry = numstats.entry(path.to_string()).or_insert((0, 0));
                    entry.0 = entry.0.max(added);
                    entry.1 = entry.1.max(removed);
                }
            }
            _ => {}
        }
    }

    let items: Vec<serde_json::Value> = statuses
        .into_iter()
        .map(|(status, path)| {
            let (added, removed) = numstats.get(&path).copied().unwrap_or((0, 0));
            serde_json::json!({
                "path": path,
                "status": status,
                "added": added,
                "removed": removed,
            })
        })
        .collect();
    serde_json::Value::Array(items)
}

/// Run a shell `script` on a remote host and capture its stdout.
///
/// The script is fed via the child's STDIN to `bash -l -s` (login shell reading
/// from stdin), NOT embedded in argv — `wsl.exe` and `ssh` both re-parse their
/// command tail, which mangles a script full of quotes/`$()`/backslashes (the
/// cause of the empty remote listing). Piping via stdin sidesteps all argv
/// quoting. `CREATE_NO_WINDOW` stops `ssh.exe` from flashing a console on
/// Windows. stdin is closed after writing so the remote `bash -s` sees EOF and
/// runs (otherwise it hangs waiting for more input). `None` for a local/unknown
/// host or a spawn failure. Real-IO build only.
#[cfg(not(coverage))]
pub fn run_remote_script(host_target: &str, script: &str) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let prefix = exec_prefix_for(Some(host_target))?;
    let bin = resolve_remote_bin(&prefix[0]);
    let mut command = Command::new(&bin);
    command
        .args(&prefix[1..])
        .arg("bash")
        .arg("-l")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().ok()?;
    {
        // Write the script then drop stdin → EOF so `bash -s` executes and exits.
        let mut stdin = child.stdin.take()?;
        stdin.write_all(script.as_bytes()).ok()?;
    }
    let out = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Like [`run_remote_script`] but kills the child if it runs longer than
/// `timeout`, returning `None`. Used for STARTUP liveness probes: a host that
/// needs interactive MFA (Duo) and has no live ControlMaster yet will sit at the
/// prompt forever, and the old unbounded probe — run sequentially on the serve
/// thread before the command loop starts — froze the ENTIRE agent (no launches,
/// blank UI) whenever one persisted session lived on such a host. Bounding the
/// probe lets startup treat a slow/blocked host as DEAD and move on. Intended for
/// SMALL outputs (it reads stdout only after the child exits). Real-IO build only.
#[cfg(not(coverage))]
pub fn run_remote_script_bounded(
    host_target: &str,
    script: &str,
    timeout: std::time::Duration,
) -> Option<String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let prefix = exec_prefix_for(Some(host_target))?;
    let bin = resolve_remote_bin(&prefix[0]);
    let mut command = Command::new(&bin);
    command
        .args(&prefix[1..])
        .arg("bash")
        .arg("-l")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().ok()?;
    {
        let mut stdin = child.stdin.take()?;
        stdin.write_all(script.as_bytes()).ok()?;
    } // stdin dropped → EOF so `bash -s` runs and exits.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let mut out = String::new();
                if let Some(mut so) = child.stdout.take() {
                    let _ = so.read_to_string(&mut out);
                }
                return Some(out);
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

/// Resolve a durable remote session's ABSOLUTE dir + the detached claude pid,
/// read over the host's connection via the reliable stdin-pipe `run_remote_script`
/// (login shell, so `$HOME` resolves). Returns `(pid, abs_dir)`, or `None` if the
/// session dir is absent or `claude.pid` is empty (claude isn't running). These
/// literals are then baked into the writer/reader argv so those commands carry NO
/// `$HOME`/`$()`/quotes — the form that survives Rust→wsl.exe/ssh.exe. Real-IO only.
#[cfg(not(coverage))]
fn remote_resolve_pid_dir(host_target: &str, sid: &str) -> Option<(String, String)> {
    let dir = remote_session_dir(sid);
    let script =
        format!("d={dir}; cd \"$d\" 2>/dev/null && pwd; cat \"$d/claude.pid\" 2>/dev/null");
    let out = run_remote_script(host_target, &script)?;
    let mut lines = out.lines();
    let abs_dir = lines.next().unwrap_or("").trim().to_string();
    let pid = lines.next().unwrap_or("").trim().to_string();
    if abs_dir.is_empty() || pid.is_empty() {
        return None;
    }
    Some((pid, abs_dir))
}

/// List a remote host's resumable Claude sessions for `project`.
///
/// Claude stores transcripts on the REMOTE host, so the desktop's local JSONL
/// reader can't see them — the New Session "Resume" picker would only ever offer
/// "Continue". This runs, over the host's own connection (reusing
/// ControlMaster / plink-share), a script that resolves the real cwd, finds the
/// project's `.jsonl` files newest-first, and prints one
/// `mtime_ms<TAB>uuid<TAB><first user line JSON>` record per session. The caller
/// parses each record into a `SessionSummary` (the snippet via
/// `discovery::first_user_prompt` on the raw line). Returns the raw stdout, or
/// `None` for a local/unknown host or a failed connection. Real-IO build only.
#[cfg(not(coverage))]
pub fn fetch_remote_session_list(host_target: &str, project: &str) -> Option<String> {
    let cd = cd_target(project);
    let script = format!(
        "base=${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}\n\
         cwd=$(cd {cd} 2>/dev/null && pwd -P) || exit 0\n\
         enc=$(printf '%s' \"$cwd\" | sed 's/[^a-zA-Z0-9]/-/g')\n\
         dir=\"$base/projects/$enc\"\n\
         for f in $(ls -t \"$dir\"/*.jsonl 2>/dev/null); do \
           u=$(basename \"$f\" .jsonl); \
           m=$(stat -c %Y \"$f\" 2>/dev/null || echo 0); \
           l=$(grep -m1 '\"type\":\"user\"' \"$f\" 2>/dev/null | head -c 4000); \
           printf '%s000\\t%s\\t%s\\n' \"$m\" \"$u\" \"$l\"; \
         done"
    );
    run_remote_script(host_target, &script)
}

/// Resolve a host's execution target into the argv prefix every remote command is
/// run through (see docs/remote-hosts.md). Returns:
/// - `None` for the local machine (`None` / `"local"` / `"localhost"` / empty),
/// - an `ssh` prefix (with the standard option set) for `user@host` or a bare alias,
/// - a `wsl.exe` prefix for `wsl:<distro>`,
/// - a `plink` prefix (PuTTY connection sharing) for `plink:<ssh-target>`.
pub fn exec_prefix_for(host_target: Option<&str>) -> Option<Vec<String>> {
    let t = host_target.map(str::trim).filter(|s| !s.is_empty())?;
    if t == "local" || t == "localhost" {
        return None;
    }
    if let Some(distro) = t.strip_prefix("wsl:") {
        if distro.is_empty() {
            return None;
        }
        return Some(vec![
            "wsl.exe".to_string(),
            "-d".to_string(),
            distro.to_string(),
        ]);
    }
    // WSL-routed SSH: run `ssh` INSIDE a WSL distro so it gets real ControlMaster
    // (interactive MFA login once → automated connections reuse the master socket),
    // using THAT distro's `~/.ssh` config + keys. This is the Windows answer for
    // Duo/MFA hosts when the user's SSH lives in WSL — native `ssh.exe` can't do
    // ControlMaster. Format: `wslssh:<distro>:<ssh-target>` (the target is an alias
    // or `user@host` resolved by the distro's ssh, NOT by Windows). ControlMaster
    // is ALWAYS applied here (the ssh binary is Linux-in-WSL, where it works),
    // unlike the native-ssh branch which gates it on `cfg(unix)`.
    if let Some(rest) = t.strip_prefix("wslssh:") {
        let mut parts = rest.splitn(2, ':');
        let distro = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        if distro.is_empty() || target.is_empty() {
            return None;
        }
        let opt = |k: &str| ["-o".to_string(), k.to_string()];
        let mut argv = vec![
            "wsl.exe".to_string(),
            "-d".to_string(),
            distro.to_string(),
            "ssh".to_string(),
            "-T".to_string(),
        ];
        argv.extend(opt("BatchMode=yes"));
        argv.extend(opt("ConnectTimeout=10"));
        argv.extend(opt("ServerAliveInterval=30"));
        argv.extend(opt("ServerAliveCountMax=3"));
        argv.extend(opt("TCPKeepAlive=yes"));
        argv.extend(opt("ControlMaster=auto"));
        argv.extend(opt("ControlPath=~/.ssh/marsad-cm-%r@%h:%p"));
        argv.extend(opt("ControlPersist=600"));
        argv.push(target.to_string());
        return Some(argv);
    }
    // PuTTY plink with SSH connection sharing — the Windows alternative to
    // ControlMaster (which `ssh.exe` can't do: `getsockname failed: Not a socket`).
    // The interactive login (bridge.rs) runs `plink -share <inner>` and becomes the
    // sharing "upstream"; these automated connections add `-batch` (disable
    // interactive prompts) and `-share` (downstream onto that upstream) so a Duo/MFA
    // host authenticates once then is reused. Flags per the PuTTY manual: `-share`
    // enable connection sharing, `-batch` disable interactive prompts. ControlMaster
    // ssh options do NOT apply to plink and are intentionally omitted.
    if let Some(inner) = t.strip_prefix("plink:") {
        if inner.is_empty() {
            return None;
        }
        return Some(vec![
            "plink".to_string(),
            "-share".to_string(),
            "-batch".to_string(),
            inner.to_string(),
        ]);
    }
    // SSH host. BatchMode for the automated connection; ControlMaster multiplexing
    // lets it reuse an interactive login's master socket (MFA hosts); keepalive
    // stops idle drops. Mirrors the SSH option set in docs/remote-hosts.md.
    let opt = |k: &str| ["-o".to_string(), k.to_string()];
    let mut argv = vec!["ssh".to_string()];
    // `-T`: no PTY. On Windows `ssh.exe` otherwise allocates a ConPTY that flashes a
    // console window; our remote commands (tmux control, `cat` FIFO bridging) never
    // need a TTY, so disable it everywhere.
    argv.push("-T".to_string());
    argv.extend(opt("BatchMode=yes"));
    argv.extend(opt("ConnectTimeout=10"));
    argv.extend(opt("ServerAliveInterval=30"));
    argv.extend(opt("ServerAliveCountMax=3"));
    argv.extend(opt("TCPKeepAlive=yes"));
    // ControlMaster multiplexing (reuse an interactive login's master socket) needs
    // Unix-domain sockets, which Windows OpenSSH lacks — there it errors `getsockname
    // failed: Not a socket`. Enable only when the agent runs on a Unix host.
    #[cfg(unix)]
    {
        argv.extend(opt("ControlMaster=auto"));
        argv.extend(opt("ControlPath=~/.ssh/marsad-cm-%r@%h:%p"));
        argv.extend(opt("ControlPersist=600"));
    }
    argv.push(t.to_string());
    Some(argv)
}

/// Build the `claude` invocation written into the remote `run.sh` (which the remote
/// tmux session runs via `bash -l run.sh < in.fifo > out.fifo 2>err.log`). Returns
/// just `cd '<project>' && exec claude '<arg>'…` — NO redirections and NO nested
/// shell wrapping. The FIFO wiring lives in `spawn_script`, and the command body goes
/// into run.sh through a heredoc, so it's quoted exactly ONCE (the old approach nested
/// `bash -lc` inside tmux inside `bash -lc` and produced mangled quoting → claude
/// never ran).
pub fn build_remote_spawn_cmd(
    project: &str,
    argv: &[String],
    setup: Option<&str>,
    env: &[(String, String)],
) -> String {
    let bin = argv.first().map(String::as_str).unwrap_or("claude");
    let claude_args = argv.get(1..).unwrap_or(&[]);
    let mut body = String::new();
    // Optional setup command, run before claude.
    if let Some(s) = setup.filter(|s| !s.is_empty()) {
        body.push_str(s);
        body.push('\n');
    }
    // One `export 'K'='V'` line per env pair (key and value POSIX single-quoted
    // via `remote_shell_quote`).
    for (k, v) in env {
        body.push_str(&format!(
            "export {}={}\n",
            remote_shell_quote(k),
            remote_shell_quote(v)
        ));
    }
    body.push_str(&format!(
        "cd {} && exec {} {}",
        cd_target(project),
        remote_shell_quote(bin),
        claude_args
            .iter()
            .map(|a| remote_shell_quote(a))
            .collect::<Vec<_>>()
            .join(" "),
    ));
    body
}

/// Shell-quote a project path for `cd`, expanding a leading `~`. Single-quoting a
/// path like `~/` (as `remote_shell_quote` would) makes `cd '~/'` look for a literal
/// `~` directory → "No such file or directory". Map a leading `~`/`~/…` to `$HOME`
/// inside DOUBLE quotes so it expands; quote everything else literally.
fn cd_target(project: &str) -> String {
    if project == "~" {
        "\"$HOME\"".to_string()
    } else if let Some(rest) = project.strip_prefix("~/") {
        // "$HOME/<rest>" — double-quoted so $HOME expands and spaces are safe.
        format!("\"$HOME/{rest}\"")
    } else {
        remote_shell_quote(project)
    }
}

// ── claude_argv ───────────────────────────────────────────────────────────────

/// Map a `Posture` to the `--permission-mode` value passed to `claude`.
fn posture_to_mode(posture: &marsad_proto::Posture) -> &'static str {
    match posture {
        marsad_proto::Posture::Ask => "default",
        marsad_proto::Posture::AutoApproveSafe => "acceptEdits",
        marsad_proto::Posture::FullAutonomy => "bypassPermissions",
    }
}

/// Build the exact `claude` invocation for a session launch.
///
/// `first_prompt` is intentionally NOT included here — in `--input-format
/// stream-json` mode the prompt must be delivered via `in.fifo` as a
/// stream-json user message, not as a positional CLI argument.
///
/// The trailing argument depends on `spec.mode`:
/// - `Fresh`    → nothing (new conversation; cwd is set by the spawn `cd`);
/// - `Continue` → `--continue` (resume the most recent conversation in the cwd);
/// - `Resume(id)` → `--resume <id>` (resume that specific conversation).
///
/// The session's `cwd` is set by the FIFO-wired `cd <project> && claude …`
/// command (see `build_spawn_cmd`), so `--continue`/`--resume` resolve against
/// the right project directory.
pub fn claude_argv(spec: &marsad_proto::LaunchSpec) -> Vec<String> {
    let mut argv = vec![
        // argv[0] is the host's configured claude executable (a wrapper like
        // `genie` or an absolute path), defaulting to the bare `claude`.
        spec.claude_bin
            .clone()
            .unwrap_or_else(|| "claude".to_string()),
        "--print".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--model".to_string(),
        spec.model.clone(),
        "--permission-mode".to_string(),
        posture_to_mode(&spec.posture).to_string(),
    ];

    // Route tool-permission prompts to the control-protocol stream so the GUI can
    // approve/deny them. `--permission-prompt-tool stdio` is the CLI sentinel that
    // makes claude emit `can_use_tool` control_requests on stdout (verified
    // against the 2.1.x binary: `mx9("stdio") => createCanUseTool`).
    //
    // ALWAYS included, for every posture (verified: claude accepts it alongside
    // `--permission-mode bypassPermissions` with no conflict). The flag only
    // *registers* a destination for approval requests; it never forces gating:
    // - Ask (`default`): every action gates.
    // - AutoApproveSafe (`acceptEdits`): edits auto-approve; risky tools gate.
    // - FullAutonomy (`bypassPermissions`): claude's OWN checks are skipped, but
    //   anything it can't bypass — notably MCP-server-sourced HITL (the MCP's own
    //   elicitation/approval, independent of claude's permission mode) — still
    //   routes here as a gate instead of silently hanging in `--print` mode.
    argv.push("--permission-prompt-tool".to_string());
    argv.push("stdio".to_string());

    match &spec.mode {
        // Fresh: nothing to add. The session's working directory is set by the
        // `cd <project>` in the remote run command (and by the local spawn cwd),
        // so claude starts a new conversation in the right place. A positional
        // here would be parsed as claude's *prompt* (e.g. a literal "~/"), which
        // is wrong — the first task is delivered as a stream-json user message.
        marsad_proto::LaunchMode::Fresh => {}
        marsad_proto::LaunchMode::Continue => argv.push("--continue".to_string()),
        marsad_proto::LaunchMode::Resume(id) => {
            argv.push("--resume".to_string());
            argv.push(id.clone());
        }
    }

    // Disabled tools: pass as a single comma-joined value to --disallowedTools.
    // Only added when the list is non-empty (verified: the flag is absent when
    // the list is empty so the absence/presence test holds trivially).
    if !spec.disallowed_tools.is_empty() {
        argv.push("--disallowedTools".to_string());
        argv.push(spec.disallowed_tools.join(","));
    }

    argv
}

// ── parse_hook_event ──────────────────────────────────────────────────────────

/// Parse a hook payload into a `HookEvent`.
/// All arms covered by the committed hook-event tests.
fn parse_hook_event(
    hook_type: &str,
    value: &serde_json::Value,
) -> Result<marsad_events::HookEvent, AgentError> {
    match hook_type {
        "Notification" => {
            let message = value["message"].as_str().unwrap_or("").to_string();
            Ok(marsad_events::HookEvent::Notification { message })
        }
        "Stop" => Ok(marsad_events::HookEvent::Stop),
        "SubagentStop" => {
            let name = value["name"].as_str().unwrap_or("").to_string();
            Ok(marsad_events::HookEvent::SubagentStop { name })
        }
        "PreToolUse" => {
            let tool = value["tool"].as_str().unwrap_or("").to_string();
            Ok(marsad_events::HookEvent::PreToolUse { tool })
        }
        "PostToolUse" => {
            let tool = value["tool"].as_str().unwrap_or("").to_string();
            Ok(marsad_events::HookEvent::PostToolUse { tool })
        }
        "SessionStart" => Ok(marsad_events::HookEvent::SessionStart),
        "SessionEnd" => Ok(marsad_events::HookEvent::SessionEnd),
        "PreCompact" => Ok(marsad_events::HookEvent::PreCompact),
        other => Err(AgentError::Protocol(format!("unknown hook type: {other}"))),
    }
}

/// Spawn the per-session reader thread: connect the transport (local FIFO/pipe, or a
/// remote durable FIFO bridge), send the control `initialize` handshake + an optional
/// first prompt, register the writer, then stream claude's stdout to every subscriber
/// until the session ends. Used both when a session is launched AND when reconnecting
/// a session that survived an agent restart (pass `first_prompt: None` for reconnect).
///
/// # Remote durable model (design 2B)
///
/// A durable remote session uses two independent ssh/wsl children per agent connection:
///   - **Writer child**: runs `tee -a $HOME/.marsad/<sid>/in.fifo` with its stdin piped
///     from the agent.  The agent writes prompts + gate responses to it.  When the agent
///     drops the child's stdin pipe the `tee` exits, but the remote claude keeps running
///     because the keepalive holder (`sleep 999999 > in.fifo`, started at launch) still
///     holds the FIFO write-end open.
///   - **Reader child**: runs `tail -n0 --pid=<claude> -f $HOME/.marsad/<sid>/out.log`
///     with its stdout piped to the agent.  Streams only NEW output (lines written after
///     connect time) so reconnects don't replay historical output.  Historical
///     conversation is supplied by the `TranscriptBackfill` mechanism over a separate
///     `run_remote_script` call.
///
/// The writer child is stored in `children` so it can be reaped on thread exit.
/// Killing the session runs `remote_kill_script` over ssh (SIGTERM to ka+claude, dir
/// removal) rather than killing a child process.
///
/// # Auto-reconnect (connection-drop recovery)
///
/// When the reader stream ends for a REMOTE session, the thread probes liveness via
/// `remote_liveness_script` with `run_remote_script_bounded`.  If the probe itself
/// fails or times out (the connection/ControlMaster is still down), the thread retries
/// with exponential backoff.  When the connection comes back:
///   - Probe returns `LIVE` → old children are reaped, pid+dir are re-resolved, new
///     writer+reader children are spawned, TranscriptBackfill is re-run, and the read
///     loop resumes — all without any user action.
///   - Probe returns `DEAD` → claude truly exited (turn done / crashed) → normal
///     teardown, same as before.
///
/// Total retry time is capped at `RECONNECT_MAX_TOTAL_SECS` so a permanently-gone
/// host doesn't leak a thread forever.
#[cfg(not(coverage))]
#[allow(clippy::too_many_arguments)]
fn spawn_session_reader(
    sid: String,
    host_target: Option<String>,
    first_prompt: Option<String>,
    send_init: bool,
    run_command: Option<String>,
    transcript_hint: Option<(String, Option<String>)>,
    data_root: String,
    subs_fifo: std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<String>>>>,
    writers_fifo: std::sync::Arc<std::sync::Mutex<HashMap<String, SharedWriter>>>,
    pending_fifo: std::sync::Arc<std::sync::Mutex<HashMap<String, PendingGate>>>,
    children: std::sync::Arc<std::sync::Mutex<HashMap<String, std::process::Child>>>,
) {
    // `run_command` is only used when the caller signals that a launch script
    // was not yet run (i.e. not provided to spawn_container).  For the durable
    // model, it's not needed here — the launch script already ran.
    let _ = run_command;

    let session_dir = std::path::PathBuf::from(&data_root)
        .join("sessions")
        .join(format!("marsad-{sid}"));
    std::thread::spawn(move || {
        use marsad_transport::Transport;

        // For REMOTE sessions: save the project path so the read loop can probe
        // git after each result event. We extract it from transcript_hint when
        // available; otherwise we have no project path to probe (skip silently).
        let remote_project: Option<String> = if host_target.is_some() {
            transcript_hint.as_ref().map(|(p, _)| p.clone())
        } else {
            None
        };

        // ── Supervising reconnect loop ────────────────────────────────────────
        // For LOCAL sessions this loop runs exactly once (no reconnect logic).
        // For REMOTE sessions it may run multiple times: after the stream ends
        // we probe liveness and re-attach if the connection dropped but claude
        // is still alive on the remote.
        let mut is_first_attach = true;
        // Backoff state (remote reconnect only).
        let mut backoff_secs = RECONNECT_INITIAL_BACKOFF_SECS;
        let mut total_waited_secs: u64 = 0;

        'reconnect: loop {
            // ── Open and connect the transport ─────────────
            // Local session → platform FIFO/pipe transport
            // (tmux+FIFO on Unix, named pipe on Windows), yielding
            // a `SplitIo` (boxed writer + buffered boxed reader).
            //
            // Remote session (host_target resolves to an exec prefix) → DURABLE model:
            //   • Writer child: ssh/wsl running `tee -a in.fifo` (stdin piped from agent).
            //   • Reader child: ssh/wsl running `tail -n0 --pid=<cl> -f out.log` (stdout piped).
            // The writer child is stored in `children` so it can be reaped on exit.
            // The reader child is owned entirely by this thread.
            let (writer, mut reader): marsad_transport::SplitIo = match exec_prefix_for(
                host_target.as_deref(),
            ) {
                Some(prefix) => {
                    use std::process::{Command, Stdio};

                    // Resolve the session's ABSOLUTE dir + detached claude pid over
                    // the reliable stdin-pipe path, so the writer/reader commands can
                    // be built from literals (no $HOME/$()/quotes that Rust→wsl.exe/
                    // ssh.exe would mangle). If claude isn't running, bail — the
                    // session reader has nothing to attach to.
                    let host = host_target.clone().unwrap_or_default();
                    let (claude_pid, abs_dir) = match remote_resolve_pid_dir(&host, &sid) {
                        Some(pd) => pd,
                        None => {
                            eprintln!(
                                    "marsad-agent: session {sid} remote resolve failed (claude not running?) — not attaching"
                                );
                            break 'reconnect;
                        }
                    };

                    // ── Spawn writer child ─────────────────────────────────────
                    // Appends everything written to its stdin into the remote FIFO.
                    let writer_argv = remote_writer_argv(&prefix, &abs_dir);
                    let w_bin = resolve_remote_bin(&writer_argv[0]);
                    let mut w_cmd = Command::new(&w_bin);
                    w_cmd
                        .args(&writer_argv[1..])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null());
                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        w_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                    }
                    let mut writer_child = match w_cmd.spawn() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("marsad-agent: remote writer spawn {sid}: {e}");
                            break 'reconnect;
                        }
                    };
                    let child_stdin = match writer_child.stdin.take() {
                        Some(s) => s,
                        None => {
                            eprintln!("marsad-agent: remote writer child {sid} has no stdin");
                            let _ = writer_child.kill();
                            let _ = writer_child.wait();
                            break 'reconnect;
                        }
                    };
                    // Store writer child so it is reaped on thread exit.
                    children.lock().unwrap().insert(sid.clone(), writer_child);

                    // ── Spawn reader child ─────────────────────────────────────
                    // Streams only NEW out.log lines (tail -n0 = skip historical).
                    let reader_argv = remote_reader_argv(&prefix, &claude_pid, &abs_dir);
                    let r_bin = resolve_remote_bin(&reader_argv[0]);
                    let mut r_cmd = Command::new(&r_bin);
                    r_cmd
                        .args(&reader_argv[1..])
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        r_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                    }
                    let mut reader_child = match r_cmd.spawn() {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("marsad-agent: remote reader spawn {sid}: {e}");
                            // Clean up writer child we already stored.
                            if let Some(mut wc) = children.lock().unwrap().remove(&sid) {
                                let _ = wc.kill();
                                let _ = wc.wait();
                            }
                            break 'reconnect;
                        }
                    };
                    // Stream reader's stderr to agent.log for diagnosis.
                    if let Some(errpipe) = reader_child.stderr.take() {
                        let esid = sid.clone();
                        std::thread::spawn(move || {
                            use std::io::BufRead;
                            let r = std::io::BufReader::new(errpipe);
                            for line in r.lines().map_while(Result::ok) {
                                if !line.trim().is_empty() {
                                    eprintln!("marsad-agent: session {esid} reader-stderr: {line}");
                                }
                            }
                        });
                    }
                    let child_stdout = match reader_child.stdout.take() {
                        Some(s) => s,
                        None => {
                            eprintln!("marsad-agent: remote reader child {sid} has no stdout");
                            let _ = reader_child.kill();
                            let _ = reader_child.wait();
                            if let Some(mut wc) = children.lock().unwrap().remove(&sid) {
                                let _ = wc.kill();
                                let _ = wc.wait();
                            }
                            break 'reconnect;
                        }
                    };
                    // Reader child: store in the session_children registry under a
                    // distinct key so the read-loop cleanup can reap it.  We use a
                    // `"reader:<sid>"` key (never used by other code) so the normal
                    // kill path (keyed by plain sid) is unaffected.
                    let reader_key = format!("reader:{sid}");
                    children.lock().unwrap().insert(reader_key, reader_child);

                    let w: Box<dyn std::io::Write + Send> = Box::new(child_stdin);
                    let r: std::io::BufReader<Box<dyn std::io::Read + Send>> =
                        std::io::BufReader::new(Box::new(child_stdout));
                    (w, r)
                }
                None => {
                    #[cfg(unix)]
                    {
                        use marsad_transport::ClaudeFifoTransport;
                        let mut fifo = match ClaudeFifoTransport::open(&session_dir) {
                            Ok(f) => f,
                            Err(e) => {
                                eprintln!("marsad-agent: fifo open {sid}: {e}");
                                break 'reconnect;
                            }
                        };
                        if let Err(e) = fifo.connect() {
                            eprintln!("marsad-agent: fifo connect {sid}: {e}");
                            break 'reconnect;
                        }
                        match fifo.into_split() {
                            Some(pair) => pair,
                            None => {
                                eprintln!("marsad-agent: fifo not connected after connect {sid}");
                                break 'reconnect;
                            }
                        }
                    }
                    #[cfg(windows)]
                    {
                        let _ = &session_dir;
                        use marsad_transport::ClaudeNamedPipeTransport;
                        let container_name = format!("marsad-{sid}");
                        let mut transport = ClaudeNamedPipeTransport::for_session(&container_name);
                        if let Err(e) = transport.connect() {
                            eprintln!("marsad-agent: pipe connect {sid}: {e}");
                            break 'reconnect;
                        }
                        match transport.into_split() {
                            Some(pair) => pair,
                            None => {
                                eprintln!("marsad-agent: pipe not connected after connect {sid}");
                                break 'reconnect;
                            }
                        }
                    }
                }
            };

            // The writer is shared (Arc<Mutex>) between this
            // thread (initialize handshake + first prompt) and
            // the serve thread (handle_send_prompt / answer_gate)
            // — boxed writers can't be try_clone'd like a File.
            let writer: SharedWriter = std::sync::Arc::new(std::sync::Mutex::new(writer));

            // Send the control-protocol `initialize` handshake first (LAUNCH only,
            // first attach only). It pairs with `--permission-prompt-tool stdio` so
            // claude routes tool-permission prompts back as `can_use_tool`
            // control_requests. On RECONNECT the remote claude is already running +
            // initialized, so re-sending it would be wrong — hence `send_init` and
            // `is_first_attach`. Fire-and-forget: the CLI's ack is ignored.
            if send_init && is_first_attach {
                use std::io::Write;
                let init = marsad_proto::encode_control_request_initialize("req_1_init");
                let mut w = writer.lock().unwrap();
                let _ = w.write_all(init.as_bytes());
                let _ = w.write_all(b"\n");
                let _ = w.flush();
                eprintln!("marsad-agent: session {sid} sent initialize handshake");
            }

            // Deliver the first_prompt (if any) immediately so claude receives its
            // initial task.  Only on the very first attach — on reconnect, claude is
            // already running (a prompt was already sent) and re-sending would be
            // wrong.  claude in --input-format stream-json mode emits nothing until
            // it sees a user message on stdin.
            if is_first_attach {
                if let Some(ref prompt) = first_prompt {
                    use marsad_proto::{encode_user_input, ContentBlock, UserInput};
                    use std::io::Write;
                    let plen = prompt.len();
                    let encoded = encode_user_input(&UserInput {
                        content: vec![ContentBlock::Text {
                            text: prompt.clone(),
                        }],
                    });
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(encoded.as_bytes());
                    let _ = w.write_all(b"\n");
                    let _ = w.flush();
                    eprintln!("marsad-agent: session {sid} delivered first_prompt ({plen} chars) to claude stdin");
                } else {
                    eprintln!("marsad-agent: session {sid} no first_prompt — claude idle until a prompt is sent (phase stays Starting)");
                }
            }

            // Register the shared writer so handle_send_prompt
            // / handle_answer_gate can deliver future prompts and
            // gate responses to this session's stdin. Both sides
            // hold an Arc to the same Mutex<Box<dyn Write>>.
            {
                let mut registry = writers_fifo.lock().unwrap();
                registry.insert(sid.clone(), std::sync::Arc::clone(&writer));
            }

            // For a REMOTE resume/reconnect, fetch the prior conversation from the
            // remote host's `.jsonl` over a fresh connection (which reuses the
            // session's ControlMaster / plink-share, so no extra MFA), and broadcast
            // it as a `TranscriptBackfill` so core can show the history. The remote
            // file is unreachable by core's local JSONL reader, so this is the only
            // way a resumed WSL/SSH session shows its past turns. Done before the read
            // loop so the backfill precedes any live events.
            // On reconnect after a connection drop (is_first_attach == false) we do
            // NOT re-run the backfill: the transcript already streamed; re-running
            // it would duplicate all previous turns in the UI.
            if is_first_attach {
                if let Some((project, uuid)) = &transcript_hint {
                    if let Some(target) = host_target.as_deref() {
                        let script = remote_transcript_script(project, uuid.as_deref());
                        // run_remote_script pipes the script via stdin to `bash -l -s` —
                        // login shell (nested wsl.exe keeps $HOME), no argv quoting, and
                        // CREATE_NO_WINDOW so ssh.exe doesn't flash a console.
                        if let Some(jsonl) = run_remote_script(target, &script) {
                            if !jsonl.trim().is_empty() {
                                let line = marsad_proto::encode_ipc(
                                    &marsad_proto::IpcMsg::TranscriptBackfill {
                                        session_id: sid.clone(),
                                        jsonl: jsonl.clone(),
                                    },
                                );
                                let subs = subs_fifo.lock().unwrap();
                                for s in subs.iter() {
                                    let _ = s.send(line.clone());
                                }
                                eprintln!(
                                    "marsad-agent: session {sid} transcript backfill ({} bytes) broadcast",
                                    jsonl.len()
                                );
                            } else {
                                eprintln!("marsad-agent: session {sid} transcript backfill: remote .jsonl empty/absent");
                            }
                        }
                    }
                }
            }

            // For a REMOTE first-attach, probe full-project usage AFTER the transcript
            // backfill so the result supersedes the partial per-session value the
            // backfill set.  Best-effort: if the output is empty or the script fails,
            // broadcast nothing (do not send an empty UsageUpdate — the empty-guard in
            // core would no-op it, but skipping the broadcast is cleaner and avoids
            // the network round-trip).
            if is_first_attach {
                if let (Some(target), Some(project)) =
                    (host_target.as_deref(), remote_project.as_deref())
                {
                    let usage_script = remote_usage_script(project);
                    if let Some(usage_out) = run_remote_script(target, &usage_script) {
                        let trimmed = usage_out.trim();
                        if !trimmed.is_empty() && trimmed != "{}" {
                            if let Ok(usage_val) =
                                serde_json::from_str::<serde_json::Value>(trimmed)
                            {
                                let uu_line =
                                    marsad_proto::encode_ipc(&marsad_proto::IpcMsg::UsageUpdate {
                                        session_id: sid.clone(),
                                        usage: usage_val,
                                    });
                                let subs = subs_fifo.lock().unwrap();
                                subs.iter().for_each(|s| {
                                    let _ = s.send(uu_line.clone());
                                });
                                eprintln!(
                                    "marsad-agent: session {sid} usage update broadcast (attach)"
                                );
                            }
                        }
                    }
                }
            }

            is_first_attach = false;

            // Read out.log tail lines and broadcast to all subscribers. The reader
            // blocks here; the writer clone is in the session_writers registry for
            // handle_send_prompt — no lock contention.
            eprintln!("marsad-agent: session {sid} stream connected");
            use std::io::BufRead;
            let mut lines_seen = 0u64;
            // Debounce tracker for the tool_use-triggered Changes probe (#4).
            // Stores the Instant of the last probe so rapid edits don't spam ssh
            // (at most one probe per CHANGES_PROBE_DEBOUNCE_SECS per session).
            let mut last_changes_probe: Option<std::time::Instant> = None;
            const CHANGES_PROBE_DEBOUNCE_SECS: u64 = 3;
            loop {
                let mut raw = String::new();
                match reader.read_line(&mut raw) {
                    Ok(0) => {
                        eprintln!(
                            "marsad-agent: session {sid} reader EOF after {lines_seen} line(s)"
                        );
                        break;
                    }
                    Err(e) => {
                        eprintln!("marsad-agent: session {sid} reader ERROR after {lines_seen} line(s): {e}");
                        break;
                    }
                    Ok(_) => {
                        lines_seen += 1;
                        let raw = raw.trim_end_matches('\n');

                        // Capture a `can_use_tool` control
                        // request as a pending gate so a later
                        // AnswerGate can write the matching
                        // control_response. The event is still
                        // broadcast (below) so the GUI can
                        // render the gate and answer it.
                        if let Ok(marsad_proto::StreamMsg::ControlRequest(cr)) =
                            marsad_proto::parse_stream_line(raw)
                        {
                            let mut gates = pending_fifo.lock().unwrap();
                            // Deduplicate: if the SAME request_id already
                            // sits in the registry (claude re-emitted it),
                            // ignore the duplicate so we don't clobber the
                            // existing entry and misroute a later AnswerGate.
                            // If it's a genuinely NEW request_id while one
                            // is already pending, replace it (the first one
                            // was either answered or abandoned) and warn.
                            let dup = gates
                                .get(&sid)
                                .map(|existing| existing.request_id == cr.request_id)
                                .unwrap_or(false);
                            if dup {
                                eprintln!(
                                    "marsad-agent: session {sid} gate DUPLICATE ignored (control_request {})",
                                    cr.request_id
                                );
                            } else {
                                if gates.contains_key(&sid) {
                                    eprintln!(
                                        "marsad-agent: session {sid} gate REPLACED — new control_request {} (previous unanswered)",
                                        cr.request_id
                                    );
                                } else {
                                    eprintln!(
                                        "marsad-agent: session {sid} gate captured (control_request {})",
                                        cr.request_id
                                    );
                                }
                                gates.insert(
                                    sid.clone(),
                                    PendingGate {
                                        request_id: cr.request_id,
                                        input: cr.input,
                                        elicitation: cr.elicitation_message.is_some(),
                                    },
                                );
                            }
                        }

                        let event: serde_json::Value = serde_json::from_str(raw)
                            .unwrap_or_else(|_| serde_json::json!({"raw": raw}));

                        // Determine whether this event should trigger a
                        // remote git Changes probe.
                        //
                        // Trigger 1 (existing): `result` event — turn ended.
                        // Trigger 2 (new, #4): `assistant` event containing a
                        // `tool_use` block for a file-modifying tool (Bash,
                        // Edit, Write, NotebookEdit, MultiEdit).  Debounced:
                        // at most one probe per CHANGES_PROBE_DEBOUNCE_SECS.
                        let is_result =
                            event.get("type").and_then(|t| t.as_str()) == Some("result");
                        let is_file_modifying_tool_use = !is_result
                            && event.get("type").and_then(|t| t.as_str()) == Some("assistant")
                            && assistant_has_file_modifying_tool_use(&event);
                        let should_probe_changes = is_result
                            || (is_file_modifying_tool_use && {
                                let now = std::time::Instant::now();
                                let elapsed_ok = last_changes_probe
                                    .map(|t| {
                                        now.duration_since(t).as_secs()
                                            >= CHANGES_PROBE_DEBOUNCE_SECS
                                    })
                                    .unwrap_or(true);
                                if elapsed_ok {
                                    last_changes_probe = Some(now);
                                }
                                elapsed_ok
                            });
                        if should_probe_changes {
                            if let (Some(target), Some(project)) =
                                (host_target.as_deref(), remote_project.as_deref())
                            {
                                let script = remote_changes_script(project);
                                if let Some(stdout) = run_remote_script(target, &script) {
                                    let changes = parse_remote_changes(&stdout);
                                    let cu_line = marsad_proto::encode_ipc(
                                        &marsad_proto::IpcMsg::ChangesUpdate {
                                            session_id: sid.clone(),
                                            changes,
                                        },
                                    );
                                    let subs2 = subs_fifo.lock().unwrap();
                                    subs2.iter().for_each(|s| {
                                        let _ = s.send(cu_line.clone());
                                    });
                                    eprintln!(
                                        "marsad-agent: session {sid} changes update broadcast ({})",
                                        if is_result { "result" } else { "tool_use" }
                                    );
                                }
                                // Project-wide usage aggregation: run on result events
                                // only (not on every tool_use — it reads all *.jsonl,
                                // so it's more expensive than the git probe).
                                if is_result {
                                    let usage_script = remote_usage_script(project);
                                    if let Some(usage_out) =
                                        run_remote_script(target, &usage_script)
                                    {
                                        let trimmed = usage_out.trim();
                                        if !trimmed.is_empty() && trimmed != "{}" {
                                            if let Ok(usage_val) =
                                                serde_json::from_str::<serde_json::Value>(trimmed)
                                            {
                                                let uu_line = marsad_proto::encode_ipc(
                                                    &marsad_proto::IpcMsg::UsageUpdate {
                                                        session_id: sid.clone(),
                                                        usage: usage_val,
                                                    },
                                                );
                                                let subs3 = subs_fifo.lock().unwrap();
                                                subs3.iter().for_each(|s| {
                                                    let _ = s.send(uu_line.clone());
                                                });
                                                eprintln!(
                                                    "marsad-agent: session {sid} usage update broadcast"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let ipc_msg = marsad_proto::IpcMsg::AgentEvent {
                            session_id: sid.clone(),
                            event,
                        };
                        let encoded = marsad_proto::encode_ipc(&ipc_msg);
                        // Broadcast to all subscribers.
                        let subs = subs_fifo.lock().unwrap();
                        subs.iter().for_each(|s| {
                            let _ = s.send(encoded.clone());
                        });
                    }
                }
            }

            eprintln!("marsad-agent: session {sid} stream ended after {lines_seen} line(s)");
            // If claude produced NO output on a remote session, run a diagnosis so the
            // agent.log gives a clue (PATH, module, etc.).
            if lines_seen == 0 {
                if let Some(target) = host_target.as_deref() {
                    // Dump claude's actual out.log — claude's stderr/error is redirected
                    // there (start.sh: `>> out.log 2>&1`), but the reader's `tail -n0`
                    // skips pre-existing content, so a startup error (bad flag, crash,
                    // `--continue` with no session, etc.) never reaches the stream. This
                    // surfaces the REAL reason claude exited, plus a PATH check.
                    let dir = remote_session_dir(&sid);
                    let script = format!(
                        "echo '== claude =='; command -v claude 2>/dev/null || echo 'claude NOT on PATH'; \
                         echo '== out.log (claude startup output/error) =='; \
                         tail -c 2000 {dir}/out.log 2>/dev/null"
                    );
                    if let Some(out) = run_remote_script(target, &script) {
                        eprintln!(
                            "marsad-agent: session {sid} remote diagnosis:\n{}",
                            out.trim()
                        );
                    }
                }
            }

            // ── Teardown for this connection attempt ─────────────────────────
            // Remove writer from registry and reap children.  For remote sessions
            // we may re-attach below (reconnect loop), so the registry slot will be
            // re-inserted on the next iteration; for local sessions and definitive
            // exits the slot stays absent (correct).
            {
                let mut registry = writers_fifo.lock().unwrap();
                registry.remove(&sid);
            }
            drop(writer);
            // Reap the writer child (remote sessions: the `tee -a in.fifo` child).
            // Dropping the writer above closes the child's stdin; `wait()` reaps it.
            // Local sessions never insert into `children`, so this is a no-op for them.
            if let Some(mut child) = children.lock().unwrap().remove(&sid) {
                let _ = child.wait();
            }
            // Reap the reader child (remote sessions: the `tail -n0 -f out.log` child)
            // stored under the `"reader:<sid>"` key.
            {
                let reader_key = format!("reader:{sid}");
                if let Some(mut rchild) = children.lock().unwrap().remove(&reader_key) {
                    let _ = rchild.kill(); // tail -f never exits on its own; kill it.
                    let _ = rchild.wait();
                }
            }

            // ── Remote reconnect: probe liveness, retry or stop ──────────────
            // Only for REMOTE sessions.  Local sessions (host_target None) stop here.
            let host = match host_target.as_deref() {
                Some(h) if exec_prefix_for(Some(h)).is_some() => h,
                _ => break 'reconnect, // local session or no exec prefix → stop
            };

            eprintln!("marsad-agent: session {sid} connection lost, probing liveness");

            // Probe + backoff loop: keep probing until the connection comes back
            // (probe succeeds) or we exceed the total retry cap.
            loop {
                if total_waited_secs >= RECONNECT_MAX_TOTAL_SECS {
                    eprintln!(
                        "marsad-agent: session {sid} reconnect cap ({RECONNECT_MAX_TOTAL_SECS}s) reached — giving up"
                    );
                    break 'reconnect;
                }

                // Sleep before the next probe (always, to avoid busy-looping).
                eprintln!("marsad-agent: session {sid} still down, retrying in {backoff_secs}s");
                std::thread::sleep(std::time::Duration::from_secs(backoff_secs));
                total_waited_secs = total_waited_secs.saturating_add(backoff_secs);
                backoff_secs = reconnect_backoff_next(backoff_secs);

                // Bounded probe: if the connection is still down, the remote
                // `bash -s` can't even start → the spawn or read fails quickly,
                // returning None.  We treat None as "still down".
                let probe_result = run_remote_script_bounded(
                    host,
                    &remote_liveness_script(&sid),
                    std::time::Duration::from_secs(RECONNECT_PROBE_TIMEOUT_SECS),
                );

                match probe_result {
                    None => {
                        // Connection still down (probe failed / timed out).
                        // Continue the probe loop.
                    }
                    Some(out) => {
                        // Connection is back.  Interpret the result.
                        if out.trim() == "LIVE" {
                            eprintln!(
                                "marsad-agent: session {sid} reconnected live remote session"
                            );
                            // Reset backoff for next potential drop.
                            backoff_secs = RECONNECT_INITIAL_BACKOFF_SECS;
                            total_waited_secs = 0;
                            // Re-attach: continue the outer reconnect loop.
                            continue 'reconnect;
                        } else {
                            eprintln!("marsad-agent: session {sid} claude exited, stopping");
                            break 'reconnect;
                        }
                    }
                }
            } // probe loop
        } // 'reconnect loop

        // Final cleanup: ensure the writer registry slot is gone (it may already
        // be absent if the reconnect loop tore it down; the remove is idempotent).
        {
            let mut registry = writers_fifo.lock().unwrap();
            registry.remove(&sid);
        }
    });
}

// ── Reconnect backoff constants and helper ───────────────────────────────────

/// Initial delay before the first liveness re-probe after a connection drop (seconds).
pub const RECONNECT_INITIAL_BACKOFF_SECS: u64 = 3;

/// Maximum delay between liveness probes (seconds).
pub const RECONNECT_MAX_BACKOFF_SECS: u64 = 30;

/// Maximum total time to spend retrying a dropped connection (seconds).
/// After this the reader thread gives up and the session is considered lost.
/// 30 minutes handles MFA re-auth windows; a permanently-gone host won't
/// leak the thread forever.
pub const RECONNECT_MAX_TOTAL_SECS: u64 = 1800;

/// Timeout for each bounded liveness probe while the connection is down (seconds).
/// Short so a host that is still unreachable doesn't stall the backoff loop.
pub const RECONNECT_PROBE_TIMEOUT_SECS: u64 = 8;

/// Compute the next backoff delay: double the current value, capped at
/// `RECONNECT_MAX_BACKOFF_SECS`.
///
/// Pure logic (no IO) — compiled in all build modes and unit-tested.
pub fn reconnect_backoff_next(current: u64) -> u64 {
    current.saturating_mul(2).min(RECONNECT_MAX_BACKOFF_SECS)
}

// ── assistant_has_file_modifying_tool_use ────────────────────────────────────

/// Return `true` if the given `assistant` stream event contains at least one
/// `tool_use` content block whose tool name is a known file-modifying tool.
///
/// Used by `spawn_session_reader` (#4) to decide whether to trigger a debounced
/// remote git Changes probe BEFORE the turn ends, so the Changes tab updates
/// while claude is mid-turn writing files rather than only at `result`.
///
/// Pure logic (no IO), compiled in all build modes — covered by unit tests.
pub fn assistant_has_file_modifying_tool_use(event: &serde_json::Value) -> bool {
    // File-modifying tools whose use signals a likely git-visible change.
    const FILE_TOOLS: &[&str] = &["Bash", "Edit", "Write", "NotebookEdit", "MultiEdit"];
    let content = match event
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };
    content.iter().any(|block| {
        block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            && block
                .get("name")
                .and_then(|n| n.as_str())
                .map(|name| FILE_TOOLS.contains(&name))
                .unwrap_or(false)
    })
}

// ── Durable remote session helpers ────────────────────────────────────────────
//
// These functions build the shell scripts and argv vectors for the durable
// remote session model (design 2B: FIFO + setsid + keepalive).  Pure logic
// (no IO), so they are NOT gated on `cfg(not(coverage))` and are always
// coverage-tested via the new tests in `tests/remote.rs`.

/// Return the remote dir path for a session (`$HOME/.marsad/<sid>`).
///
/// The shell literal `$HOME` is used (not expanded at build time) so it
/// resolves correctly on the remote host regardless of the local platform.
pub fn remote_session_dir(sid: &str) -> String {
    format!("$HOME/.marsad/{}", remote_shell_quote(sid))
}

/// Build the shell script that launches a NEW durable remote session.
///
/// Run via `bash -l -s` on the remote. Steps:
///
/// 1. Create `$HOME/.marsad/<sid>/` and a named FIFO `in.fifo`.
/// 2. Start a keepalive holder (`nohup setsid sleep 999999 > in.fifo`) that keeps
///    the FIFO write-end open so claude never sees EOF when the agent disconnects.
/// 3. Start the detached claude (`nohup setsid sh -c '<run_cmd> < in.fifo >>
///    out.log 2>&1'`). `nohup` + `setsid` detach it from the ssh session.
/// 4. Write `ka.pid` (the keepalive `$!`) and `claude.pid`.
///
/// `claude.pid` is written from INSIDE `start.sh` as `$$` — the REAL claude pid.
/// The exec chain `start.sh (sh) → exec bash -l run.sh → exec claude` keeps one
/// pid throughout, so `$$` captured at the top of `start.sh` IS claude's pid. We
/// must NOT rely on the launcher's `$!` (the `nohup setsid` wrapper): `setsid`
/// forks on some systems and the wrapper then exits immediately, leaving `$!`
/// pointing at a dead pid. That bug made `tail --pid=<claude.pid>` quit instantly
/// ("reader EOF after 0 lines") and `remote_liveness_script` report DEAD on a
/// claude that was actually alive — breaking launch display AND resume. The
/// launcher still seeds `claude.pid` with `$!` as a placeholder; `start.sh`
/// overwrites it with `$$`, and a short `sleep` lets that land before the reader
/// resolves the pid.
///
/// `run_cmd` is the `build_remote_spawn_cmd` output (`cd '<proj>' && exec
/// 'claude' <args>`); it is embedded verbatim — no extra shell quoting.
pub fn remote_durable_launch_script(sid: &str, run_cmd: &str) -> String {
    let dir = remote_session_dir(sid);
    // The run_cmd comes from build_remote_spawn_cmd which already applies
    // remote_shell_quote to all paths and args.  Embedding it inside a
    // `sh -c '...'` would create nested quoting problems (the run_cmd uses
    // POSIX single-quoting internally).  We instead write it to `run.sh` via
    // a heredoc (single-quoted marker `'MARSAD_RUN'` prevents `$`/`\`
    // expansion inside the body, so the run_cmd is safe).  A separate
    // `start.sh` wrapper execs `run.sh` with the FIFO redirections — this
    // avoids passing the dir path through any additional quoting layer and
    // lets `nohup setsid` operate on a simple executable path.
    format!(
        r#"set -e
dir={dir}
mkdir -p "$dir"
rm -f "$dir/in.fifo"
mkfifo "$dir/in.fifo"
touch "$dir/out.log"
cat > "$dir/run.sh" << 'MARSAD_RUN'
{run_cmd}
MARSAD_RUN
printf '#!/bin/sh\necho $$ > "%s/claude.pid"\nexec bash -l "%s/run.sh" < "%s/in.fifo" >> "%s/out.log" 2>&1\n' "$dir" "$dir" "$dir" "$dir" > "$dir/start.sh"
chmod +x "$dir/run.sh" "$dir/start.sh"
nohup setsid sh -c 'exec sleep 999999 > '"$dir/in.fifo"'' </dev/null >/dev/null 2>/dev/null &
echo $! > "$dir/ka.pid"
sleep 0.1
nohup setsid "$dir/start.sh" </dev/null >/dev/null 2>/dev/null &
echo $! > "$dir/claude.pid"
sleep 0.3
echo "marsad-durable-launch: sid={sid} dir=$dir"
"#,
        dir = dir,
        run_cmd = run_cmd,
        sid = sid,
    )
}

/// Build the shell script that checks whether a durable remote session is live.
///
/// Returns `LIVE` on stdout if ALL of:
///  - the session dir exists,
///  - `in.fifo` is a named FIFO (proves the session was set up),
///  - the keepalive PID recorded in `ka.pid` is still running, AND
///  - the claude PID recorded in `claude.pid` is still running.
///
/// Returns `DEAD` otherwise.  Requiring `claude.pid` (not just the keepalive)
/// prevents the agent from "reconnecting" to a zombie session whose keepalive
/// still holds the FIFO open but whose claude process has already exited (e.g.
/// crashed on startup, or finished) — such a reconnect would attach a reader to
/// a stream that will never produce output.  Intended for `run_remote_script`.
pub fn remote_liveness_script(sid: &str) -> String {
    let dir = remote_session_dir(sid);
    format!(
        r#"dir={dir}
if [ ! -p "$dir/in.fifo" ]; then echo DEAD; exit 0; fi
ka=$(cat "$dir/ka.pid" 2>/dev/null || echo "")
cl=$(cat "$dir/claude.pid" 2>/dev/null || echo "")
if [ -n "$ka" ] && kill -0 "$ka" 2>/dev/null \
   && [ -n "$cl" ] && kill -0 "$cl" 2>/dev/null; then echo LIVE; else echo DEAD; fi
"#,
        dir = dir,
    )
}

/// Build the shell script that tears down a durable remote session.
///
/// Sends SIGTERM to the keepalive holder and the detached claude process (both
/// PIDs are stored in `$HOME/.marsad/<sid>/`), waits briefly for them to exit,
/// then removes the session dir entirely.  Idempotent — missing PIDs or a
/// missing dir are silently ignored.
pub fn remote_kill_script(sid: &str) -> String {
    let dir = remote_session_dir(sid);
    format!(
        r#"dir={dir}
if [ -d "$dir" ]; then
  ka=$(cat "$dir/ka.pid" 2>/dev/null || echo "")
  cl=$(cat "$dir/claude.pid" 2>/dev/null || echo "")
  [ -n "$ka" ] && kill "$ka" 2>/dev/null || true
  [ -n "$cl" ] && kill "$cl" 2>/dev/null || true
  sleep 0.2
  [ -n "$ka" ] && kill -9 "$ka" 2>/dev/null || true
  [ -n "$cl" ] && kill -9 "$cl" 2>/dev/null || true
  rm -rf "$dir"
  echo "marsad-kill: removed $dir"
else
  echo "marsad-kill: dir not found, nothing to do"
fi
"#,
        dir = dir,
    )
}

/// Build the argv for the writer child: an ssh/wsl process running
/// `cat >> $HOME/.marsad/<sid>/in.fifo` with its stdin piped from the agent.
///
/// The agent writes prompts (stream-json user messages) to the child's stdin;
/// the child forwards them into the remote FIFO.  The child's stdin EOF closes
/// the `cat` on the remote side — but the FIFO is kept open by the keepalive
/// holder, so the detached claude does NOT see EOF.
/// Build the argv for the writer child: `tee -a <dir>/in.fifo` with its stdin
/// piped from the agent. The agent writes prompts (stream-json user messages) to
/// the child's stdin; `tee` appends them into the remote FIFO (stdout → the
/// child's null). The child's stdin EOF closes `tee` on the remote side — but the
/// FIFO is kept open by the keepalive holder, so detached claude does NOT see EOF.
///
/// `dir` is the RESOLVED ABSOLUTE session dir (from `remote_resolve_pid_dir`), and
/// the args are passed as separate argv tokens — NO shell, NO quotes, NO `$()`.
/// This is essential: a command string with embedded `"$(...)"` (the old form) is
/// mangled when Rust's Windows arg-quoting passes it through `wsl.exe`/`ssh.exe`,
/// which made `tail`/`cat` fail and the session die instantly ("reader EOF after
/// 0 lines"). Keeping every token shell-free and quote-free avoids that entirely.
pub fn remote_writer_argv(prefix: &[String], dir: &str) -> Vec<String> {
    let mut argv = prefix.to_vec();
    argv.push("tee".to_string());
    argv.push("-a".to_string());
    argv.push(format!("{dir}/in.fifo"));
    argv
}

/// Build the argv for the reader child: `tail -n0 --pid=<pid> -f <dir>/out.log`
/// with its stdout piped to the agent. `-n0` streams only NEW lines (history comes
/// from `TranscriptBackfill`); `--pid=<claude>` makes `tail` EOF when the detached
/// claude dies (stop/crash/kill) so the reader tears down cleanly instead of
/// hanging "alive". `pid` + `dir` are resolved literals (see
/// `remote_resolve_pid_dir`); passed as separate argv tokens — NO shell, NO
/// quotes, NO `$()` — to survive Rust→`wsl.exe`/`ssh.exe` arg passing.
pub fn remote_reader_argv(prefix: &[String], pid: &str, dir: &str) -> Vec<String> {
    let mut argv = prefix.to_vec();
    argv.push("tail".to_string());
    argv.push("-n0".to_string());
    argv.push(format!("--pid={pid}"));
    argv.push("-f".to_string());
    argv.push(format!("{dir}/out.log"));
    argv
}
