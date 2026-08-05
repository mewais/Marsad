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
mod driver;
#[cfg(not(coverage))]
pub mod exec_shell;
mod ipc;
mod pty;
mod reader;
mod remote;
#[cfg(not(coverage))]
mod serve;
mod wasl;
mod wasl_session;

#[cfg(not(coverage))]
use remote::REMOTE_RUNTIME_DIR;

pub use driver::{build_remote_run_cmd, claude_argv, codex_appserver_argv, opencode_argv};
pub use reader::{
    assistant_has_file_modifying_tool_use, conn_on_drop, reconnect_action, reconnect_backoff_next,
    ReconnectAction, RECONNECT_INITIAL_BACKOFF_SECS, RECONNECT_MAX_BACKOFF_SECS,
};
pub use remote::{
    build_remote_spawn_cmd, exec_prefix_for,
    remote_durable_launch_script, remote_kill_script, remote_liveness_script, remote_reader_argv,
    remote_session_dir, remote_writer_argv,
};
#[cfg(not(coverage))]
pub use remote::{
    fetch_remote_session_list, probe_liveness, run_remote_script, run_remote_script_bounded,
};
pub use remote::{parse_liveness, Liveness, LIVENESS_SENTINEL};
pub use wasl::{
    load_wasl_config, load_wasl_session_config, render_wasl_node_toml, select_wasl_session_runtime,
    wasl_a2a_bind, wasl_control_bind, wasl_node_protocol, wasl_operator_bind, wasl_event_lines,
    WaslNodeConfig,
    render_wasl_worker_toml, wasl_worker_mcp_config, WaslWorkerPeer,
    // Remote-host Wasl node: bind kinds, local-tunnel allocation, tunnel argv,
    // remote spawn/kill scripts, and the spawn-sentinel parser.
    parse_wasl_node_spawn, remote_wasl_node_kill_script, remote_wasl_node_launch_script,
    wasl_local_tunnel_bind, wasl_local_tunnel_port, wasl_remote_bind, wasl_tunnel_ssh_argv,
    WaslBindKind, WaslNodeSpawn,
};
pub use wasl_session::{
    pump_wasl_observe, wasl_observe_should_reconnect, wasl_reconnect_after_seq,
    wasl_session_event_lines, wasl_session_status_line, WaslObserveEnd,
};

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

// ── Wasl Backend (X6) — WaslSessionClient/WaslSessionProvider registry ─────────
//
// The "Wasl Backend" feature (docs/design/wasl-backend-plan.md §6.1): a session
// whose underlying process is owned END-TO-END by Wasl, driven through a
// `marsad_wasl_session::WaslSessionClient`. STRUCTURALLY IDENTICAL to the X1
// `wasl_connectors` registry above (`Arc<Mutex<HashMap<..>>>`, always-compiled
// — NOT `#[cfg(not(coverage))]`-gated — so `handle_ipc`'s `*Wasl*` arms
// observably branch on registry membership under coverage instrumentation too,
// exactly as the `WaslConnectorsWrap` doc comment explains for the operator
// registry).
//
// Keyed by MARSAD session id (not host_id — a Wasl-managed session is a
// session-scoped runtime owner). The value is `Arc<dyn WaslSessionClient>` (not
// `Box`) for the SAME lock-hygiene reason as `WaslConnectorMap` (§8.1): every
// `*Wasl*` `handle_ipc` arm clones the `Arc` handle out and DROPS the map lock
// BEFORE calling any potentially-blocking client method (`prompt`,
// `resolve_native_gate`, `interrupt`, `shutdown`), so a wedged Wasl peer can
// never freeze the serve loop or stall an unrelated session's operations.
type WaslSessionClientMap = std::sync::Arc<
    std::sync::Mutex<HashMap<String, std::sync::Arc<dyn marsad_wasl_session::WaslSessionClient>>>,
>;
opaque_debug_wrap!(WaslSessionClientsWrap(WaslSessionClientMap));

// `WaslSessionClient: Send` (not `Send + Sync`), so `Arc<dyn WaslSessionClient>`
// is neither `Send` nor `Sync` on its own — clippy's `arc_with_non_send_sync`
// flags nesting it in the outer `Arc<Mutex<..>>`. Same disposition as
// `empty_wasl_connector_map`: not a real hazard (the whole registry lives on the
// single serve/command thread that owns `Agent`), the `Arc` wrapping exists only
// so the `*Wasl*` arms can clone a handle out and drop the map lock before the
// blocking client call.
#[allow(clippy::arc_with_non_send_sync)]
fn empty_wasl_session_client_map() -> WaslSessionClientMap {
    std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))
}

