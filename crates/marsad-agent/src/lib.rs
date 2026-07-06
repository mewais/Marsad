//! marsad-agent — detached supervisor daemon.
//!
//! See `../AGENTS.md` and `docs/specs/agent.md` for the authoritative contract.
//!
//! The crate is split into: this file (config/error types + the `Agent` struct
//! itself), `driver` (build the `claude` invocation and drive the session
//! container), `ipc` (decode + dispatch inbound IPC), `reader` (the per-session
//! NDJSON pump/broadcast thread), `remote` (SSH/WSL/plink machinery), and
//! `serve` (the blocking real-IO event loop). See `AGENTS.md` for the map.

use std::collections::HashMap;
use std::fmt;

use marsad_lifecycle::{FakeContainer, Persisted};
use marsad_notify::{CaptureNotifier, Notifier};
use marsad_store::{MemoryBackend, Store};

pub mod capabilities;
pub mod codex;
mod driver;
mod ipc;
mod pty;
mod reader;
mod remote;
#[cfg(not(coverage))]
mod serve;
mod wasl;

#[cfg(not(coverage))]
use remote::REMOTE_RUNTIME_DIR;

pub use driver::{claude_argv, posture_to_mode};
pub use reader::{
    assistant_has_file_modifying_tool_use, reconnect_backoff_next, RECONNECT_INITIAL_BACKOFF_SECS,
    RECONNECT_MAX_BACKOFF_SECS, RECONNECT_MAX_TOTAL_SECS, RECONNECT_PROBE_TIMEOUT_SECS,
};
pub use remote::{
    build_remote_codex_eval_loop, build_remote_spawn_cmd, exec_prefix_for,
    remote_durable_launch_script, remote_kill_script, remote_liveness_script, remote_reader_argv,
    remote_session_dir, remote_writer_argv,
};
#[cfg(not(coverage))]
pub use remote::{fetch_remote_session_list, run_remote_script, run_remote_script_bounded};
pub use wasl::{load_wasl_config, wasl_event_lines};

// WAVE 3b — pty escape-hatch terminal. `default_shell`/`pty_exec_argv` are
// pure and always compiled; the real spawn/resize API is production-only
// (real OS IO), re-exported so `crates/marsad-agent/tests/pty.rs` can drive
// it directly under plain `cargo test` (see `pty.rs`'s module doc).
pub use pty::{default_shell, pty_exec_argv, shell_for};
#[cfg(not(coverage))]
pub use pty::{resize_pty, spawn_pty, PtyEntry, PtyError};

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

// X2 Wasl connector registry (docs/specs/wasl-connector.md §4.2/§4.3) — keyed
// by host_id. Unlike the other real-IO registries below (session_writers,
// pending_gates, ...), this is NOT `#[cfg(not(coverage))]`-gated: the
// `AnswerWaslGate` arm in `handle_ipc` must observably branch on registry
// membership (present vs. absent) under coverage instrumentation too, and
// `register_wasl_connector` is a plain always-compiled test seam (see
// `wasl.rs`). Constructing/registering a connector performs no IO by itself
// (`WaslOperatorClient::new` is a pure struct literal) — only `start()` does,
// and that is only ever invoked from `serve()`'s `#[cfg(not(coverage))]` block.
//
// Type aliases (rather than inlining these into the `opaque_debug_wrap!`
// call) so `cargo fmt` can't insert a trailing comma inside the macro's
// `$inner:ty` capture, which breaks the `)`-terminated matcher.
// Values are `Arc<dyn WaslConnector>`, not `Box<dyn WaslConnector>` — X4.3
// hardening (review finding #1): `handle_answer_wasl_gate` must be able to
// clone the connector handle out from under the registry lock and drop the
// lock BEFORE calling the (blocking, network-bound) `decide()` method, so a
// slow/wedged operator can never hold up any other user of this registry.
// `register_wasl_connector` converts the `Box` callers still pass in
// (`Arc::from`) at insertion time; `start()`/`stop()` (which need `&mut
// self` on the connector) are only ever called on the owned `Box` BEFORE it
// is registered (see `serve.rs`'s wiring block), so nothing here ever needs
// mutable access through the `Arc`.
type WaslConnectorMap = std::sync::Arc<
    std::sync::Mutex<HashMap<String, std::sync::Arc<dyn marsad_wasl::WaslConnector>>>,
>;
opaque_debug_wrap!(WaslConnectorsWrap(WaslConnectorMap));

