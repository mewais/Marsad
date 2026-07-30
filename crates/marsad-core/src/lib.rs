//! marsad-core — facade tying B01–B11 into the single API the desktop (Tauri)
//! and phones (UniFFI) call.
//!
//! See `docs/specs/core.md` for the authoritative contract.
//!
//! UniFFI binding generation is deferred to the phone-app build; this crate is a
//! plain Rust library driven entirely by the unit-test suite over `LoopbackFake`.

use std::collections::HashMap;

use marsad_events::{reduce, Event, Phase, SessionState};
use marsad_gates::{DecisionLedger, GateDecision};
use marsad_hosts::{resolve_driver_bin, HostError, HostRegistry, SshRunner};
use marsad_proto::{encode_ipc, IpcMsg};
use marsad_store::{Backend, PersistedHost, Store, Workspace as StoreWorkspace};
use marsad_transport::Transport;
use marsad_viewmodel::{AppSnapshot, FileChange, Filter, SessionView, Tab, Workspace};
use thiserror::Error;

pub mod discovery;
mod agent_events;
mod catalog;
mod helpers;
mod sessions;
mod views;
mod workstreams;
use helpers::*;

// ---------------------------------------------------------------------------
// Public re-exports (spec requirement: callers use marsad_core::{LaunchSpec, Posture})
// ---------------------------------------------------------------------------

pub use discovery::SessionSummary;
pub use marsad_hosts::{Host, HostStatus};
pub use marsad_proto::{LaunchMode, LaunchSpec, Posture};
pub use marsad_store::{NotifPref, PersistedSettings, PersistedTemplate};

// ---------------------------------------------------------------------------
// CoreError
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("transport error: {0}")]
    Transport(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("unknown session: {0}")]
    UnknownSession(String),

    #[error("agent error: {0}")]
    Agent(String),

    #[error("host error: {0}")]
    Host(String),

    #[error("connection error: {0}")]
    Connection(String),

    /// X3 (docs/specs/wasl-connector.md §5.2): `answer_wasl_gate` was called
    /// for a `(host_id, gate_id)` no `WaslGateRequested` ever populated.
    #[error("unknown wasl gate: {0}")]
    UnknownWaslGate(String),
}

// ---------------------------------------------------------------------------
// HostHealthPayload — Default-able wrapper for host health snapshots
// ---------------------------------------------------------------------------

/// Payload for `VmUpdate::HostHealth`.  Implements `Default` so tests can
/// construct it with `Default::default()` without specifying a variant.
#[derive(Debug, Clone, Default)]
pub struct HostHealthPayload {
    pub host_id: String,
    pub status: Option<marsad_hosts::HostStatus>,
}

// ---------------------------------------------------------------------------
// VmUpdate
// ---------------------------------------------------------------------------