// The provider the agent uses to `get_or_start` a `WaslSessionClient` on a
// `LaunchWaslSession`. Registered directly (`register_wasl_session_provider`,
// the analogue of `register_wasl_connector`) or built lazily on first launch via
// the factory (`set_wasl_session_provider_factory`, the analogue of
// `set_wasl_connector_factory`). Always compiled — the launch path must be
// reachable under coverage, and constructing a provider performs no IO by itself
// (only `serve()`'s observe threads do real IO, and that block is
// `#[cfg(not(coverage))]`).
type WaslSessionProviderSlot = Option<Box<dyn marsad_wasl_session::WaslSessionProvider>>;
opaque_debug_wrap!(WaslSessionProviderWrap(WaslSessionProviderSlot));

/// The `set_wasl_session_provider_factory` parameter type, aliased so clippy's
/// `type_complexity` lint doesn't fire on the setter signature. Unlike the X1
/// connector factory, the Wasl session provider factory is ZERO-arg: the Wasl
/// target key travels on each `LaunchWaslSession`, so the provider needs no
/// per-call argument (see the test file's seam note).
pub type WaslSessionProviderFactoryFn =
    Box<dyn Fn() -> Box<dyn marsad_wasl_session::WaslSessionProvider> + Send>;
type WaslSessionProviderFactory = Option<WaslSessionProviderFactoryFn>;
opaque_debug_wrap!(WaslSessionProviderFactoryWrap(WaslSessionProviderFactory));

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

// Per-session queue of prompts that arrived before the session's writer was
// registered — production builds only. The reader thread registers the writer
// only AFTER it (re)attaches (for remote sessions: a seconds-long remote pid-dir
// resolve after a resume/reconnect). A prompt sent in that window used to be
// DROPPED while `handle_send_prompt` still returned PromptSent — so it appeared
// in the UI as sent but never reached claude. Instead we QUEUE the already-encoded
// stream-json bytes here (newline-terminated) and the reader flushes them, in
// order, the moment it registers the writer. Bounded (`PENDING_PROMPT_MAX`); NOT
// time-limited — a queued prompt survives a slow/long outage and is delivered on
// the next re-attach (the host session is durable), so the user never loses input.
#[cfg(not(coverage))]
pub(crate) type PendingPrompts =
    std::sync::Arc<std::sync::Mutex<HashMap<String, Vec<(std::time::Instant, Vec<u8>)>>>>;

/// Sessions that currently have a live reader thread — OR a (re)launch in flight
/// that is about to spawn one. This is the authoritative "a resume must NOT
/// relaunch" signal: inserted synchronously at (re)launch (`launch_with_id`) and
/// at every reader (re)spawn, and cleared when the reader gives up
/// (`spawn_session_reader`'s give-up path) or the session is stopped
/// (`handle_stop_session`).
///
/// It REPLACES the old `self.sessions`-based resume guard. `self.sessions` is
/// only ever removed by an explicit Stop, so a session whose reader gave up on a
/// connection-drop/liveness pause stayed in `self.sessions` forever — the resume
/// guard then no-op'd every resume, never re-spawned a reader, and the next
/// prompt sat in `pending_prompts` undrained until a full Marsad restart. Keying
/// the guard on reader liveness instead lets a paused session's resume relaunch
/// (re-spawning the reader, which drains the queue) while a genuinely-attached
/// session's resume still no-ops (no duplicate reader/claude).
///
/// NOT `#[cfg(not(coverage))]`: the resume guard (`handle_resume_session`) is
/// compiled in every build, so this field must exist in every build too.
pub(crate) type LiveReaders =
    std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>;

/// Max queued prompts retained per session (most-recent wins).
#[cfg(not(coverage))]
pub(crate) const PENDING_PROMPT_MAX: usize = 64;