// `WaslConnector: Send` (not `Send + Sync` — the trait is defined in
// marsad-wasl, out of this crate's edit scope, and widening it would ripple
// into the real `WaslOperatorClient`/`FakeWaslConnector` impls for no
// behavioral benefit), so `dyn WaslConnector` isn't `Sync`, which makes
// `Arc<dyn WaslConnector>` neither `Send` nor `Sync` on its own — clippy's
// `arc_with_non_send_sync` flags nesting it inside the (Send+Sync) outer
// `Arc<Mutex<..>>`. That's a correct observation about the TYPE in the
// abstract, but not a real hazard here: this whole registry only ever lives
// on and is touched from the single serve/command thread that owns `Agent`
// (see `serve.rs`'s "Concurrency model" doc comment — `Agent` itself is
// deliberately never shared across a real OS thread boundary); the `Arc`
// wrapping exists purely so `handle_answer_wasl_gate` can clone a handle out
// and drop the map lock before calling the connector's blocking `decide()`,
// not for cross-thread sharing of connector instances.
#[allow(clippy::arc_with_non_send_sync)]
fn empty_wasl_connector_map() -> WaslConnectorMap {
    std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))
}

// Optional connector-factory override (the `set_wasl_connector_factory` test
// seam, §4.2). `None` = use the real-client factory, which `serve()` supplies
// lazily (only reachable from `#[cfg(not(coverage))]` code, since
// `WaslOperatorClient`'s `WaslConnector` impl itself is `cfg(not(coverage))`
// in marsad-wasl). Storing the closure here is not IO.
type WaslConnectorFactory = Option<wasl::WaslConnectorFactoryFn>;
opaque_debug_wrap!(WaslConnectorFactoryWrap(WaslConnectorFactory));

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

/// Minimal per-session spec cache for codex sessions — holds everything
/// `codex_turn_cmd` needs to build follow-up turn commands, keyed by session_id.
/// Inserted at launch, read by `handle_send_prompt`, never cleared (small footprint).
/// Production builds only — gated out of coverage.
#[cfg(not(coverage))]
#[derive(Clone, Debug)]
pub(crate) struct CachedCodexSpec {
    pub model: String,
    pub project: String,
    pub posture_mode: String,
    pub claude_bin: Option<String>,
    pub posture: marsad_proto::Posture,
}

/// Rebuild a `CachedCodexSpec` from the persisted fields of a codex session.
///
/// On a process restart the in-memory `codex_specs` map (populated at launch in
/// `launch_with_id`) is empty, so a reconnected codex session cannot build a
/// follow-up/resume turn command in `handle_send_prompt` — the prompt is
/// silently DROPPED. The reconnect path in `serve.rs` calls this to reconstruct
/// the spec from the persisted `PersistedSession` record (the same fields
/// `launch_with_id` cached) so codex sessions can take new prompts after a
/// restart.
///
/// `posture_str` is the persisted `PersistedSession::posture` string (`"Ask"` /
/// `"AutoApproveSafe"` / `"FullAutonomy"`); it is parsed the same way `handle_resume`
/// parses it (serde on the string), defaulting to `Ask` for missing/unknown
/// values. Pure (no IO, no `self`) so it is unit-testable.
#[cfg(not(coverage))]
pub(crate) fn cached_codex_spec_from_persisted(
    model: String,
    project: String,
    claude_bin: Option<String>,
    posture_str: Option<&str>,
) -> CachedCodexSpec {
    let posture = posture_str
        .and_then(|s| {
            serde_json::from_value::<marsad_proto::Posture>(serde_json::Value::String(
                s.to_string(),
            ))
            .ok()
        })
        .unwrap_or(marsad_proto::Posture::Ask);
    CachedCodexSpec {
        model,
        project,
        posture_mode: driver::posture_to_mode(&posture).to_string(),
        claude_bin,
        posture,
    }
}

/// Per-session codex spec cache wrapper (production builds only).
#[cfg(not(coverage))]
struct CodexSpecsWrap(std::sync::Arc<std::sync::Mutex<HashMap<String, CachedCodexSpec>>>);

#[cfg(not(coverage))]
impl fmt::Debug for CodexSpecsWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CodexSpecsWrap")
    }
}

/// Per-session codex thread_id registry — keyed by session_id, holds the
/// `thread_id` from codex's `thread.started` event so subsequent turns can
/// `codex exec resume <id>` instead of fresh. Shared between the FIFO reader
/// thread (which inserts the thread_id when it arrives) and `handle_send_prompt`
/// (which reads it to build the turn command). Production builds only.
#[cfg(not(coverage))]
struct CodexThreadsWrap(std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>);

