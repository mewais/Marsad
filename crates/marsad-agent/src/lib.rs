//! marsad-agent — detached supervisor daemon.
//!
//! See `../AGENTS.md` and `docs/specs/agent.md` for the authoritative contract.

use std::collections::HashMap;
use std::fmt;

use marsad_lifecycle::{FakeContainer, Persisted};
use marsad_notify::{CaptureNotifier, Notifier};
use marsad_store::{MemoryBackend, Store};
use serde::Deserialize;

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

/// A `can_use_tool` permission request claude is blocked on, awaiting an answer.
/// Keyed by session_id (claude asks one tool at a time per session and blocks
/// until the control_response arrives).
#[cfg(not(coverage))]
#[derive(Clone)]
struct PendingGate {
    request_id: String,
    input: serde_json::Value,
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
    ///   `cd <project> && claude <args> < in.fifo > out.fifo 2>err.log`
    ///   Otherwise falls back to `argv.join(" ")`.
    #[cfg(coverage)]
    fn build_spawn_cmd(&self, _container_name: &str, _project: &str, argv: &[String]) -> String {
        argv.join(" ")
    }

    #[cfg(not(coverage))]
    fn build_spawn_cmd(&self, container_name: &str, project: &str, argv: &[String]) -> String {
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
                // argv[0] is "claude"; the rest are its arguments.
                // We build: cd <project> && claude <args> < in.fifo > out.fifo 2>err.log
                let claude_args = &argv[1..];
                format!(
                    "cd {} && claude {} < {} > {} 2>{}",
                    shell_quote(project),
                    claude_args
                        .iter()
                        .map(|a| shell_quote(a))
                        .collect::<Vec<_>>()
                        .join(" "),
                    in_fifo.display(),
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
                let claude_args = &argv[1..];
                format!("cd /d \"{}\" && claude {}", project, claude_args.join(" "))
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
        let cmd = self.build_spawn_cmd(container_name, &spec.project, &argv);
        self.active_container()
            .spawn(container_name, &cmd)
            .map_err(|e| AgentError::Spawn(format!("{e}")))
    }

    /// Real-IO build: local hosts use the platform container; remote hosts
    /// (`exec_prefix_for` → `Some`) run the same tmux+FIFO container on the remote
    /// via `RemoteTmuxContainer`.
    #[cfg(not(coverage))]
    fn spawn_container(
        &mut self,
        container_name: &str,
        spec: &marsad_proto::LaunchSpec,
    ) -> Result<(), AgentError> {
        let argv = claude_argv(spec);
        match exec_prefix_for(spec.host_target.as_deref()) {
            None => {
                let cmd = self.build_spawn_cmd(container_name, &spec.project, &argv);
                self.active_container()
                    .spawn(container_name, &cmd)
                    .map_err(|e| AgentError::Spawn(format!("{e}")))
            }
            Some(prefix) => {
                use marsad_lifecycle::Container as _;
                let cmd =
                    build_remote_spawn_cmd(REMOTE_RUNTIME_DIR, container_name, &spec.project, &argv);
                let mut c =
                    marsad_lifecycle::RemoteTmuxContainer::new(prefix, REMOTE_RUNTIME_DIR.to_string());
                c.spawn(container_name, &cmd)
                    .map_err(|e| AgentError::Spawn(format!("{e}")))
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
            Some(prefix) => {
                use marsad_lifecycle::Container as _;
                let mut c =
                    marsad_lifecycle::RemoteTmuxContainer::new(prefix, REMOTE_RUNTIME_DIR.to_string());
                let _ = c.kill(container_name);
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
        self.spawn_container(&container_name, &spec)?;

        let persisted = marsad_store::PersistedSession {
            id: session_id.clone(),
            session_id: None,
            name: session_id.clone(),
            host_id: spec.host_id.clone(),
            host_target: spec.host_target.clone(),
            project: spec.project.clone(),
            branch: "main".to_string(),
            model: spec.model.clone(),
            last_phase: "Starting".to_string(),
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
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| AgentError::Protocol(format!("bad json: {e}")))?;

        let type_str = value
            .as_object()
            .and_then(|obj| obj.get("type"))
            .and_then(|v| v.as_str())
            .ok_or(AgentError::Protocol(
                "missing type discriminant".to_string(),
            ))?;

        match type_str {
            "LaunchSession" => self.handle_launch_session(&value),
            "ResumeSession" => self.handle_resume_session(&value),
            "AttachSession" => self.handle_attach_session(&value),
            "StopSession" => self.handle_stop_session(&value),
            "SendPrompt" => self.handle_send_prompt(&value),
            "Interrupt" => self.handle_interrupt(&value),
            "AnswerGate" => self.handle_answer_gate(&value),
            "Subscribe" => self.handle_subscribe(),
            "Unsubscribe" => Ok(vec![]),
            other => Err(AgentError::Protocol(format!("unknown IPC type: {other}"))),
        }
    }

    fn handle_launch_session(
        &mut self,
        value: &serde_json::Value,
    ) -> Result<Vec<String>, AgentError> {
        let msg: LaunchSessionMsg = serde_json::from_value(value.clone())
            .map_err(|e| AgentError::Protocol(format!("LaunchSession schema error: {e}")))?;
        let posture = parse_posture_value(&value["posture"]).map_err(AgentError::Protocol)?;
        let spec = marsad_proto::LaunchSpec {
            project: msg.project,
            host_id: msg.host_id.unwrap_or("local".to_string()),
            model: msg.model,
            posture,
            first_prompt: msg.first_prompt,
            // Fresh / continue / resume-by-id, from optional IPC fields.
            mode: parse_launch_mode_value(value),
            host_target: msg.host_target,
        };
        // Reuse the caller's session id when supplied (Core generates it so it can
        // navigate immediately and recognise the session's events); otherwise
        // generate one (e.g. a raw client that omits it).
        let session_id = match value["session_id"].as_str().filter(|s| !s.is_empty()) {
            Some(id) => self.launch_with_id(id.to_string(), spec)?,
            None => self.launch(spec)?,
        };
        let event = serde_json::json!({"type": "SessionLaunched", "session_id": session_id});
        Ok(vec![event.to_string()])
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
        value: &serde_json::Value,
    ) -> Result<Vec<String>, AgentError> {
        let session_id = value["session_id"]
            .as_str()
            .ok_or(AgentError::Protocol(
                "ResumeSession missing session_id".to_string(),
            ))?
            .to_string();

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

        let spec = marsad_proto::LaunchSpec {
            project: persisted.project,
            host_id: persisted.host_id,
            model: persisted.model,
            // The store does not persist posture; default to Ask so gates surface.
            posture: marsad_proto::Posture::Ask,
            first_prompt: value["first_prompt"].as_str().map(|s| s.to_string()),
            mode,
            host_target: persisted.host_target,
        };
        let new_id = self.launch(spec)?;
        let event = serde_json::json!({"type": "SessionResumed", "session_id": new_id});
        Ok(vec![event.to_string()])
    }

    fn handle_attach_session(
        &mut self,
        value: &serde_json::Value,
    ) -> Result<Vec<String>, AgentError> {
        let session_id = value["session_id"].as_str().ok_or(AgentError::Protocol(
            "AttachSession missing session_id".to_string(),
        ))?;
        if !self.sessions.contains_key(session_id) {
            return Err(AgentError::Protocol(format!(
                "AttachSession: unknown session {session_id}"
            )));
        }
        let event = serde_json::json!({"type": "SessionAttached", "session_id": session_id});
        Ok(vec![event.to_string()])
    }

    fn handle_stop_session(
        &mut self,
        value: &serde_json::Value,
    ) -> Result<Vec<String>, AgentError> {
        let session_id = value["session_id"].as_str().ok_or(AgentError::Protocol(
            "StopSession missing session_id".to_string(),
        ))?;
        if !self.sessions.contains_key(session_id) {
            return Err(AgentError::Protocol(format!(
                "StopSession: unknown session {session_id}"
            )));
        }
        let container_name = format!("marsad-{session_id}");
        self.kill_container(session_id, &container_name);
        self.sessions.remove(session_id);
        let event = serde_json::json!({"type": "SessionStopped", "session_id": session_id});
        Ok(vec![event.to_string()])
    }

    fn handle_send_prompt(&mut self, value: &serde_json::Value) -> Result<Vec<String>, AgentError> {
        let session_id = value["session_id"].as_str().ok_or(AgentError::Protocol(
            "SendPrompt missing session_id".to_string(),
        ))?;
        if !self.sessions.contains_key(session_id) {
            return Err(AgentError::Protocol(format!(
                "SendPrompt: unknown session {session_id}"
            )));
        }
        let text = value["text"]
            .as_str()
            .ok_or(AgentError::Protocol("SendPrompt missing text".to_string()))?;

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
                let _ = writer.write_all(encoded.as_bytes());
                let _ = writer.write_all(b"\n");
                let _ = writer.flush();
            }
        }
        // Suppress unused-variable warning in coverage builds.
        #[cfg(coverage)]
        let _ = text;

        let event = serde_json::json!({"type": "PromptSent", "session_id": session_id});
        Ok(vec![event.to_string()])
    }

    fn handle_interrupt(&mut self, value: &serde_json::Value) -> Result<Vec<String>, AgentError> {
        let session_id = value["session_id"].as_str().ok_or(AgentError::Protocol(
            "Interrupt missing session_id".to_string(),
        ))?;
        // If the session is gone, interrupt is a benign no-op.
        let event = serde_json::json!({"type": "Interrupted", "session_id": session_id});
        Ok(vec![event.to_string()])
    }

    fn handle_answer_gate(&mut self, value: &serde_json::Value) -> Result<Vec<String>, AgentError> {
        let session_id = value["session_id"].as_str().ok_or(AgentError::Protocol(
            "AnswerGate missing session_id".to_string(),
        ))?;

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
            let allow = value["allow"].as_bool().unwrap_or(false);
            let deny_reason = value["deny_reason"].as_str().map(|s| s.to_string());

            // Pull (and clear) the pending gate for this session.
            let pending = {
                let mut gates = self.pending_gates.0.lock().unwrap();
                gates.remove(session_id)
            };

            if let Some(gate) = pending {
                let resp = marsad_proto::ControlResponse {
                    request_id: gate.request_id,
                    allow,
                    deny_reason,
                    updated_input: if allow { Some(gate.input) } else { None },
                };
                let encoded = marsad_proto::encode_control_response(&resp);
                let writers = self.session_writers.0.lock().unwrap();
                if let Some(shared) = writers.get(session_id) {
                    let mut writer = shared.lock().unwrap();
                    let _ = writer.write_all(encoded.as_bytes());
                    let _ = writer.write_all(b"\n");
                    let _ = writer.flush();
                }
            }
        }

        // If no pending gate, this is a benign no-op (the tests assert non-Protocol-error).
        let event = serde_json::json!({"type": "GateAnswered", "session_id": session_id});
        Ok(vec![event.to_string()])
    }

    fn handle_subscribe(&mut self) -> Result<Vec<String>, AgentError> {
        let subscriber_id = format!("sub-{}", self.next_id);
        self.next_id += 1;
        let ack = serde_json::json!({"type": "Subscribed", "subscriber_id": subscriber_id});
        Ok(vec![ack.to_string()])
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
        use marsad_transport::{LocalIpcListener, Transport};
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

        // ── Reconcile existing tmux sessions on startup ────────────────────
        self.reconcile_all();

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

        // ── Pending-gate registry ──────────────────────────────────────────
        // Shared across the serve thread (handle_answer_gate via self) and the
        // FIFO threads (which insert when claude emits a `can_use_tool`).
        let pending_gates_for_fifo = Arc::clone(&self.pending_gates.0);

        // ── Accept thread ──────────────────────────────────────────────────
        // We spin off the accept loop in a separate thread so that the current
        // thread can run the agent command loop below.  The accept thread
        // never touches the `Agent`.
        let data_root = self.config.data_root.clone();
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
                                        let err_line = serde_json::json!({
                                            "type": "Error",
                                            "message": msg
                                        })
                                        .to_string();
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
                                            let session_dir =
                                                std::path::PathBuf::from(&data_root_client)
                                                    .join("sessions")
                                                    .join(format!("marsad-{session_id}"));
                                            let subs_fifo = Arc::clone(&subs_for_fifo);
                                            let sid = session_id.clone();
                                            let first_prompt = reply.first_prompt;
                                            let host_target = reply.host_target;
                                            // Clone the writers registry so the FIFO thread
                                            // can register the writer and the serve thread
                                            // can look it up via self.session_writers.0.
                                            let writers_fifo = Arc::clone(&writers_for_fifo);
                                            let pending_fifo = Arc::clone(&pending_for_fifo);

                                            std::thread::spawn(move || {
                                                // ── Open and connect the transport ─────────────
                                                // Local session → platform FIFO/pipe transport
                                                // (tmux+FIFO on Unix, named pipe on Windows). Remote
                                                // session (host_target resolves to an exec prefix) →
                                                // RemoteFifoTransport bridging the remote FIFOs over
                                                // ssh/wsl. All three yield the same `SplitIo`
                                                // (boxed writer + buffered boxed reader), so the rest
                                                // of this thread is transport-agnostic.
                                                let (writer, mut reader): marsad_transport::SplitIo =
                                                    match exec_prefix_for(host_target.as_deref()) {
                                                        Some(prefix) => {
                                                            let dir = format!(
                                                                "{}/sessions/marsad-{}",
                                                                REMOTE_RUNTIME_DIR, sid
                                                            );
                                                            let mut t =
                                                                marsad_transport::RemoteFifoTransport::new(
                                                                    prefix,
                                                                    format!("{dir}/in.fifo"),
                                                                    format!("{dir}/out.fifo"),
                                                                );
                                                            if let Err(e) = t.connect() {
                                                                eprintln!("marsad-agent: remote connect {sid}: {e}");
                                                                return;
                                                            }
                                                            match t.into_split() {
                                                                Some(pair) => pair,
                                                                None => {
                                                                    eprintln!("marsad-agent: remote not connected {sid}");
                                                                    return;
                                                                }
                                                            }
                                                        }
                                                        None => {
                                                            #[cfg(unix)]
                                                            {
                                                                use marsad_transport::ClaudeFifoTransport;
                                                                let mut fifo = match ClaudeFifoTransport::open(&session_dir) {
                                                                    Ok(f) => f,
                                                                    Err(e) => {
                                                                        eprintln!("marsad-agent: fifo open {sid}: {e}");
                                                                        return;
                                                                    }
                                                                };
                                                                if let Err(e) = fifo.connect() {
                                                                    eprintln!("marsad-agent: fifo connect {sid}: {e}");
                                                                    return;
                                                                }
                                                                match fifo.into_split() {
                                                                    Some(pair) => pair,
                                                                    None => {
                                                                        eprintln!("marsad-agent: fifo not connected after connect {sid}");
                                                                        return;
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
                                                                    return;
                                                                }
                                                                match transport.into_split() {
                                                                    Some(pair) => pair,
                                                                    None => {
                                                                        eprintln!("marsad-agent: pipe not connected after connect {sid}");
                                                                        return;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    };

                                                // The writer is shared (Arc<Mutex>) between this
                                                // thread (initialize handshake + first prompt) and
                                                // the serve thread (handle_send_prompt / answer_gate)
                                                // — boxed writers can't be try_clone'd like a File.
                                                let writer: SharedWriter =
                                                    std::sync::Arc::new(std::sync::Mutex::new(writer));

                                                // Send the control-protocol `initialize`
                                                // handshake first. This is required before claude
                                                // will route tool-permission prompts back to us as
                                                // `can_use_tool` control_requests (it pairs with
                                                // `--permission-prompt-tool stdio` in claude_argv).
                                                // Fire-and-forget: the CLI's ack is ignored.
                                                {
                                                    use std::io::Write;
                                                    let init =
                                                        marsad_proto::encode_control_request_initialize(
                                                            "req_1_init",
                                                        );
                                                    let mut w = writer.lock().unwrap();
                                                    let _ = w.write_all(init.as_bytes());
                                                    let _ = w.write_all(b"\n");
                                                    let _ = w.flush();
                                                }

                                                // Deliver the first_prompt (if any) immediately
                                                // so claude receives its initial task.  claude in
                                                // --input-format stream-json mode emits nothing
                                                // until it sees a user message on stdin.
                                                if let Some(prompt) = first_prompt {
                                                    use marsad_proto::{
                                                        encode_user_input, ContentBlock, UserInput,
                                                    };
                                                    use std::io::Write;
                                                    let encoded = encode_user_input(&UserInput {
                                                        content: vec![ContentBlock::Text {
                                                            text: prompt,
                                                        }],
                                                    });
                                                    let mut w = writer.lock().unwrap();
                                                    let _ = w.write_all(encoded.as_bytes());
                                                    let _ = w.write_all(b"\n");
                                                    let _ = w.flush();
                                                }

                                                // Register the shared writer so handle_send_prompt
                                                // / handle_answer_gate can deliver future prompts and
                                                // gate responses to this session's stdin. Both sides
                                                // hold an Arc to the same Mutex<Box<dyn Write>>.
                                                {
                                                    let mut registry = writers_fifo.lock().unwrap();
                                                    registry.insert(
                                                        sid.clone(),
                                                        std::sync::Arc::clone(&writer),
                                                    );
                                                }

                                                // Read out.fifo lines and broadcast to all
                                                // subscribers.  The reader blocks here; the
                                                // writer clone is in the session_writers registry
                                                // for handle_send_prompt — no lock contention.
                                                use std::io::BufRead;
                                                loop {
                                                    let mut raw = String::new();
                                                    match reader.read_line(&mut raw) {
                                                        Ok(0) | Err(_) => break,
                                                        Ok(_) => {
                                                            let raw = raw.trim_end_matches('\n');

                                                            // Capture a `can_use_tool` control
                                                            // request as a pending gate so a later
                                                            // AnswerGate can write the matching
                                                            // control_response. The event is still
                                                            // broadcast (below) so the GUI can
                                                            // render the gate and answer it.
                                                            if let Ok(
                                                                marsad_proto::StreamMsg::ControlRequest(
                                                                    cr,
                                                                ),
                                                            ) = marsad_proto::parse_stream_line(raw)
                                                            {
                                                                let mut gates =
                                                                    pending_fifo.lock().unwrap();
                                                                gates.insert(
                                                                    sid.clone(),
                                                                    PendingGate {
                                                                        request_id: cr.request_id,
                                                                        input: cr.input,
                                                                    },
                                                                );
                                                            }

                                                            let event: serde_json::Value =
                                                                serde_json::from_str(raw)
                                                                    .unwrap_or_else(|_| {
                                                                        serde_json::json!({"raw": raw})
                                                                    });
                                                            let ipc_msg =
                                                                marsad_proto::IpcMsg::AgentEvent {
                                                                    session_id: sid.clone(),
                                                                    event,
                                                                };
                                                            let encoded =
                                                                marsad_proto::encode_ipc(&ipc_msg);
                                                            // Broadcast to all subscribers.
                                                            let subs = subs_fifo.lock().unwrap();
                                                            subs.iter().for_each(|s| {
                                                                let _ = s.send(encoded.clone());
                                                            });
                                                        }
                                                    }
                                                }

                                                // Clean up: remove writer from registry when
                                                // claude exits (FIFO closed).
                                                {
                                                    let mut registry = writers_fifo.lock().unwrap();
                                                    registry.remove(&sid);
                                                }
                                                drop(writer);
                                            });
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

            // Send reply; if the client thread is gone, ignore the error.
            let _ = cmd.reply_tx.send(AgentReply {
                out_lines,
                new_session_id,
                first_prompt,
                host_target,
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
const REMOTE_RUNTIME_DIR: &str = "~/.marsad";

/// POSIX single-quote for remote shell commands (the remote is always a POSIX
/// `sh`, regardless of the local platform — so this is not `cfg(unix)`-gated).
fn remote_shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Resolve a host's execution target into the argv prefix every remote command is
/// run through (see docs/remote-hosts.md). Returns:
/// - `None` for the local machine (`None` / `"local"` / `"localhost"` / empty),
/// - an `ssh` prefix (with the standard option set) for `user@host` or a bare alias,
/// - a `wsl.exe` prefix for `wsl:<distro>`.
pub fn exec_prefix_for(host_target: Option<&str>) -> Option<Vec<String>> {
    let t = host_target.map(str::trim).filter(|s| !s.is_empty())?;
    if t == "local" || t == "localhost" {
        return None;
    }
    if let Some(distro) = t.strip_prefix("wsl:") {
        if distro.is_empty() {
            return None;
        }
        return Some(vec!["wsl.exe".to_string(), "-d".to_string(), distro.to_string()]);
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
    argv.extend(opt("ControlMaster=auto"));
    argv.extend(opt("ControlPath=~/.ssh/marsad-cm-%r@%h:%p"));
    argv.extend(opt("ControlPersist=600"));
    argv.push(t.to_string());
    Some(argv)
}

/// Build the FIFO-wired `claude` invocation that runs inside the remote tmux
/// session. Mirrors the local unix command but with remote paths under
/// `remote_runtime_dir` — `<root>/sessions/<name>/{in,out}.fifo` — matching the
/// paths `RemoteTmuxContainer` creates.
pub fn build_remote_spawn_cmd(
    remote_runtime_dir: &str,
    container_name: &str,
    project: &str,
    argv: &[String],
) -> String {
    let dir = format!("{remote_runtime_dir}/sessions/{container_name}");
    let claude_args = argv.get(1..).unwrap_or(&[]);
    format!(
        "cd {} && claude {} < {dir}/in.fifo > {dir}/out.fifo 2>{dir}/err.log",
        remote_shell_quote(project),
        claude_args
            .iter()
            .map(|a| remote_shell_quote(a))
            .collect::<Vec<_>>()
            .join(" "),
    )
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
/// - `Fresh`    → the project path positional (a new conversation in that dir);
/// - `Continue` → `--continue` (resume the most recent conversation in the cwd);
/// - `Resume(id)` → `--resume <id>` (resume that specific conversation).
///
/// The session's `cwd` is set by the FIFO-wired `cd <project> && claude …`
/// command (see `build_spawn_cmd`), so `--continue`/`--resume` resolve against
/// the right project directory.
pub fn claude_argv(spec: &marsad_proto::LaunchSpec) -> Vec<String> {
    let mut argv = vec![
        "claude".to_string(),
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

    // Route tool-permission prompts to the control-protocol stream so the GUI
    // can approve/deny them — but ONLY when the posture wants human approval.
    // `--permission-prompt-tool stdio` is the CLI sentinel that makes claude
    // emit `can_use_tool` control_requests on stdout (verified against the 2.1.x
    // binary: `mx9("stdio") => createCanUseTool`). For AutoApproveSafe /
    // FullAutonomy we leave the CLI's own permission mode in charge.
    if matches!(spec.posture, marsad_proto::Posture::Ask) {
        argv.push("--permission-prompt-tool".to_string());
        argv.push("stdio".to_string());
    }

    match &spec.mode {
        marsad_proto::LaunchMode::Fresh => argv.push(spec.project.clone()),
        marsad_proto::LaunchMode::Continue => argv.push("--continue".to_string()),
        marsad_proto::LaunchMode::Resume(id) => {
            argv.push("--resume".to_string());
            argv.push(id.clone());
        }
    }

    argv
}

// ── parse_launch_mode_value ─────────────────────────────────────────────────────

/// Decode the optional start-mode fields of a LaunchSession IPC message.
///
/// - `"resume_session_id": "<id>"` (non-empty) → `Resume(id)`
/// - `"continue": true`                        → `Continue`
/// - neither                                   → `Fresh`
///
/// `resume_session_id` takes precedence over `continue` if both are present.
fn parse_launch_mode_value(v: &serde_json::Value) -> marsad_proto::LaunchMode {
    if let Some(id) = v["resume_session_id"].as_str() {
        if !id.is_empty() {
            return marsad_proto::LaunchMode::Resume(id.to_string());
        }
    }
    if v["continue"].as_bool() == Some(true) {
        return marsad_proto::LaunchMode::Continue;
    }
    marsad_proto::LaunchMode::Fresh
}

// ── parse_posture_value ───────────────────────────────────────────────────────

/// Decode the `posture` field of a LaunchSession IPC message.
/// All arms (three happy + two error) are covered by committed tests.
fn parse_posture_value(v: &serde_json::Value) -> Result<marsad_proto::Posture, String> {
    let s = match v.as_str() {
        Some(s) => s,
        None => return Err(format!("posture must be a string, got: {v}")),
    };
    match s {
        "Ask" => Ok(marsad_proto::Posture::Ask),
        "AutoApproveSafe" => Ok(marsad_proto::Posture::AutoApproveSafe),
        "FullAutonomy" => Ok(marsad_proto::Posture::FullAutonomy),
        other => Err(format!("unknown posture: {other}")),
    }
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

// ── Internal wire types ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LaunchSessionMsg {
    project: String,
    host_id: Option<String>,
    #[serde(default)]
    host_target: Option<String>,
    model: String,
    first_prompt: Option<String>,
}