/// A single UI update emitted by `on_agent_line` or other state-changing calls.
///
/// `Session` is the large, common variant (it carries the full `SessionView`).
/// These updates are constructed and immediately consumed (emitted to the
/// webview), never stored in bulk, so boxing the hot-path variant would add an
/// allocation per event for no real benefit — we accept the size difference.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum VmUpdate {
    /// Overview-panel aggregates changed (counts, groups).
    Overview(marsad_viewmodel::OverviewGroups),

    /// One session's view-model changed.
    Session(SessionView),

    /// A session was removed entirely (deleted by the user) — the UI should drop
    /// its row. Distinct from `Session`, which only ever adds/updates.
    SessionRemoved { id: String },

    /// A toast notification.
    Toast { icon: String, text: String },

    /// Host health changed.
    HostHealth(HostHealthPayload),

    /// The full host list changed (added/removed/status updated).
    Hosts(Vec<Host>),

    /// The full workspace list changed (saved/duplicated/deleted/imported).
    Workspaces(Vec<Workspace>),

    /// The full workstream list changed (created/deleted/migrated).
    Workstreams(Vec<marsad_workstream::Workstream>),

    /// The workstreams board recomputed.
    Board(marsad_viewmodel::BoardView),

    /// One (driver, host) capability snapshot changed (U1 dynamic capability
    /// discovery, docs/specs/dynamic-capabilities.md §3.5).
    Capabilities(marsad_viewmodel::CapabilitiesView),

    /// X3 (docs/specs/wasl-connector.md §5.2): full pending Wasl-gate list
    /// (Hosts-style full-list semantics — idempotent to re-render,
    /// resync-friendly: duplicate `WaslGateRequested` collapses to the same
    /// list).
    WaslGates(Vec<WaslGateView>),
    /// One Wasl gate resolved (toast/history material). `outcome`/`decided_by`
    /// verbatim from the wire (incl. "unknown"/"resync" for lag-resync
    /// synthesis).
    WaslGateDecided {
        host_id: String,
        gate_id: String,
        outcome: String,
        decided_by: String,
        reason: Option<String>,
    },
    /// One Wasl observation-feed entry (typed; graph/feed material for X4).
    WaslObservation {
        host_id: String,
        entry: marsad_wasl::AuditEntry,
    },
}

// ---------------------------------------------------------------------------
// WaslGateView (X3, docs/specs/wasl-connector.md §5.2)
// ---------------------------------------------------------------------------

/// UI-shaped pending-gate row. `wasl_gates` keys on `(host_id, gate_id)` in a
/// `BTreeMap` so `VmUpdate::WaslGates` is deterministically ordered.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WaslGateView {
    pub host_id: String,
    pub gate_id: String,
    pub kind: String,
    pub from: String,
    pub summary: String,
    pub preview: String,
    pub requested_at: String,
    /// Authoritative for countdown UI (`timeout_ms` may be 0 on resync).
    pub deadline: String,
}

// ---------------------------------------------------------------------------
// NoopSsh — placeholder SshRunner for Core's HostRegistry
// ---------------------------------------------------------------------------

/// Core never runs SSH on its (locked) event path — health checks are performed
/// off the lock by the desktop and reported back via [`Core::set_host_status`].
/// The registry still requires a runner, so Core supplies this inert one.
struct NoopSsh;

impl SshRunner for NoopSsh {
    fn run(&self, _target: &str, _cmd: &str) -> Result<String, HostError> {
        Ok(String::new())
    }
}

// ---------------------------------------------------------------------------
// CoreConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub agent_endpoint: String,
    pub data_root: String,
}

// ---------------------------------------------------------------------------
// GitProbe — seam for reading a session's working-tree changes
// ---------------------------------------------------------------------------

/// Reads the working-tree changes in a session's project directory.
///
/// Injected into `Core` so the real (subprocess) implementation can be swapped
/// for a fake in tests — mirroring how `Transport` is injected.
pub trait GitProbe {
    /// Return the changed files under `project_dir` (empty if not a repo / on error).
    fn changes(&self, project_dir: &str) -> Vec<FileChange>;
}

/// Production `GitProbe` backed by the `git` CLI.
pub struct RealGit;

impl GitProbe for RealGit {
    // Under coverage builds the real subprocess path is compiled out (its
    // correctness is proven by the live i10 end-to-end test, not by `llvm-cov`).
    #[cfg(coverage)]
    fn changes(&self, _project_dir: &str) -> Vec<FileChange> {
        Vec::new()
    }