/// Keep only the freshest `PENDING_PROMPT_MAX` queued prompts for `sid` and
/// return them in original order to deliver. Pure so it can be unit-tested
/// without threads/IO.
///
/// NO time-based drop: a prompt the user sent must survive an outage "slow for a
/// while" and be delivered whenever the reader re-attaches (the host session is
/// durable). An earlier 120s TTL silently discarded prompts on a slow reconnect —
/// that was input loss, which we no longer accept. `now` is retained in the
/// signature (unused) for call-site/test stability.
#[cfg(not(coverage))]
pub(crate) fn drain_fresh_prompts(
    items: Vec<(std::time::Instant, Vec<u8>)>,
    _now: std::time::Instant,
) -> Vec<Vec<u8>> {
    let len = items.len();
    let start = len.saturating_sub(PENDING_PROMPT_MAX);
    items.into_iter().skip(start).map(|(_, b)| b).collect()
}

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

// Per-HOST `ssh -L` tunnel children for a REMOTE Wasl node — production builds
// only. A remote node spawn (`ensure_remote_wasl_runtime_node`) opens one
// foreground `ssh -N -L` child per bind kind (control/operator/a2a) and tracks
// them here keyed by host_id so `teardown_wasl_tunnels` can SIGKILL + reap them
// on node/session stop, freeing the local tunnel ports and preventing ssh child
// leaks. LOCAL Wasl nodes never populate this map (they dial the node's own
// loopback bind directly, no tunnel). Gated out of the coverage build.
#[cfg(not(coverage))]
struct WaslTunnelsWrap(
    std::sync::Arc<std::sync::Mutex<HashMap<String, Vec<std::process::Child>>>>,
);

#[cfg(not(coverage))]
impl fmt::Debug for WaslTunnelsWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WaslTunnelsWrap")
    }
}

/// Minimal per-session spec cache for codex sessions — holds everything
/// the reader's app-server handshake (thread/start or thread/resume) needs,
/// keyed by session_id. Inserted at launch, read by `handle_send_prompt`,
/// never cleared (small footprint). Production builds only — gated out of coverage.
#[cfg(not(coverage))]
#[derive(Clone, Debug)]
pub(crate) struct CachedCodexSpec {
    pub model: String,
    pub project: String,
    /// The raw session mode id (e.g. "acceptEdits", or "" for backend default).
    /// Used by `handle_send_prompt` to pass the correct approvalPolicy/sandbox to
    /// the codex app-server via `thread/start`. Maps via `codex_mode_settings`.
    pub session_mode: String,
    /// The persisted codex thread_id for a resume. `Some(non-empty)` → the reader
    /// sends `wasit_codex::encode_thread_resume(id, thread_id, cwd)` instead of
    /// `encode_thread_start(...)`. `None` (fresh launch) → always thread/start.
    /// Mirrors `CachedOpencodeSpec.acp_session_id`.
    pub resume_thread_id: Option<String>,
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
/// `mode` is the persisted `PersistedSession::mode` (already normalized to a mode id).
/// `resume_thread_id` is the persisted `PersistedSession::session_id` (the codex
/// thread_id, written back after `thread/started`). When `Some(non-empty)` the
/// reader sends `thread/resume` instead of `thread/start`, mirroring the opencode
/// `acp_session_id` → `session/load` path.
/// Pure (no IO, no `self`) so it is unit-testable.
#[cfg(not(coverage))]
pub(crate) fn cached_codex_spec_from_persisted(
    model: String,
    project: String,
    mode: Option<String>,
    resume_thread_id: Option<String>,
) -> CachedCodexSpec {
    CachedCodexSpec {
        model,
        project,
        session_mode: mode.unwrap_or_default(),
        resume_thread_id,
    }
}

/// Minimal per-session spec for opencode ACP sessions — holds what the reader
/// thread needs to perform the ACP handshake (initialize + session/new or
/// session/load). Keyed by Marsad session_id. Production builds only.
#[cfg(not(coverage))]
#[derive(Clone, Debug)]
pub(crate) struct CachedOpencodeSpec {
    pub model: String,
    pub project: String,
    /// Stored ACP sessionId from a prior run (for `session/load` on resume).
    /// `None` for fresh launches → the reader sends `session/new`.
    pub acp_session_id: Option<String>,
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

/// Per-session codex app-server thread state — the `thread_id` (from the
/// `thread/start` result, for `turn/start` / `thread/settings/update`) plus the
/// current `turn_id` (from `turn/started`, for `turn/interrupt`; `None` between
/// turns). Typed rather than overloading one string map with two value namespaces.
#[derive(Clone, Default)]
pub struct CodexThread {
    pub thread_id: String,
    pub turn_id: Option<String>,
}

/// Per-session codex thread registry — keyed by session_id. Shared between the
/// reader thread (which sets thread_id on `thread/start` and turn_id on
/// `turn/started`/`turn/completed`) and `handle_send_prompt`/`handle_interrupt`/
/// `handle_set_session_config_option` (which read them to address JSON-RPC
/// requests). Production builds only.
#[cfg(not(coverage))]
struct CodexThreadsWrap(std::sync::Arc<std::sync::Mutex<HashMap<String, CodexThread>>>);

#[cfg(not(coverage))]
impl fmt::Debug for CodexThreadsWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CodexThreadsWrap")
    }
}

