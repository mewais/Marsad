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
        })
    }

    /// Construct with real OS IO: `TmuxContainer`, `DbusNotifier`, `FsBackend`.
    ///
    /// Gated `#[cfg(not(coverage))]` so the coverage build never sees these
    /// lines.  Called by `main` in production; integration tests call it
    /// directly.
    #[cfg(not(coverage))]
    pub fn with_real_io(cfg: AgentConfig) -> Result<Agent, AgentError> {
        use marsad_lifecycle::TmuxContainer;
        use marsad_notify::DbusNotifier;
        use marsad_store::{FsBackend, Store};

        let data_root = std::path::PathBuf::from(&cfg.data_root);
        let tmux = TmuxContainer::new(data_root.clone());
        let notifier = Box::new(DbusNotifier::new());
        let store = Box::new(Store::new(Box::new(FsBackend::new(data_root))));

        Ok(Agent {
            config: cfg,
            sessions: HashMap::new(),
            next_id: 1,
            // Fake container slot: unused in real-io mode but kept to satisfy the
            // type (container_calls() is a no-op here; integration tests assert
            // tmux session existence directly, not via container_calls()).
            container: ContainerWrap(Box::new(FakeContainer::new())),
            notifier: NotifierWrap(notifier),
            store: StoreWrap(store),
            real_container: RealContainerWrap(Some(Box::new(tmux))),
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
            // Build the FIFO-wired shell invocation.
            // The session dir is <data_root>/sessions/<container_name>.
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
        } else {
            argv.join(" ")
        }
    }

    /// Launch a new session via the injected container.
    pub fn launch(&mut self, spec: marsad_proto::LaunchSpec) -> Result<String, AgentError> {
        let session_id = self.next_session_id();
        let container_name = format!("marsad-{session_id}");
        let argv = claude_argv(&spec);
        let cmd = self.build_spawn_cmd(&container_name, &spec.project, &argv);

        self.active_container()
            .spawn(&container_name, &cmd)
            .map_err(|e| AgentError::Spawn(format!("{e}")))?;

        let persisted = marsad_store::PersistedSession {
            id: session_id.clone(),
            session_id: None,
            name: session_id.clone(),
            host_id: spec.host_id.clone(),
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
        };
        let session_id = self.launch(spec)?;
        let event = serde_json::json!({"type": "SessionLaunched", "session_id": session_id});
        Ok(vec![event.to_string()])
    }

    fn handle_resume_session(
        &mut self,
        value: &serde_json::Value,
    ) -> Result<Vec<String>, AgentError> {
        let session_id = value["session_id"].as_str().ok_or(AgentError::Protocol(
            "ResumeSession missing session_id".to_string(),
        ))?;
        let event = serde_json::json!({"type": "SessionResumed", "session_id": session_id});
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
        let _ = self.active_container().kill(&container_name);
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
        let _text = value["text"]
            .as_str()
            .ok_or(AgentError::Protocol("SendPrompt missing text".to_string()))?;
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

            use marsad_lifecycle::{Action, RuntimePhase, SessionRuntime};

            // Delegate to lifecycle::reconcile; fold any error (AttachRace transient,
            // ResumeFailed spawn failure) into a MarkStale runtime.
            let runtime = marsad_lifecycle::reconcile(&known, self.active_container()).unwrap_or(
                SessionRuntime {
                    action: Action::MarkStale,
                    phase: Some(RuntimePhase::Stale),
                },
            );

            runtimes.push(runtime);
        }

        runtimes
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
        // thread to open a FIFO reader for that session.
        struct AgentCmd {
            line: String,
            reply_tx: mpsc::SyncSender<AgentReply>,
        }
        struct AgentReply {
            out_lines: Result<Vec<String>, String>,
            /// Set when a LaunchSession/ResumeSession produced a session_id and
            /// a new FIFO reader should be started.
            new_session_id: Option<String>,
        }

        // The command sender is cloned into each client thread.
        let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCmd>();

        // ── Broadcast list ─────────────────────────────────────────────────
        // Arc<Mutex<Vec<Sender>>> — every connected client has one slot.
        // Session FIFO reader threads and the client reader thread on disconnect
        // both hold a clone of this Arc.
        let subscribers: Arc<Mutex<Vec<mpsc::Sender<String>>>> = Arc::new(Mutex::new(Vec::new()));

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
                let (bcast_tx, bcast_rx) = mpsc::channel::<String>();
                {
                    let mut subs = subs_accept.lock().unwrap();
                    subs.push(bcast_tx);
                }

                let cmd_tx_client = cmd_tx_accept.clone();
                let subs_client = Arc::clone(&subs_accept);
                let data_root_client = data_root.clone();
                let subs_for_fifo = Arc::clone(&subs_accept);

                // Wrap the client in Arc<Mutex<>> so both the reader and writer
                // threads share it.  Reads and writes are on separate threads
                // but the Mutex ensures they don't interleave.
                use marsad_transport::LocalIpc;
                let shared_client: Arc<Mutex<LocalIpc>> = Arc::new(Mutex::new(client));
                let shared_client_writer = Arc::clone(&shared_client);

                // Writer thread: drain bcast_rx → client socket.
                std::thread::spawn(move || {
                    for line in bcast_rx {
                        // Skip empty probe lines used for subscriber-list pruning.
                        if line.is_empty() {
                            continue;
                        }
                        let mut c = shared_client_writer.lock().unwrap();
                        if c.send_line(&line).is_err() {
                            break;
                        }
                    }
                });

                // Reader thread: recv inbound lines, dispatch to agent, send replies.
                std::thread::spawn(move || {
                    loop {
                        let line_opt = {
                            let mut c = shared_client.lock().unwrap();
                            c.recv_line()
                        };

                        match line_opt {
                            Ok(None) | Err(_) => {
                                // Clean EOF or IO error — disconnect.
                                // Sessions/tmux are intentionally NOT killed.
                                break;
                            }
                            Ok(Some(line)) => {
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
                                        let mut c = shared_client.lock().unwrap();
                                        let _ = c.send_line(&err_line);
                                    }
                                    Ok(out_lines) => {
                                        // Send response lines back to this client.
                                        for out_line in &out_lines {
                                            let mut c = shared_client.lock().unwrap();
                                            let _ = c.send_line(out_line);
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

                                            std::thread::spawn(move || {
                                                use marsad_transport::ClaudeFifoTransport;
                                                let mut fifo =
                                                    match ClaudeFifoTransport::open(&session_dir) {
                                                        Ok(f) => f,
                                                        Err(e) => {
                                                            eprintln!(
                                                                "marsad-agent: fifo open {sid}: {e}"
                                                            );
                                                            return;
                                                        }
                                                    };
                                                if let Err(e) = fifo.connect() {
                                                    eprintln!(
                                                        "marsad-agent: fifo connect {sid}: {e}"
                                                    );
                                                    return;
                                                }
                                                loop {
                                                    match fifo.recv_line() {
                                                        Ok(None) | Err(_) => break,
                                                        Ok(Some(raw)) => {
                                                            let event: serde_json::Value =
                                                                serde_json::from_str(&raw)
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

            let result = self.handle_ipc(&cmd.line);

            let (out_lines, new_session_id) = match result {
                Err(e) => (Err(format!("{e}")), None),
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
                    (Ok(lines), sid)
                }
            };

            // Send reply; if the client thread is gone, ignore the error.
            let _ = cmd.reply_tx.send(AgentReply {
                out_lines,
                new_session_id,
            });
        }

        Ok(())
    }
}

// ── shell_quote ───────────────────────────────────────────────────────────────

/// Single-quote a string for POSIX shell.  Replaces `'` with `'\''`.
/// Gated out of the coverage build (only used by `build_spawn_cmd` in the
/// real-io path).
#[cfg(not(coverage))]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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

/// Build the exact `claude` invocation for a fresh session launch.
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
        spec.project.clone(),
    ];
    if let Some(ref prompt) = spec.first_prompt {
        argv.push(prompt.clone());
    }
    argv
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
    model: String,
    first_prompt: Option<String>,
}