    #[cfg(not(coverage))]
    fn changes(&self, project_dir: &str) -> Vec<FileChange> {
        use std::process::Command;

        let status = match Command::new("git")
            .args(["-C", project_dir, "status", "--porcelain"])
            .output()
        {
            Ok(o) if o.status.success() => o.stdout,
            _ => return Vec::new(),
        };

        // Per-file added/removed for tracked changes (untracked → 0/0).
        let mut numstat: HashMap<String, (u32, u32)> = HashMap::new();
        if let Ok(o) = Command::new("git")
            .args(["-C", project_dir, "diff", "--numstat"])
            .output()
        {
            if o.status.success() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    let mut it = line.split('\t');
                    if let (Some(a), Some(d), Some(p)) = (it.next(), it.next(), it.next()) {
                        numstat.insert(
                            p.to_string(),
                            (a.parse().unwrap_or(0), d.parse().unwrap_or(0)),
                        );
                    }
                }
            }
        }

        let mut out = Vec::new();
        for line in String::from_utf8_lossy(&status).lines() {
            // Porcelain v1 lines are "XY <path>" (status code in cols 0..2).
            if line.len() < 4 {
                continue;
            }
            let status_code = line[..2].trim().to_string();
            let path = line[3..].trim().to_string();
            let (added, removed) = numstat.get(&path).copied().unwrap_or((0, 0));
            out.push(FileChange {
                path,
                status: status_code,
                added,
                removed,
            });
        }
        out
    }
}

// ---------------------------------------------------------------------------
// TranscriptReader — seam for reading a session's full conversation
// ---------------------------------------------------------------------------

/// Loads **prior history** for a resumed conversation from claude's authoritative
/// session JSONL. Used only at launch (for `Resume`/`Continue`); live turns are
/// built from the stream + injected prompts, so this never races claude's writes.
/// Injected so tests can supply a fake.
pub trait TranscriptReader {
    /// History of a specific claude session (resume-by-id).
    fn session(
        &self,
        project: &str,
        claude_session_id: &str,
    ) -> Vec<marsad_events::TranscriptEntry>;
    /// History of the most-recent session in `project` (continue-most-recent).
    fn most_recent(&self, project: &str) -> Vec<marsad_events::TranscriptEntry>;
    /// Aggregated per-model usage from a specific session's JSONL — used to seed
    /// token/cost totals for LOCAL sessions on attach. Default: empty (fakes
    /// need not override unless testing usage seeding specifically).
    fn session_usage(
        &self,
        _project: &str,
        _claude_session_id: &str,
    ) -> std::collections::HashMap<String, discovery::ModelUsageSummary> {
        std::collections::HashMap::new()
    }
}

/// Production `TranscriptReader`: reads `<claude config>/projects/<enc>/<id>.jsonl`.
pub struct RealTranscript;

impl RealTranscript {
    /// claude's projects root: `$CLAUDE_CONFIG_DIR/projects` if set, else
    /// `$HOME/.claude/projects`.
    #[cfg(not(coverage))]
    fn projects_root() -> Option<std::path::PathBuf> {
        if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
            if !dir.is_empty() {
                return Some(std::path::PathBuf::from(dir).join("projects"));
            }
        }
        std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .map(|h| std::path::PathBuf::from(h).join(".claude").join("projects"))
    }
}

// The real paths depend on the user's environment ($HOME / $CLAUDE_CONFIG_DIR) so
// they are excluded from coverage; the parsing they delegate to
// (`discovery::session_transcript`/`parse_transcript`/`list_sessions`) is
// unit-tested directly, and the end-to-end read is exercised by the live i10 test.
impl TranscriptReader for RealTranscript {
    #[cfg(coverage)]
    fn session(&self, _project: &str, _id: &str) -> Vec<marsad_events::TranscriptEntry> {
        Vec::new()
    }

    #[cfg(not(coverage))]
    fn session(&self, project: &str, id: &str) -> Vec<marsad_events::TranscriptEntry> {
        let Some(root) = Self::projects_root() else {
            return Vec::new();
        };
        discovery::session_transcript(&root, project, id).unwrap_or_default()
    }

    #[cfg(coverage)]
    fn most_recent(&self, _project: &str) -> Vec<marsad_events::TranscriptEntry> {
        Vec::new()
    }