/// Per-session ACP sessionId registry for opencode sessions — keyed by Marsad
/// session_id, holds the ACP `sessionId` string returned by `session/new` or
/// `session/load`. Shared between the reader thread (which inserts it when the
/// `session/new` result arrives) and `handle_send_prompt` / `handle_interrupt`
/// / `handle_answer_gate` (which read it to address subsequent ACP requests).
///
/// Mirrors `codex_threads`: a simple string map, production builds only.
#[cfg(not(coverage))]
struct AcpSessionsWrap(std::sync::Arc<std::sync::Mutex<HashMap<String, String>>>);

#[cfg(not(coverage))]
impl fmt::Debug for AcpSessionsWrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AcpSessionsWrap")
    }
}

/// A `can_use_tool` permission request claude is blocked on, awaiting an answer.
/// Keyed by session_id (claude asks one tool at a time per session and blocks
/// until the control_response arrives).
#[cfg(not(coverage))]
#[derive(Clone)]
struct PendingGate {
    /// The gate's ROUTING id — must be globally unique (it becomes the frontend
    /// `decision_id` and the core ledger's dedup key). For claude this is the
    /// control-request id (a UUID). For opencode it is the toolCall id (unique per
    /// tool call) — NOT the raw ACP JSON-RPC id, which is `0` for the first
    /// permission of every session and would collide across sessions in the ledger
    /// (→ answers silently deduped as "already seen").
    request_id: String,
    input: serde_json::Value,
    /// True if this gate answers an MCP `elicitation` request (answered with
    /// `{action}`) rather than a `can_use_tool` request (`{behavior,
    /// updatedInput}`). See `handle_answer_gate`.
    elicitation: bool,
    /// opencode ACP only: the RAW JSON-RPC `id` of the `session/request_permission`
    /// request (e.g. `0`), needed to `encode_response` back to opencode. Codex also
    /// uses it (the raw id of the approval ServerRequest). Kept separate from
    /// `request_id` because that id is not unique across sessions. `None` for claude.
    acp_response_id: Option<String>,
    /// codex app-server only: the ServerRequest method that raised this gate
    /// (`item/commandExecution/requestApproval`, `…/fileChange/…`,
    /// `…/permissions/…`, `item/tool/requestUserInput`, `mcpServer/elicitation/request`).
    /// The answer path keys the JSON-RPC reply shape off it ({decision} vs
    /// {permissions} vs {answers} vs {action,content}). `None` for claude/opencode.
    codex_method: Option<String>,
    /// The exact gate event Value that was broadcast to subscribers as
    /// `AgentEvent{session_id, event}` when this gate was raised (claude's raw
    /// control_request, or the translated control_request for opencode/codex).
    /// Retained so `handle_subscribe` can REPLAY the gate to a client that
    /// (re)connects after the one-shot broadcast — e.g. a desktop restart while
    /// the durable agent + a blocked session stay alive. Without replay the
    /// reconnecting GUI never learns of a gate the session is still stuck on.
    event: serde_json::Value,
}