#[cfg(not(coverage))]
impl fmt::Debug for CodexThreadsWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CodexThreadsWrap")
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

// WAVE 3b per-session pty registry wrapper — production builds only. Gated
// out of the coverage build so no uncovered lines appear (mirrors
// `SessionWritersWrap`). See `pty.rs`'s `PtyRegistry` doc comment.
#[cfg(not(coverage))]
struct PtyEntriesWrap(pty::PtyRegistry);

#[cfg(not(coverage))]
impl fmt::Debug for PtyEntriesWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PtyEntriesWrap")
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
    /// WAVE 3b: per-session live pty registry (the Terminal tab's escape
    /// hatch). Production builds only — gated out of the coverage build.
    #[cfg(not(coverage))]
    pty_entries: PtyEntriesWrap,
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
    /// Per-session codex spec cache: model/project/posture for building
    /// `codex_turn_cmd` in `handle_send_prompt`. Production builds only.
    #[cfg(not(coverage))]
    codex_specs: CodexSpecsWrap,
    /// Per-session codex thread_id map: populated by the reader when
    /// `thread.started` arrives, read by `handle_send_prompt` for resume.
    /// Production builds only.
    #[cfg(not(coverage))]
    codex_threads: CodexThreadsWrap,
    /// Wasl connector registry (X2), keyed by host_id. Always compiled — see
    /// `WaslConnectorsWrap`'s doc comment.
    wasl_connectors: WaslConnectorsWrap,
    /// Wasl connector-factory override test seam (X2). Always compiled.
    wasl_connector_factory: WaslConnectorFactoryWrap,
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
            pty_entries: PtyEntriesWrap(std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))),
            #[cfg(not(coverage))]
            session_children: SessionChildrenWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            #[cfg(not(coverage))]
            session_run_cmds: SessionRunCmdsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            #[cfg(not(coverage))]
            codex_specs: CodexSpecsWrap(std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))),
            #[cfg(not(coverage))]
            codex_threads: CodexThreadsWrap(std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))),
            wasl_connectors: WaslConnectorsWrap(empty_wasl_connector_map()),
            wasl_connector_factory: WaslConnectorFactoryWrap(None),
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
            pty_entries: PtyEntriesWrap(std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))),
            session_children: SessionChildrenWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            session_run_cmds: SessionRunCmdsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            codex_specs: CodexSpecsWrap(std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))),
            codex_threads: CodexThreadsWrap(std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))),
            wasl_connectors: WaslConnectorsWrap(empty_wasl_connector_map()),
            wasl_connector_factory: WaslConnectorFactoryWrap(None),
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
        // X4 hardening (review finding #2): a `wasl:true` REMOTE launch has
        // no way to resolve the `--mcp-config` placeholder `claude_argv`
        // injects — `driver::materialize_wasl_mcp_config` only rewrites it
        // to a real path for LOCAL sessions (remote hosts keep the
        // unresolved `wasl-mcp.json` literal, a documented gap). Launching
        // anyway would hand claude a broken `--mcp-config wasl-mcp.json`
        // argument pointing at a file that is never written. Fail fast with
        // a clear, honest error instead of a silently-broken remote launch.
        if spec.wasl && exec_prefix_for(spec.host_target.as_deref()).is_some() {
            return Err(AgentError::Protocol(format!(
                "wasl:true is not supported for remote launches yet (host_target={:?}): \
                 the Wasl MCP config can only be resolved for local sessions today",
                spec.host_target
            )));
        }

        // X4b (docs/specs/wasl-connector.md §X4b): a `wasl:true` LOCAL
        // launch needs the host's `wasl-mcpd` receive node up (adopted if
        // already live, spawned otherwise) — the very node the injected
        // `--mcp-config` points claude at. No-op (zero container calls)
        // when `spec.wasl` is false, per `adopt_or_spawn_wasl_mcpd`'s own
        // guard.
        self.adopt_or_spawn_wasl_mcpd(&spec);

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
                // For a REMOTE session, compute the run-command body now — this is the
                // only point with the full LaunchSpec (posture + mode + setup + env) in hand.
                //
                // - claude: `cd '<proj>' && exec '<bin>' <args>` — run via a login shell
                //   with FIFO wired by start.sh (design 2B).
                // - codex: the eval-loop body `cd '<proj>' && while IFS= read -r __cmd; do
                //   eval "$__cmd" </dev/null; done` — the long-lived process whose stdin is
                //   in.fifo (wired by start.sh) and whose stdout is out.log. Per-turn
                //   codex_turn_cmd output is written into in.fifo by the local writer child;
                //   the eval-loop runs each command as a `codex exec` subprocess, appending
                //   JSONL to out.log (captured by start.sh's >> out.log redirection).
                //   remote_liveness_script / remote_kill_script work byte-identically:
                //   claude.pid here is the eval-loop's pid, not a codex process, but the
                //   operational semantics (kill the holder, SIGTERM + rm -rf the dir) are
                //   the same.
                let run_cmd = if spec.driver == "codex" {
                    build_remote_codex_eval_loop(
                        &spec.project,
                        spec.setup.as_deref(),
                        &spec.env,
                    )
                } else {
                    build_remote_spawn_cmd(
                        &spec.project,
                        &claude_argv(&spec),
                        spec.setup.as_deref(),
                        &spec.env,
                    )
                };
                self.session_run_cmds
                    .0
                    .lock()
                    .unwrap()
                    .insert(session_id.clone(), run_cmd);
            }

            // Cache codex spec for later turns (handle_send_prompt): both local and
            // remote codex sessions need this so follow-up turns can build
            // codex_turn_cmd with the right model/posture/project/bin.
            if spec.driver == "codex" {
                let cached = CachedCodexSpec {
                    model: spec.model.clone(),
                    project: spec.project.clone(),
                    posture_mode: driver::posture_to_mode(&spec.posture).to_string(),
                    claude_bin: spec.claude_bin.clone(),
                    posture: spec.posture.clone(),
                };
                self.codex_specs
                    .0
                    .lock()
                    .unwrap()
                    .insert(session_id.clone(), cached);
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
            driver: spec.driver.clone(),
            slash_commands: vec![],
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
}