    #[cfg(not(coverage))]
    fn most_recent(&self, project: &str) -> Vec<marsad_events::TranscriptEntry> {
        let Some(root) = Self::projects_root() else {
            return Vec::new();
        };
        let sessions = discovery::list_sessions(&root, project).unwrap_or_default();
        match sessions.first() {
            Some(s) => discovery::session_transcript(&root, project, &s.id).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    #[cfg(not(coverage))]
    fn session_usage(
        &self,
        project: &str,
        id: &str,
    ) -> std::collections::HashMap<String, discovery::ModelUsageSummary> {
        let Some(root) = Self::projects_root() else {
            return std::collections::HashMap::new();
        };
        discovery::session_usage(&root, project, id)
    }
}

// ---------------------------------------------------------------------------
// RuntimeOwner (X6 — Wasl Backend, wasl-backend-plan.md §6.1)
// ---------------------------------------------------------------------------

/// Who owns this session's underlying runtime.
///
/// `MarsadManaged` = today's model (the agent owns a claude/codex/opencode
/// process in a tmux/ConPTY container). `WaslManaged` = the NEW backend: the
/// process is Wasl's; Marsad is a thin client of a Wasl `DeliveryTarget`.
/// Defaults to `MarsadManaged` so every existing session and serialized record
/// is unchanged (load-bearing optionality, §2).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum RuntimeOwner {
    #[default]
    MarsadManaged,
    /// Carries the persisted binding so core can map Marsad-session-id ⇄
    /// Wasl-target-id, route prompt/attach/gate/interrupt/stop to the right
    /// client, AND reconnect to a still-alive Wasl session after a restart.
    WaslManaged { binding: WaslSessionBinding },
}

/// The persisted identity of a Wasl-managed session's underlying target
/// (§6.1). Persisted (via `marsad_store::PersistedWaslBinding`) because the Wasl
/// session's lifetime is independent of Marsad's process lifetime: if core/the
/// GUI restarts while the Wasl session is still alive, Marsad must reconnect to
/// it rather than losing track of it.
#[derive(Debug, Clone, PartialEq)]
pub struct WaslSessionBinding {
    /// Wasl-side stable target id (`target_id()`) — the handle to reattach to.
    pub target_id: String,
    /// The node the target lives on. Local-first this phase.
    pub host_id: String,
    /// The opaque Wasl target key used to re-obtain the client via
    /// `WaslSessionProvider::get_or_start` on reconnect. Relayed verbatim.
    pub provider_key: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Per-session record
// ---------------------------------------------------------------------------

struct SessionRecord {
    project: String,
    /// Host this session runs on (registry id). Surfaced in the SessionView so
    /// the UI labels the session with its real host instead of always "local".
    host_id: String,
    state: SessionState,
    /// Last-probed working-tree changes for this session's project dir.
    changes: Vec<FileChange>,
    /// Conversation transcript: resume history (loaded once at launch) + live
    /// assistant/tool entries (from the stream) + user prompts (injected on send).
    transcript: Vec<marsad_events::TranscriptEntry>,
    /// Keys of assistant content blocks already appended (dedup). claude streams
    /// ONE assistant message as MULTIPLE events sharing a message id (one per
    /// content block: text, tool_use, …), so we dedup per `<msg_id>\0<block>` —
    /// NOT per message id, which dropped every block after the first (live
    /// transcript looked frozen while the restart backfill showed the full reply).
    seen_msg_ids: std::collections::HashSet<String>,
    /// User-supplied display name. `None` → derived from the project basename.
    name: Option<String>,
    /// True while an async `!` shell command is executing off-thread (set by
    /// `inject_shell_command`, cleared by `inject_shell_result`). Surfaced in the
    /// SessionView so the UI shows a working indicator without freezing.
    shell_running: bool,
    /// Agent driver for this session ("claude" / "codex" / …). Stored so
    /// node_of() can expose the real driver on the workstream board instead of
    /// always reporting Claude.
    driver: String,
    /// Lifecycle liveness (two-axis model): `true` = Resumed (process alive on
    /// its host), `false` = Paused (not running, for ANY reason). Authoritative
    /// from the agent — NOT inferred from `state.phase`. Set true on launch and
    /// on any evidence of life (agent stream event / transcript backfill /
    /// `SessionLiveness{live:true}`); set false on stop and on
    /// `SessionLiveness{live:false}` (host reboot / OOM / crash / claude exit).
    /// Restored sessions start `false` (unknown until the agent confirms) so we
    /// never optimistically claim a session is running before proving it.
    live: bool,
    /// Per-session CONNECTION health (distinct from `live`): whether Marsad
    /// currently has a clean stream to the (remote) session. Driven by the agent's
    /// `SessionConn` events (real stream drop / re-attach / flap), NOT the periodic
    /// host poll. Used to HARD-BLOCK sending into a broken pipe and to show the true
    /// state. `live=false` (Paused) takes priority in the UI. Defaults to
    /// `Connected` (local sessions and until the agent says otherwise).
    conn_health: marsad_proto::ConnHealth,
    /// X6 (Wasl Backend, §6.1): who owns this session's runtime. Default
    /// `MarsadManaged` (today's model, unchanged); `WaslManaged` routes the
    /// dedicated `*Wasl*` IPC family and folds the Wasl observe stream.
    runtime_owner: RuntimeOwner,
    /// X6: the highest observe-stream `seq` folded for this WaslManaged session
    /// (the "last observed seq" of §6.1 step 2). On reconnect, `attach_session`
    /// re-issues `AttachWaslSession { after_seq = wasl_last_seq }` so the stream
    /// resumes with no gap and no dup. Persisted alongside the binding.
    wasl_last_seq: u64,
    /// X6: observe-stream seqs already folded, for dedup (a replay overlap after
    /// an `ObserveLost`/reattach must not double-fold — §10.4).
    wasl_seen_seqs: std::collections::HashSet<u64>,
    /// X6: transcript index of the currently-open (in-progress) Wasl assistant
    /// entry, so a run of `AssistantDelta`s APPENDS to the same entry rather than
    /// creating one per delta (§10.4). `None` between turns.
    wasl_open_assistant: Option<usize>,
    /// X6: whether the most recent Wasl turn has seen its terminal event
    /// (`TurnCompleted`/`TurnFailed`) with no newer `TurnAccepted` since. A delta
    /// arriving while this is `true` is a POST-TERMINAL stray → ignored, not
    /// folded (§10.4 defensive case). Cleared by the next `TurnAccepted`.
    wasl_turn_closed: bool,
    /// X6: pending native-gate details keyed by `gate_id`, so
    /// `answer_wasl_native_gate` can reconstruct the exact `Gate::Permission`
    /// (and thus the stable `DecisionId`) to record + relay.
    wasl_native_gates: HashMap<String, marsad_wasl_session::WaslNativeGateDetail>,
}

// ---------------------------------------------------------------------------
// Core
// ---------------------------------------------------------------------------

pub struct Core {
    _cfg: CoreConfig,
    transport: Box<dyn Transport>,
    git: Box<dyn GitProbe>,
    transcript: Box<dyn TranscriptReader>,
    hosts: HostRegistry,
    store: Store,
    session_order: Vec<String>,
    sessions: HashMap<String, SessionRecord>,
    ledger: DecisionLedger,
    active_ws: Option<String>,
    workspaces: Vec<Workspace>,
    workstreams: Vec<marsad_workstream::Workstream>,
    connections: Vec<marsad_workstream::Connection>,
    profiles: Vec<marsad_workstream::Profile>,
    membership: HashMap<String, String>,
    ws_leads: HashMap<String, String>,
    active_profile: Option<String>,
    /// U1 dynamic capability discovery: last known snapshot per (driver, host_id).
    capabilities: HashMap<(String, String), marsad_proto::Capabilities>,
    /// Per-host set of drivers whose capability discovery has LANDED since the
    /// host last came online. A host stays `Connecting` until every probed driver
    /// has reported (agents + versioned models loaded), then flips to `Connected`.
    /// Cleared when the host goes Unreachable so a reconnect re-discovers.
    caps_loaded: HashMap<String, std::collections::HashSet<String>>,
    /// Per-host marker that a FRESH capability discovery has been kicked off this
    /// online session. DISTINCT from `caps_loaded`, which is also populated by the
    /// persisted caps replayed from disk on startup — so on a restart `caps_loaded`
    /// looks "fully loaded" from stale data and would suppress re-probing. This
    /// marker is only set by an actual connect-time trigger, so first-Connected
    /// always re-probes (picking up a CLI installed since the last run), regardless
    /// of persisted caps. Cleared on Unreachable so a reconnect re-triggers.
    discovery_triggered: std::collections::HashSet<String>,
    /// X3 (docs/specs/wasl-connector.md §5.2): pending Wasl gates, keyed by
    /// (host_id, gate_id). Node-authoritative: entries are added by
    /// `WaslGateRequested` and removed ONLY by `WaslGateDecided` (incl.
    /// synthesized "unknown"/"resync" ones) — `answer_wasl_gate` does NOT
    /// optimistically remove.
    wasl_gates: std::collections::BTreeMap<(String, String), WaslGateView>,
}

/// Default persistence backend: real filesystem in production, in-memory under
/// the coverage build (where `FsBackend` is compiled out).
#[cfg(not(coverage))]
fn default_backend(data_root: &str) -> Box<dyn Backend> {
    Box::new(marsad_store::FsBackend::new(data_root))
}
#[cfg(coverage)]
fn default_backend(_data_root: &str) -> Box<dyn Backend> {
    Box::new(marsad_store::MemoryBackend::new())
}

impl Core {
    /// Create a new `Core`.  The transport is connected eagerly; if the
    /// underlying connect() call fails (e.g. broken socket) the error is
    /// silently ignored here — the first send will surface a BrokenPipe instead.
    /// This keeps `Core::new` infallible in all committed test scenarios.
    pub fn new(cfg: CoreConfig, transport: Box<dyn Transport>) -> Result<Core, CoreError> {
        let backend = default_backend(&cfg.data_root);
        Core::with_deps(
            cfg,
            transport,
            Box::new(RealGit),
            Box::new(RealTranscript),
            backend,
        )
    }