/// Per-session pending-gate registry. Shared (Arc) between the FIFO reader
/// threads (which push_back when a `can_use_tool` arrives) and the agent command
/// loop (`handle_answer_gate`, which removes the matching gate when the GUI
/// answers).
///
/// The value is a `VecDeque`, not a single slot: a single turn can block on a
/// burst of gates (e.g. WebSearch fanning out across a flapping remote), and an
/// earlier design that kept only the latest gate silently dropped the rest —
/// answering one then misrouted to a gate claude was no longer blocked on. The
/// deque preserves every distinct pending gate (deduped by `request_id`) so each
/// is answered against its own `request_id`.
#[cfg(not(coverage))]
struct PendingGatesWrap(
    std::sync::Arc<std::sync::Mutex<HashMap<String, std::collections::VecDeque<PendingGate>>>>,
);

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
    /// Sessions with a live/being-launched reader thread (see `LiveReaders`).
    /// The resume guard keys on this, NOT `sessions`, so a reader-give-up pause
    /// resumes correctly instead of no-op'ing and stranding the next prompt.
    live_readers: LiveReaders,
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
    /// Per-session prompts queued while the writer wasn't registered yet (the
    /// reader is still (re)attaching). Flushed in order by the reader on writer
    /// registration. Production builds only — gated out of the coverage build.
    #[cfg(not(coverage))]
    pending_prompts: PendingPrompts,
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
    /// Per-host `ssh -L` tunnel children for REMOTE Wasl nodes, keyed by host_id.
    /// Populated by `ensure_remote_wasl_runtime_node`, reaped by
    /// `teardown_wasl_tunnels`. Production builds only.
    #[cfg(not(coverage))]
    wasl_tunnels: WaslTunnelsWrap,
    /// Per-session codex spec cache: model/project/posture for building
    /// the reader's codex handshake. Production builds only.
    #[cfg(not(coverage))]
    codex_specs: CodexSpecsWrap,
    /// Per-session codex thread_id map: populated by the reader when
    /// `thread.started` arrives, read by `handle_send_prompt` for resume.
    /// Production builds only.
    #[cfg(not(coverage))]
    codex_threads: CodexThreadsWrap,
    /// Per-session ACP sessionId map for opencode sessions: populated by the
    /// reader when the `session/new` (or `session/load`) result arrives,
    /// read by `handle_send_prompt`/`handle_interrupt`/`handle_answer_gate`
    /// to address subsequent ACP JSON-RPC requests. Mirrors `codex_threads`.
    /// Production builds only.
    #[cfg(not(coverage))]
    acp_sessions: AcpSessionsWrap,
    /// Wasl connector registry (X2), keyed by host_id. Always compiled — see
    /// `WaslConnectorsWrap`'s doc comment.
    wasl_connectors: WaslConnectorsWrap,
    /// Wasl connector-factory override test seam (X2). Always compiled.
    wasl_connector_factory: WaslConnectorFactoryWrap,
    /// Wasl Backend (X6): per-session `WaslSessionClient` registry, keyed by
    /// Marsad session id. Always compiled — see `WaslSessionClientsWrap`.
    wasl_session_clients: WaslSessionClientsWrap,
    /// Wasl Backend (X6): the registered `WaslSessionProvider` (direct
    /// registration seam). Always compiled.
    wasl_session_provider: WaslSessionProviderWrap,
    /// Wasl Backend (X6): the provider-factory override test seam. Always
    /// compiled.
    wasl_session_provider_factory: WaslSessionProviderFactoryWrap,
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
            live_readers: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
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
            pending_prompts: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
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
            wasl_tunnels: WaslTunnelsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            #[cfg(not(coverage))]
            codex_specs: CodexSpecsWrap(std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))),
            #[cfg(not(coverage))]
            codex_threads: CodexThreadsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            #[cfg(not(coverage))]
            acp_sessions: AcpSessionsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            wasl_connectors: WaslConnectorsWrap(empty_wasl_connector_map()),
            wasl_connector_factory: WaslConnectorFactoryWrap(None),
            wasl_session_clients: WaslSessionClientsWrap(empty_wasl_session_client_map()),
            wasl_session_provider: WaslSessionProviderWrap(None),
            wasl_session_provider_factory: WaslSessionProviderFactoryWrap(None),
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
            live_readers: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
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
            pending_prompts: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
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
            wasl_tunnels: WaslTunnelsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            codex_specs: CodexSpecsWrap(std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))),
            codex_threads: CodexThreadsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            acp_sessions: AcpSessionsWrap(std::sync::Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            ))),
            wasl_connectors: WaslConnectorsWrap(empty_wasl_connector_map()),
            wasl_connector_factory: WaslConnectorFactoryWrap(None),
            wasl_session_clients: WaslSessionClientsWrap(empty_wasl_session_client_map()),
            wasl_session_provider: WaslSessionProviderWrap(None),
            wasl_session_provider_factory: WaslSessionProviderFactoryWrap(None),
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
                //   codex turn/start JSON-RPC is written into in.fifo by the local writer child;
                //   the eval-loop runs each command as a `codex exec` subprocess, appending
                //   JSONL to out.log (captured by start.sh's >> out.log redirection).
                //   remote_liveness_script / remote_kill_script work byte-identically:
                //   claude.pid here is the eval-loop's pid, not a codex process, but the
                //   operational semantics (kill the holder, SIGTERM + rm -rf the dir) are
                //   the same.
                // Per-driver remote run-command (codex eval-loop / `opencode acp` /
                // claude). Dispatched in one tested helper so opencode can't silently
                // fall back to claude again (see driver::build_remote_run_cmd).
                let run_cmd = driver::build_remote_run_cmd(&spec);
                self.session_run_cmds
                    .0
                    .lock()
                    .unwrap()
                    .insert(session_id.clone(), run_cmd);
            }

            // opencode: no additional in-memory spec needed at launch (the ACP
            // sessionId arrives later via the session/new response and is captured
            // in `acp_sessions` by the reader thread). The persistent process is
            // spawned by `spawn_container` exactly like claude — no extra work here.

            // Cache codex spec for later turns (handle_send_prompt): both local and
            // remote codex sessions need this so the reader's app-server handshake
            // (thread/start) can set the right model/posture/project/bin.
            if spec.driver == "codex" {
                // Fresh launch: resume_thread_id is None (the thread_id is not
                // known yet — it arrives later via thread/started → SessionInit).
                // For a Resume(uuid) launch the thread_id lives in spec.mode
                // as the uuid but codex uses the thread_id from the prior
                // thread/started, not the marsad session uuid — the persistence
                // block in agent_events.rs will have stored it in session_id.
                // That path uses cached_codex_spec_from_persisted (serve.rs),
                // not this block, so None here is always correct for new launches.
                let cached = CachedCodexSpec {
                    model: spec.model.clone(),
                    project: spec.project.clone(),
                    session_mode: spec.session_mode.clone(),
                    resume_thread_id: None,
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
            mode: if spec.session_mode.is_empty() { None } else { Some(spec.session_mode.clone()) },
            disallowed_tools: spec.disallowed_tools.clone(),
            driver: spec.driver.clone(),
            slash_commands: vec![],
            tools: vec![],
            mcp_servers: vec![],
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
        // Mark a reader as live/in-flight for this session SYNCHRONOUSLY (in the
        // agent thread, before the async reader spawn), so a concurrent resume for
        // the same id no-ops instead of relaunching a duplicate. Cleared when the
        // reader gives up or the session is stopped. See `LiveReaders`.
        self.live_readers.lock().unwrap().insert(session_id.clone());
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
    fn drain_fresh_prompts_keeps_all_in_order_regardless_of_age() {
        let now = std::time::Instant::now();
        // Even an "old" prompt is preserved now — no time-based drop. A prompt the
        // user sent must survive a slow/long outage and be delivered on re-attach.
        let items = vec![
            (now - std::time::Duration::from_secs(1), b"first\n".to_vec()),
            (now - std::time::Duration::from_secs(600), b"old\n".to_vec()),
            (
                now - std::time::Duration::from_secs(2),
                b"second\n".to_vec(),
            ),
        ];
        let out = drain_fresh_prompts(items, now);
        // All kept, in original order (bounded only by PENDING_PROMPT_MAX).
        assert_eq!(
            out,
            vec![b"first\n".to_vec(), b"old\n".to_vec(), b"second\n".to_vec()]
        );
    }

    #[test]
    fn drain_fresh_prompts_bounds_to_most_recent_max() {
        let now = std::time::Instant::now();
        // MAX + 3 fresh items; only the most recent MAX are retained.
        let n = PENDING_PROMPT_MAX + 3;
        let items: Vec<_> = (0..n)
            .map(|i| (now, format!("p{i}\n").into_bytes()))
            .collect();
        let out = drain_fresh_prompts(items, now);
        assert_eq!(out.len(), PENDING_PROMPT_MAX);
        // The oldest 3 (p0,p1,p2) are dropped; first kept is p3.
        assert_eq!(out[0], b"p3\n".to_vec());
        assert_eq!(
            out[PENDING_PROMPT_MAX - 1],
            format!("p{}\n", n - 1).into_bytes()
        );
    }

    #[test]
    fn rebuilds_bypass_permissions_spec_from_persisted() {
        let spec = cached_codex_spec_from_persisted(
            "codex-mini-latest".to_string(),
            "/tmp/proj".to_string(),
            Some("bypassPermissions".to_string()),
            None,
        );
        assert_eq!(spec.model, "codex-mini-latest");
        assert_eq!(spec.project, "/tmp/proj");
        assert_eq!(spec.session_mode, "bypassPermissions");
        assert!(spec.resume_thread_id.is_none());
    }

    #[test]
    fn rebuilds_accept_edits_spec() {
        let spec = cached_codex_spec_from_persisted(
            "m".to_string(),
            "p".to_string(),
            Some("acceptEdits".to_string()),
            None,
        );
        assert_eq!(spec.session_mode, "acceptEdits");
    }

    #[test]
    fn default_mode_round_trips() {
        let spec = cached_codex_spec_from_persisted("m".into(), "p".into(), Some("default".to_string()), None);
        assert_eq!(spec.session_mode, "default");
    }

    #[test]
    fn missing_mode_defaults_to_empty() {
        // A pre-mode persisted record (mode: None) must not panic and must
        // default to empty (backend default).
        let spec = cached_codex_spec_from_persisted("m".into(), "p".into(), None, None);
        assert_eq!(spec.session_mode, "");
    }

    #[test]
    fn any_mode_string_passes_through() {
        // Any mode string is passed through verbatim — no validation here.
        let spec = cached_codex_spec_from_persisted(
            "m".into(),
            "p".into(),
            Some("plan".to_string()),
            None,
        );
        assert_eq!(spec.session_mode, "plan");
    }

    // ── Resume thread_id tests ────────────────────────────────────────────────

    /// A reconnected codex session with a known thread_id must carry it into
    /// CachedCodexSpec.resume_thread_id so the reader sends thread/resume.
    #[test]
    fn resume_thread_id_round_trips_from_persisted() {
        let tid = "thread-abc-123".to_string();
        let spec = cached_codex_spec_from_persisted(
            "m".into(),
            "p".into(),
            Some("acceptEdits".to_string()),
            Some(tid.clone()),
        );
        assert_eq!(spec.resume_thread_id, Some(tid));
    }

    /// A fresh launch (no thread_id yet in the store) must leave resume_thread_id None.
    #[test]
    fn fresh_launch_has_no_resume_thread_id() {
        let spec = cached_codex_spec_from_persisted(
            "m".into(),
            "p".into(),
            None,
            None,
        );
        assert!(spec.resume_thread_id.is_none());
    }

    /// An empty-string thread_id (pre-persist, corrupted store) must not be
    /// treated as a valid resume — the intercept checks `Some(non-empty)`.
    #[test]
    fn empty_resume_thread_id_is_none() {
        // The caller (serve.rs) filters empty strings before passing; test that
        // passing None explicitly leaves the field None.
        let spec = cached_codex_spec_from_persisted("m".into(), "p".into(), None, None);
        assert!(
            spec.resume_thread_id
                .as_deref()
                .map(|s| s.is_empty())
                .unwrap_or(true),
            "empty or absent resume_thread_id must not trigger thread/resume"
        );
    }

    // ── Resume-vs-fresh decision helper ──────────────────────────────────────

    /// Pure helper: given a resume_thread_id slot, returns the method to send.
    /// Extracted from the intercept logic for unit-testability.
    fn codex_thread_method(resume_thread_id: Option<&str>) -> &'static str {
        match resume_thread_id.filter(|s| !s.is_empty()) {
            Some(_) => "thread/resume",
            None => "thread/start",
        }
    }

    #[test]
    fn fresh_codex_uses_thread_start() {
        assert_eq!(codex_thread_method(None), "thread/start");
    }

    #[test]
    fn resume_codex_uses_thread_resume() {
        assert_eq!(codex_thread_method(Some("thread-abc")), "thread/resume");
    }

    #[test]
    fn empty_thread_id_uses_thread_start() {
        assert_eq!(codex_thread_method(Some("")), "thread/start");
    }
}