// ---------------------------------------------------------------------------
// Unit tests for the pure reconnect helpers (production build only — the
// helpers they cover are `#[cfg(not(coverage))]`, so these run under the
// normal `cargo test`, not the coverage build).
// ---------------------------------------------------------------------------
#[cfg(all(test, not(coverage)))]
mod reconnect_spec_tests {
    use super::*;

    #[test]
    fn rebuilds_full_autonomy_spec_from_persisted() {
        let spec = cached_codex_spec_from_persisted(
            "codex-mini-latest".to_string(),
            "/tmp/proj".to_string(),
            Some("codex".to_string()),
            Some("FullAutonomy"),
        );
        assert_eq!(spec.model, "codex-mini-latest");
        assert_eq!(spec.project, "/tmp/proj");
        assert_eq!(spec.claude_bin.as_deref(), Some("codex"));
        assert_eq!(spec.posture, marsad_proto::Posture::FullAutonomy);
        // posture_mode is the codex/claude permission-mode string.
        assert_eq!(spec.posture_mode, driver::posture_to_mode(&spec.posture));
        assert_eq!(spec.posture_mode, "bypassPermissions");
    }

    #[test]
    fn rebuilds_auto_approve_safe_spec() {
        let spec = cached_codex_spec_from_persisted(
            "m".to_string(),
            "p".to_string(),
            None,
            Some("AutoApproveSafe"),
        );
        assert_eq!(spec.posture, marsad_proto::Posture::AutoApproveSafe);
        assert_eq!(spec.posture_mode, "acceptEdits");
        assert!(spec.claude_bin.is_none());
    }

    #[test]
    fn ask_posture_round_trips() {
        let spec =
            cached_codex_spec_from_persisted("m".into(), "p".into(), None, Some("Ask"));
        assert_eq!(spec.posture, marsad_proto::Posture::Ask);
        assert_eq!(spec.posture_mode, "default");
    }

    #[test]
    fn missing_posture_defaults_to_ask() {
        // A pre-posture persisted record (posture: None) must not panic and must
        // default to Ask — mirroring how `handle_resume` parses the field.
        let spec = cached_codex_spec_from_persisted("m".into(), "p".into(), None, None);
        assert_eq!(spec.posture, marsad_proto::Posture::Ask);
        assert_eq!(spec.posture_mode, "default");
    }

    #[test]
    fn unknown_posture_string_defaults_to_ask() {
        let spec = cached_codex_spec_from_persisted(
            "m".into(),
            "p".into(),
            None,
            Some("NotARealPosture"),
        );
        assert_eq!(spec.posture, marsad_proto::Posture::Ask);
    }
}