    /// Like [`Core::new`] but with an injected [`GitProbe`] (real transcript
    /// reader + default backend). Retained for tests that only need to fake git.
    pub fn with_git(
        cfg: CoreConfig,
        transport: Box<dyn Transport>,
        git: Box<dyn GitProbe>,
    ) -> Result<Core, CoreError> {
        let backend = default_backend(&cfg.data_root);
        Core::with_deps(cfg, transport, git, Box::new(RealTranscript), backend)
    }

    /// Like [`Core::new`] but with the git + transcript seams injected and the
    /// default persistence backend.
    pub fn with_probes(
        cfg: CoreConfig,
        transport: Box<dyn Transport>,
        git: Box<dyn GitProbe>,
        transcript: Box<dyn TranscriptReader>,
    ) -> Result<Core, CoreError> {
        let backend = default_backend(&cfg.data_root);
        Core::with_deps(cfg, transport, git, transcript, backend)
    }

    /// Full-injection constructor (git + transcript + persistence backend). Tests
    /// pass a `MemoryBackend` to exercise host persistence without filesystem IO.
    pub fn with_deps(
        cfg: CoreConfig,
        mut transport: Box<dyn Transport>,
        git: Box<dyn GitProbe>,
        transcript: Box<dyn TranscriptReader>,
        backend: Box<dyn Backend>,
    ) -> Result<Core, CoreError> {
        // Best-effort connect; ignore errors (send_line will detect them).
        let _ = transport.connect();
        Ok(Core {
            _cfg: cfg,
            transport,
            git,
            transcript,
            // Health checks run off the Core lock (desktop) → inert runner here.
            hosts: HostRegistry::new(Box::new(NoopSsh)),
            store: Store::new(backend),
            session_order: Vec::new(),
            sessions: HashMap::new(),
            ledger: DecisionLedger::new(),
            active_ws: None,
            workspaces: Vec::new(),
            workstreams: Vec::new(),
            connections: Vec::new(),
            profiles: Vec::new(),
            membership: HashMap::new(),
            ws_leads: HashMap::new(),
            active_profile: None,
            capabilities: HashMap::new(),
            caps_loaded: HashMap::new(),
            discovery_triggered: std::collections::HashSet::new(),
            wasl_gates: std::collections::BTreeMap::new(),
        })
    }

    // -----------------------------------------------------------------------
    // Snapshot
    // -----------------------------------------------------------------------

    pub fn snapshot(&self) -> AppSnapshot {
        let sessions: Vec<SessionView> = self
            .session_order
            .iter()
            .filter_map(|id| self.sessions.get(id).map(|r| self.make_session_view(id, r)))
            .collect();

        AppSnapshot {
            sessions,
            hosts: self.hosts.list().to_vec(),
            workspaces: self.workspaces.clone(),
            active_ws: self.active_ws.clone(),
            selected_id: None,
            filter: Filter::All,
            search: String::new(),
            tab: Tab::Activity,
        }
    }





    /// Whether a session's host runs on this machine (so a local `git` probe is
    /// valid). Remote hosts get their changes from the agent's `ChangesUpdate`.
    /// Unknown host id falls back to treating the bare `"local"`/empty id as local.
    fn host_is_local(&self, host_id: &str) -> bool {
        self.hosts
            .list()
            .iter()
            .find(|h| h.id == host_id)
            .map(|h| h.is_local)
            .unwrap_or(host_id == "local" || host_id.is_empty())
    }

    fn require_session(&self, id: &str) -> Result<(), CoreError> {
        if self.sessions.contains_key(id) {
            Ok(())
        } else {
            Err(CoreError::UnknownSession(id.to_string()))
        }
    }

    /// Send a raw line over the transport.
    fn send_raw_line(&mut self, line: &str) -> Result<(), CoreError> {
        self.transport
            .send_line(line)
            .map_err(|e| CoreError::Transport(e.to_string()))
    }

    /// Encode an `IpcMsg` and send it.
    fn send_ipc(&mut self, msg: &IpcMsg) -> Result<(), CoreError> {
        self.send_raw_line(&encode_ipc(msg))
    }

    fn make_session_view(&self, id: &str, record: &SessionRecord) -> SessionView {
        let s = &record.state;
        // Build a sorted Vec<ModelUsageRow> from the per-model map for determinism.
        let mut model_usage: Vec<marsad_viewmodel::ModelUsageRow> = s
            .model_usage
            .iter()
            .map(|(model, mu)| marsad_viewmodel::ModelUsageRow {
                model: model.clone(),
                input_tokens: mu.input_tokens,
                output_tokens: mu.output_tokens,
                cost_usd: mu.cost_usd,
                estimated: mu.estimated,
            })
            .collect();
        model_usage.sort_by(|a, b| a.model.cmp(&b.model));
        // U4 cost truthfulness: the session-level cost is "estimated" only when
        // there IS model_usage data and every row of it is estimated — i.e. no
        // live `result` event has ever landed for this session. A session with
        // no cost data at all (empty model_usage) is not "estimated", it's just
        // empty; it defaults to false per the wire contract.
        let cost_estimated =
            !s.model_usage.is_empty() && s.model_usage.values().all(|mu| mu.estimated);
        // U4 cost-tile consistency (review finding #4): when the session cost
        // is estimated, `s.cost_usd` is still 0.0 (it is set ONLY by a live
        // `result` event — `marsad_events::reduce_result` — which never ran
        // for a backfill-only session), yet the per-model `model_usage` rows
        // carry real, nonzero estimated dollars. Surfacing the raw 0.0 here
        // produced a "~ estimated $0.00" headline above a model-breakdown
        // table showing nonzero estimated dollars (a contradiction), and made
        // the aggregate "Cost today" tile — which filters/sums on
        // `cost_usd != 0` — silently drop such sessions. Report the summed
        // per-model estimated cost instead, so the headline equals the sum of
        // the breakdown rows below it. A LIVE session (`cost_estimated ==
        // false`) keeps its authoritative agent-reported `s.cost_usd`
        // verbatim — never overridden by a table recomputation (pinned by
        // `tests/u4_cost_truthfulness.rs::session_cost_usd_top_level_survives_backfill`).
        let cost_usd = if cost_estimated {
            model_usage.iter().map(|row| row.cost_usd).sum()
        } else {
            s.cost_usd
        };
        let posture = permission_mode_to_posture(s.permission_mode.as_deref());
        // Build subagent_list: running subagents first (for prominence), then done.
        let mut subagent_list: Vec<marsad_viewmodel::SubagentView> = s
            .subagents
            .iter()
            .map(|a| marsad_viewmodel::SubagentView {
                name: a.name.clone(),
                model: a.model.clone(),
                status: match a.status {
                    marsad_events::SubagentStatus::Running => "Running".to_string(),
                    marsad_events::SubagentStatus::Done => "Done".to_string(),
                },
            })
            .collect();
        // Sort: running first (they are most actionable), done last.
        subagent_list.sort_by_key(|a| if a.status == "Running" { 0u8 } else { 1u8 });
        SessionView {
            id: id.to_string(),
            name: record
                .name
                .clone()
                .unwrap_or_else(|| session_display_name(&record.project, id)),
            host_id: record.host_id.clone(),
            project: record.project.clone(),
            branch: String::new(),
            model: s.model.clone().unwrap_or_default(),
            // A running `!` shell command means the session is actively doing
            // work — surface it as Working even if claude's own phase is a
            // terminal state (Done/Stale), so the badge/board don't read "done"
            // while a bash command is still running. Cleared when the shell
            // output arrives (inject_shell_result clears shell_running).
            state: if record.shell_running {
                Phase::Working
            } else {
                s.phase.clone()
            },
            changed_files: record.changes.len() as u32,
            subagents: s.subagents.len() as u32,
            running_subagents: s
                .subagents
                .iter()
                .filter(|a| a.status == marsad_events::SubagentStatus::Running)
                .count() as u32,
            cost_usd,
            tokens: s.tokens,
            context_pct: s.context_pct,
            gate: s.gate.clone(),
            transcript: record.transcript.clone(),
            changes: record.changes.clone(),
            model_usage,
            cost_estimated,
            compactions: s.compactions,
            last_compaction: s.last_compaction.clone(),
            rate_limit: s.rate_limit.clone(),
            tools: s.tools.clone(),
            mcp_servers: s.mcp_servers.clone(),
            slash_commands: s.slash_commands.clone(),
            posture,
            subagent_list,
            shell_running: record.shell_running,
            driver: record.driver.clone(),
            live: record.live,
            conn_health: conn_health_str(record.conn_health),
            end_reason: s.end_reason,
            plan_entries: s.plan_entries.clone(),
            current_mode: s.current_mode.clone(),
            available_modes: s.available_modes.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Private free functions
// ---------------------------------------------------------------------------

#[cfg(test)]
mod noop_ssh_tests {
    use super::{NoopSsh, SshRunner};

    #[test]
    fn noop_ssh_run_is_inert() {
        // Core never invokes the registry's runner (health checks are external),
        // but the placeholder must remain a safe no-op.
        assert_eq!(NoopSsh.run("user@host", "echo ping").unwrap(), "");
    }
}
