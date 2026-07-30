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

    // -----------------------------------------------------------------------
    // Workspace helpers
    // -----------------------------------------------------------------------

    fn store_ws_to_vm(ws: &StoreWorkspace) -> Workspace {
        Workspace {
            id: ws.id.clone(),
            name: ws.name.clone(),
            session_ids: ws.session_ids.clone(),
            saved_at: ws.saved_at.clone(),
        }
    }

    fn vm_ws_to_store(ws: &Workspace) -> StoreWorkspace {
        StoreWorkspace {
            id: ws.id.clone(),
            name: ws.name.clone(),
            session_ids: ws.session_ids.clone(),
            saved_at: ws.saved_at.clone(),
        }
    }

    fn workspaces_update(&self) -> VmUpdate {
        VmUpdate::Workspaces(self.workspaces.clone())
    }

    // -----------------------------------------------------------------------
    // Host commands
    // -----------------------------------------------------------------------

    /// Load persisted hosts from the store into the registry (call once at
    /// startup). Returns a `Hosts` update for the now-complete list.
    pub fn load_hosts(&mut self) -> Result<Vec<VmUpdate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Host(format!("restore hosts: {e:?}")))?;
        for h in restored.hosts {
            // localhost is seeded by the registry; its persisted launch config
            // (claude_bin/setup/env) is applied via update so the seeded entry
            // carries it, rather than appending a duplicate.
            if h.is_local {
                let _ =
                    self.hosts
                        .update(&h.id, h.claude_bin, h.setup, h.env, None, h.agent_commands);
            } else {
                self.hosts.load_remote(
                    &h.id,
                    &h.label,
                    &h.target,
                    h.claude_bin,
                    h.setup,
                    h.env,
                    h.agent_commands,
                );
            }
        }
        Ok(vec![VmUpdate::Hosts(self.hosts.list().to_vec())])
    }

    // -----------------------------------------------------------------------
    // Workspace commands
    // -----------------------------------------------------------------------

    /// Load persisted workspaces and active_ws from the store (call once at
    /// startup). Returns a `Workspaces` update for the now-complete list.
    pub fn load_workspaces(&mut self) -> Result<Vec<VmUpdate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Agent(format!("restore workspaces: {e:?}")))?;
        self.workspaces = restored
            .workspaces
            .iter()
            .map(Self::store_ws_to_vm)
            .collect();
        if let Some(id) = restored.active_ws {
            self.active_ws = Some(id);
        }
        Ok(vec![self.workspaces_update()])
    }

    /// Load persisted sessions from the store into the in-memory list (call once
    /// at startup, AFTER `load_hosts` so host labels resolve). Without this the
    /// dashboard comes up empty after a full app exit even though the sessions
    /// were persisted — the user perceives them as "lost". Call order matters:
    /// already-tracked ids (a live session this Core launched) are left untouched.
    ///
    /// Each restored session is marked `Stale` (not live): its claude process
    /// may or may not have survived the exit, so we present it as resumable
    /// (via `restart_session`) rather than claim a liveness we cannot verify. If
    /// the agent re-attaches a still-live local session, its next stream event
    /// flips the phase. Transcript history is loaded from the on-disk JSONL for
    /// local sessions; remote transcripts live on the remote host and are not
    /// fetched here, so a restored remote session shows an empty transcript until
    /// it streams again.
    pub fn load_sessions(&mut self) -> Result<Vec<VmUpdate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Agent(format!("restore sessions: {e:?}")))?;
        // X6 (Wasl Backend, §6.1): restore the per-session Wasl runtime bindings
        // so a WaslManaged session reloads as `WaslManaged` (routing the `*Wasl*`
        // family) rather than degrading to `MarsadManaged`. Stored as their own
        // records so this is fully additive to the existing session schema.
        let wasl_bindings = self.store.wasl_bindings().unwrap_or_default();
        let mut updates = Vec::new();
        for s in restored.sessions {
            if self.sessions.contains_key(&s.id) {
                continue;
            }
            // Recover the runtime owner (and the last observed seq) from the
            // persisted binding, if any. Absent binding → MarsadManaged (default).
            let (runtime_owner, wasl_last_seq) = match wasl_bindings.get(&s.id) {
                Some(b) => (
                    RuntimeOwner::WaslManaged {
                        binding: WaslSessionBinding {
                            target_id: b.target_id.clone(),
                            host_id: b.host_id.clone(),
                            provider_key: b.provider_key.clone(),
                        },
                    },
                    b.last_seq,
                ),
                None => (RuntimeOwner::MarsadManaged, 0),
            };
            // opencode owns no claude `~/.claude` transcript — its history comes
            // from the ACP stream. Reading the project's claude transcript here
            // would splice a CO-LOCATED claude session's history under an opencode
            // session (opencode header + claude transcript). Matches the guard in
            // the agent reader's remote backfill.
            let transcript = if s.driver == "opencode" {
                Vec::new()
            } else {
                match &s.session_id {
                    Some(sid) if !sid.is_empty() => self.transcript.session(&s.project, sid),
                    _ => self.transcript.most_recent(&s.project),
                }
            };
            let mut state = SessionState::initial();
            // Two-axis model: the ACTIVITY of a restored session is idle → Done
            // (it isn't mid-turn from our POV). "Not running" is carried by the
            // separate liveness axis (`live=false` below → Paused) until the agent
            // reconnects and proves it live. (Previously this was `Stale`, which
            // conflated activity with liveness.)
            state.phase = Phase::Done;
            state.model = Some(s.model.clone());
            state.session_id = s.session_id.clone();
            // Restore the persisted posture so the Config tab shows the correct
            // posture even when the live claude process does NOT re-emit
            // system/init (reconnect after restart). Without this, permission_mode
            // stays None and posture defaults to "ask" for every reconnected session.
            state.permission_mode = s
                .posture
                .as_deref()
                .and_then(posture_name_to_permission_mode)
                .map(str::to_string);
            // Restore slash_commands, tools, and mcp_servers from the last
            // system/init so Config/Composer are pre-populated on attach/reconnect
            // without waiting for a fresh system/init.
            state.slash_commands = s.slash_commands.clone();
            state.tools = s.tools.clone();
            state.mcp_servers = s.mcp_servers.clone();
            // Restore the custom name: PersistedSession.name is the marsad id
            // when no custom name was set (agent default). Treat that as None so
            // the view falls back to the project basename.
            let restored_name = if s.name != s.id {
                Some(s.name.clone())
            } else {
                None
            };
            // Probe git changes immediately for LOCAL sessions so the Changes tab
            // is populated without waiting for the first tool_use/result event.
            // Remote sessions get their changes from the agent's ChangesUpdate IPC.
            let is_local = self.host_is_local(&s.host_id);
            let initial_changes = if is_local {
                self.git.changes(&s.project)
            } else {
                Vec::new()
            };
            // Seed token/cost totals for LOCAL sessions from the session JSONL so
            // the Usage tab shows the whole-conversation total immediately on attach
            // rather than zero. Remote sessions get their usage from the agent's
            // UsageUpdate IPC (remote_usage_script). The first live `result` event
            // will authoritatively replace these estimates via reduce_result.
            if is_local && s.driver != "opencode" {
                if let Some(sid) = s.session_id.as_deref().filter(|s| !s.is_empty()) {
                    let usage_agg = self.transcript.session_usage(&s.project, sid);
                    if !usage_agg.is_empty() {
                        let total_tokens: u64 = usage_agg
                            .values()
                            .map(|u| {
                                u.input_tokens
                                    .saturating_add(u.output_tokens)
                                    .saturating_add(u.cache_read_input_tokens)
                                    .saturating_add(u.cache_creation_input_tokens)
                            })
                            .sum();
                        if total_tokens > 0 {
                            state.tokens = total_tokens;
                            let window = state
                                .model
                                .as_deref()
                                .map(marsad_events::SessionState::context_window_for)
                                .unwrap_or(200_000);
                            let pct = (total_tokens as f64 / window as f64 * 100.0) as f32;
                            state.context_pct = pct.min(100.0);
                        }
                        for (model, summary) in usage_agg {
                            state.model_usage.insert(
                                model,
                                marsad_proto::ModelUsage {
                                    input_tokens: summary.input_tokens,
                                    output_tokens: summary.output_tokens,
                                    cost_usd: summary.cost_usd,
                                    estimated: true,
                                },
                            );
                        }
                    }
                }
            }
            self.session_order.push(s.id.clone());
            self.sessions.insert(
                s.id.clone(),
                SessionRecord {
                    project: s.project.clone(),
                    host_id: s.host_id.clone(),
                    state,
                    changes: initial_changes,
                    transcript,
                    seen_msg_ids: std::collections::HashSet::new(),
                    name: restored_name,
                    shell_running: false,
                    driver: s.driver.clone(),
                    // Restored from the store: we do NOT yet know if the process
                    // survived on its host. Start Paused; the agent flips it to
                    // Resumed via a stream event / backfill / SessionLiveness once
                    // it actually reconnects. No optimism — truth on confirmation.
                    live: false,
                    conn_health: marsad_proto::ConnHealth::Connected,
                    runtime_owner,
                    wasl_last_seq,
                    wasl_seen_seqs: std::collections::HashSet::new(),
                    wasl_open_assistant: None,
                    wasl_turn_closed: false,
                    wasl_native_gates: HashMap::new(),
                },
            );
            let sv = self.make_session_view(&s.id, self.sessions.get(&s.id).unwrap());
            updates.push(VmUpdate::Session(sv));
        }
        let snap = self.snapshot();
        updates.push(VmUpdate::Overview(marsad_viewmodel::group_overview(&snap)));
        Ok(updates)
    }

    /// Save the current session list as a new named workspace, persist it,
    /// and return the updated workspace list.
    pub fn save_workspace(
        &mut self,
        name: &str,
        session_ids: Vec<String>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let id = format!("ws-{}", name.to_lowercase().replace(' ', "-"));
        let saved_at = chrono_now();
        let ws = Workspace {
            id,
            name: name.to_string(),
            session_ids,
            saved_at,
        };
        let store_ws = Self::vm_ws_to_store(&ws);
        self.store
            .snapshot_workspace(&store_ws)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        // Upsert into in-memory list (overwrite if same id).
        if let Some(pos) = self.workspaces.iter().position(|w| w.id == ws.id) {
            self.workspaces[pos] = ws;
        } else {
            self.workspaces.push(ws);
        }
        Ok(vec![self.workspaces_update()])
    }

    /// Duplicate a workspace by id, persist the copy, and return the updated list.
    pub fn duplicate_workspace(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let original = self
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| CoreError::Agent(format!("workspace not found: {id}")))?
            .clone();
        let new_id = format!("{}-copy", original.id);
        let copy = Workspace {
            id: new_id,
            name: format!("{} (copy)", original.name),
            session_ids: original.session_ids.clone(),
            saved_at: chrono_now(),
        };
        let store_ws = Self::vm_ws_to_store(&copy);
        self.store
            .snapshot_workspace(&store_ws)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        self.workspaces.push(copy);
        Ok(vec![self.workspaces_update()])
    }

    /// Delete a workspace by id, remove from persistence, and return the updated list.
    pub fn delete_workspace(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.store
            .delete_workspace(id)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        self.workspaces.retain(|w| w.id != id);
        // If the deleted workspace was active, clear active_ws.
        if self.active_ws.as_deref() == Some(id) {
            self.active_ws = None;
        }
        Ok(vec![self.workspaces_update()])
    }

    /// Export a workspace as a `.marsad.json` string (no VmUpdate; returns JSON).
    pub fn export_workspace(&self, id: &str) -> Result<String, CoreError> {
        self.store
            .export_workspace(id)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))
    }

    /// Import a workspace from a `.marsad.json` string, persist it, and return
    /// the updated list.
    pub fn import_workspace(&mut self, json: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let store_ws = self
            .store
            .import_workspace(json)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        let ws = Self::store_ws_to_vm(&store_ws);
        // Upsert into in-memory list.
        if let Some(pos) = self.workspaces.iter().position(|w| w.id == ws.id) {
            self.workspaces[pos] = ws;
        } else {
            self.workspaces.push(ws);
        }
        Ok(vec![self.workspaces_update()])
    }

    /// Add a remote host (`label`, `user@host`), persist it, and return the
    /// updated host list. The host starts `Reconnecting` until a health check.
    #[allow(clippy::too_many_arguments)]
    pub fn add_host(
        &mut self,
        label: &str,
        target: &str,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
        agent_commands: Vec<(String, String)>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let host = self
            .hosts
            .add(label, target, claude_bin, setup, env, agent_commands)
            .map_err(|e| CoreError::Host(e.to_string()))?
            .clone();
        let persisted = PersistedHost {
            id: host.id,
            label: host.label,
            target: host.target,
            is_local: host.is_local,
            claude_bin: host.claude_bin,
            setup: host.setup,
            env: host.env,
            agent_commands: host.agent_commands,
        };
        self.store
            .upsert_host(&persisted)
            .map_err(|e| CoreError::Host(format!("{e:?}")))?;
        Ok(vec![VmUpdate::Hosts(self.hosts.list().to_vec())])
    }

    /// Update an existing host's launch configuration (`claude_bin` / `setup` /
    /// `env` / `agent_commands`), and optionally its `target`. Works for any host
    /// id INCLUDING the protected `localhost` (pass `None` for `target` to leave
    /// the target unchanged — localhost's stays `"this machine"`). Persists the
    /// change and returns the updated host list.
    #[allow(clippy::too_many_arguments)]
    pub fn update_host(
        &mut self,
        id: &str,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
        target: Option<String>,
        agent_commands: Vec<(String, String)>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let host = self
            .hosts
            .update(id, claude_bin, setup, env, target, agent_commands)
            .map_err(|e| CoreError::Host(e.to_string()))?
            .clone();
        let persisted = PersistedHost {
            id: host.id,
            label: host.label,
            target: host.target,
            is_local: host.is_local,
            claude_bin: host.claude_bin,
            setup: host.setup,
            env: host.env,
            agent_commands: host.agent_commands,
        };
        self.store
            .upsert_host(&persisted)
            .map_err(|e| CoreError::Host(format!("{e:?}")))?;
        Ok(vec![VmUpdate::Hosts(self.hosts.list().to_vec())])
    }

    /// Remove a host by id, delete it from the store, and return the updated list.
    pub fn remove_host(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.hosts
            .remove(id)
            .map_err(|e| CoreError::Host(e.to_string()))?;
        self.store
            .delete_host(id)
            .map_err(|e| CoreError::Host(format!("{e:?}")))?;
        Ok(vec![VmUpdate::Hosts(self.hosts.list().to_vec())])
    }

    /// Apply an externally-computed health result for a host (runtime status, not
    /// persisted). Returns the updated host list.
    /// The drivers Marsad probes on every host (mirrors the agent's probe set).
    /// A host is only `Connected` once ALL of these have reported their caps.
    const PROBED_DRIVERS: [&'static str; 3] = ["claude", "codex", "gemini"];

    /// True once every probed driver's capability discovery has landed for `id`
    /// (agents + versioned models loaded) since the host last came online.
    fn host_caps_fully_loaded(&self, id: &str) -> bool {
        match self.caps_loaded.get(id) {
            Some(seen) => Self::PROBED_DRIVERS.iter().all(|d| seen.contains(*d)),
            None => false,
        }
    }

    pub fn set_host_status(&mut self, id: &str, status: HostStatus) -> Vec<VmUpdate> {
        // The health worker reports raw reachability (Connected / Unreachable).
        // We translate that into the truthful UI status: a reachable host stays
        // `Connecting` until its capability discovery has fully landed (agents +
        // versioned models), then flips to `Connected`. No optimism, no timeouts
        // surfaced — just "connecting while the background load happens".
        match status {
            HostStatus::Connected => {
                let fully = self.host_caps_fully_loaded(id);
                let effective = if fully {
                    HostStatus::Connected
                } else {
                    HostStatus::Connecting
                };
                self.hosts.set_status(id, effective);
                // Kick off a FRESH all-driver discovery exactly once per online
                // session, on the first Connected — even when `fully` is already
                // true from PERSISTED caps restored on restart. Those persisted
                // caps still display immediately (the host flips Connected with no
                // wait); this background probe freshens them so a CLI installed
                // since the last run is picked up (the WSL/SSH staleness bug). We
                // gate on `discovery_triggered` — NOT `caps_loaded`, which the
                // persisted replay also fills — so a restart genuinely re-probes.
                // `insert` returns true only the first time, so repeated health
                // ticks don't re-probe. Cleared on Unreachable so reconnect re-runs.
                if self.discovery_triggered.insert(id.to_string()) {
                    self.refresh_capabilities(Some(id.to_string()), None, None);
                }
            }
            HostStatus::Unreachable => {
                // Went offline → forget its discovery so a reconnect re-probes
                // cleanly (and the host correctly returns to Connecting first).
                self.caps_loaded.remove(id);
                self.discovery_triggered.remove(id);
                self.hosts.set_status(id, HostStatus::Unreachable);
            }
            other => {
                self.hosts.set_status(id, other);
            }
        }
        vec![VmUpdate::Hosts(self.hosts.list().to_vec())]
    }

    // -----------------------------------------------------------------------
    // Settings commands
    // -----------------------------------------------------------------------

    /// Default sensible settings returned when nothing has been persisted yet.
    fn default_settings() -> PersistedSettings {
        use marsad_store::NotifPref;
        use std::collections::HashMap;

        let mut notif_prefs: HashMap<String, NotifPref> = HashMap::new();
        for (ev, enabled, priority, sound, group, dnd) in [
            ("gate_needs_you", true, "urgent", true, false, false),
            ("gate_error", true, "urgent", true, false, false),
            ("session_done", true, "normal", false, true, true),
            ("session_started", false, "low", false, true, true),
            ("host_unreachable", true, "urgent", true, false, false),
        ] {
            notif_prefs.insert(
                ev.to_string(),
                NotifPref {
                    enabled,
                    priority: priority.to_string(),
                    sound,
                    group,
                    dnd,
                },
            );
        }
        PersistedSettings {
            notif_prefs,
            play_sound: true,
            group_alerts: true,
            do_not_disturb: false,
            background: "Gradient".to_string(),
            restore_on_launch: true,
            confirm_before_stop: true,
            default_model: "claude-sonnet-4-6".to_string(),
            default_posture: "ask".to_string(),
            default_driver: "claude".to_string(),
            default_models: Vec::new(),
        }
    }

    /// Return the stored settings, or sensible defaults if nothing has been persisted yet.
    pub fn get_settings(&self) -> PersistedSettings {
        self.store
            .restore()
            .ok()
            .and_then(|r| r.settings)
            .unwrap_or_else(Self::default_settings)
    }

    /// Persist the app settings blob.
    pub fn set_settings(&mut self, s: &PersistedSettings) -> Result<(), CoreError> {
        self.store
            .upsert_settings(s)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))
    }

    // -----------------------------------------------------------------------
    // Changes commands
    // -----------------------------------------------------------------------

    /// Overwrite the working-tree change list for a session (e.g. after a
    /// commit that cleared some of the files) and return the usual pair of
    /// `[VmUpdate::Session, VmUpdate::Overview]`.
    ///
    /// Returns an empty vec (no-op) for an unknown session id so that callers
    /// never need to guard against the session having been removed concurrently.
    pub fn set_session_changes(&mut self, id: &str, changes: Vec<FileChange>) -> Vec<VmUpdate> {
        let Some(record) = self.sessions.get_mut(id) else {
            return vec![];
        };
        record.changes = changes;
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        vec![VmUpdate::Session(sv), VmUpdate::Overview(groups)]
    }

    // -----------------------------------------------------------------------
    // Session commands
    // -----------------------------------------------------------------------

    pub fn launch_session(&mut self, spec: LaunchSpec) -> Result<String, CoreError> {
        let id = new_session_id();
        self.launch_session_with_id(id, spec)
    }

    /// X6 (Wasl Backend, §6.1): launch a `WaslManaged` session — one whose
    /// underlying process is owned END-TO-END by Wasl. Emits `LaunchWaslSession`
    /// (NOT `LaunchSession` — no container/argv semantics apply), registers a
    /// `RuntimeOwner::WaslManaged` record, and PERSISTS the `WaslSessionBinding`
    /// (as its own store record) so a fresh `Core` over the same backend can
    /// reconnect after a restart. Returns the Marsad session id.
    pub fn launch_wasl_session(
        &mut self,
        host_id: &str,
        wasl_target_key: serde_json::Value,
        name: Option<String>,
    ) -> Result<String, CoreError> {
        let id = new_session_id();

        // Drive the agent: build the WaslSessionClient behind its registry.
        self.send_ipc(&IpcMsg::LaunchWaslSession {
            session_id: id.clone(),
            host_id: host_id.to_string(),
            wasl_target_key: wasl_target_key.clone(),
            name: name.clone(),
        })?;

        let binding = WaslSessionBinding {
            // target_id is Wasl-assigned and learned on connect; unknown at launch
            // time (the provider maps the key → a target inside the agent). Empty
            // until a status/event surfaces it; the provider_key is what reconnect
            // actually keys on.
            target_id: String::new(),
            host_id: host_id.to_string(),
            provider_key: wasl_target_key.clone(),
        };

        // Persist a session record + the Wasl binding so a fresh Core restores
        // this as WaslManaged (routing the `*Wasl*` family) and can reconnect.
        // A WaslManaged session has no claude container/argv; the persisted
        // `driver` is a display marker ("wasl") and every claude-only field is
        // empty/default.
        let custom_name = name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let persisted_name = custom_name.clone().unwrap_or_else(|| id.clone());
        let persisted = marsad_store::PersistedSession {
            id: id.clone(),
            session_id: None,
            name: persisted_name,
            host_id: host_id.to_string(),
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
            project: String::new(),
            branch: "main".to_string(),
            model: String::new(),
            last_phase: "Starting".to_string(),
            posture: None,
            disallowed_tools: Vec::new(),
            driver: "wasl".to_string(),
            slash_commands: Vec::new(),
            tools: Vec::new(),
            mcp_servers: Vec::new(),
        };
        // Best-effort: a store error must not fail the launch (the session is
        // already live in-memory + driven over IPC); reconnect degrades but the
        // running session is unaffected. Mirrors the store-write disposition
        // elsewhere in this file.
        let _ = self.store.upsert_session(&persisted);
        let _ = self
            .store
            .upsert_wasl_binding(&marsad_store::PersistedWaslBinding {
                session_id: id.clone(),
                target_id: binding.target_id.clone(),
                host_id: binding.host_id.clone(),
                provider_key: binding.provider_key.clone(),
                last_seq: 0,
            });

        let mut state = SessionState::initial();
        state.phase = Phase::Starting;
        self.session_order.push(id.clone());
        self.sessions.insert(
            id.clone(),
            SessionRecord {
                project: String::new(),
                host_id: host_id.to_string(),
                state,
                changes: Vec::new(),
                transcript: Vec::new(),
                seen_msg_ids: std::collections::HashSet::new(),
                name: custom_name,
                shell_running: false,
                // Display driver only — the real driver is Wasl's (§6.1).
                driver: "wasl".to_string(),
                live: true,
                conn_health: marsad_proto::ConnHealth::Connected,
                runtime_owner: RuntimeOwner::WaslManaged { binding },
                wasl_last_seq: 0,
                wasl_seen_seqs: std::collections::HashSet::new(),
                wasl_open_assistant: None,
                wasl_turn_closed: false,
                wasl_native_gates: HashMap::new(),
            },
        );

        // Auto-create the single-node fast-lane workstream, mirroring
        // launch_session_with_id, so a WaslManaged session shows on the board too.
        if !self.membership.contains_key(&id) {
            let record = self.sessions.get(&id).unwrap();
            let ws_name = record
                .name
                .clone()
                .unwrap_or_else(|| session_display_name(&record.project, &id));
            let ws_id = format!("ws-{id}");
            let ws = marsad_workstream::Workstream {
                id: ws_id.clone(),
                name: ws_name,
                goal: String::new(),
                profile: None,
            };
            let _ = self.store.upsert_workstream(&ws);
            self.workstreams.push(ws);
            self.membership.insert(id.clone(), ws_id.clone());
            self.ws_leads.insert(ws_id, id.clone());
            let _ = self.store.set_membership(&self.membership);
        }

        Ok(id)
    }

    /// X6 (§6.1): the runtime owner of `id`. Used by the centralized routing
    /// methods and `answer_gate` to pick the MarsadManaged vs WaslManaged path.
    fn runtime_owner(&self, id: &str) -> RuntimeOwner {
        self.sessions
            .get(id)
            .map(|r| r.runtime_owner.clone())
            .unwrap_or_default()
    }

    /// X6 centralized routing (§6.1): the RuntimeOwner match for "attach" happens
    /// in EXACTLY ONE place. MarsadManaged → today's `AttachSession`; WaslManaged
    /// → `AttachWaslSession { after_seq = wasl_last_seq }` (re-pump from the last
    /// observed seq — the reconnect contract, §6.1 step 2).
    fn attach_session_events(&mut self, id: &str) -> Result<(), CoreError> {
        match self.runtime_owner(id) {
            RuntimeOwner::MarsadManaged => self.send_ipc(&IpcMsg::AttachSession {
                session_id: id.to_string(),
            }),
            RuntimeOwner::WaslManaged { .. } => {
                let after_seq = self.sessions.get(id).map(|r| r.wasl_last_seq).unwrap_or(0);
                self.send_ipc(&IpcMsg::AttachWaslSession {
                    session_id: id.to_string(),
                    after_seq,
                })
            }
        }
    }

    /// X6 centralized routing (§6.1): the RuntimeOwner match for "interrupt".
    fn interrupt_session_routed(&mut self, id: &str) -> Result<(), CoreError> {
        match self.runtime_owner(id) {
            RuntimeOwner::MarsadManaged => self.send_ipc(&IpcMsg::Interrupt {
                session_id: id.to_string(),
            }),
            RuntimeOwner::WaslManaged { .. } => self.send_ipc(&IpcMsg::InterruptWaslSession {
                session_id: id.to_string(),
            }),
        }
    }

    /// X6 centralized routing (§6.1): the RuntimeOwner match for "stop".
    fn stop_session_routed(&mut self, id: &str) -> Result<(), CoreError> {
        match self.runtime_owner(id) {
            RuntimeOwner::MarsadManaged => self.send_ipc(&IpcMsg::StopSession {
                session_id: id.to_string(),
            }),
            RuntimeOwner::WaslManaged { .. } => self.send_ipc(&IpcMsg::StopWaslSession {
                session_id: id.to_string(),
                drain: false,
            }),
        }
    }

    /// X6 centralized routing (§6.1): the RuntimeOwner match for "send prompt".
    /// WaslManaged mints a fresh UUID idempotency key per human send (§4.4/§12 Q5)
    /// and defaults to the Queue busy policy (§12 Q4).
    fn send_session_prompt(&mut self, id: &str, text: &str) -> Result<(), CoreError> {
        match self.runtime_owner(id) {
            RuntimeOwner::MarsadManaged => self.send_ipc(&IpcMsg::SendPrompt {
                session_id: id.to_string(),
                text: text.to_string(),
            }),
            RuntimeOwner::WaslManaged { .. } => self.send_ipc(&IpcMsg::SendWaslPrompt {
                session_id: id.to_string(),
                text: text.to_string(),
                idempotency_key: marsad_wasl_session::new_idempotency_key(),
                busy_policy: "queue".to_string(),
                max_wait_ms: 0,
            }),
        }
    }

    /// Send the LaunchSession IPC for the given `id` and `spec`, seed the
    /// transcript, and register the session record.
    ///
    /// This is the shared implementation used by both `launch_session` (which
    /// generates a fresh id) and `restart_session` (which reuses the existing id).
    fn launch_session_with_id(
        &mut self,
        id: String,
        spec: LaunchSpec,
    ) -> Result<String, CoreError> {
        let permission_mode = posture_to_permission_mode(&spec.posture);
        // Forward the host so the agent runs the session on the right machine.
        // Resolve the remote execution target from the host id via the registry
        // (local/unknown hosts → None → the agent runs locally). An explicit
        // `spec.host_target` wins if a caller already set one.
        let host_target = spec.host_target.clone().or_else(|| {
            self.hosts
                .list()
                .iter()
                .find(|h| h.id == spec.host_id && !h.is_local)
                .map(|h| h.target.clone())
        });
        // Resolve the per-host launch config (claude_bin / setup / env) from the
        // registry by host_id and inject it for the agent. Applies to ALL host
        // types (local, WSL, SSH/plink). An explicit value already on the spec
        // wins over the registry lookup. `env` is forwarded as `[k, v]` pairs
        // (the agent parses it back identically).
        //
        // Driver-aware bin resolution (deliverable #1): look up `spec.driver` in
        // `host.agent_commands`; fall back to `claude_bin` for "claude"; else bare
        // driver name. An explicit `spec.claude_bin` still wins (preserves the
        // restart path's persisted-value passthrough).
        let host_cfg = self.hosts.list().iter().find(|h| h.id == spec.host_id);
        let claude_bin = if spec.claude_bin.is_some() {
            spec.claude_bin.clone()
        } else if let Some(host) = host_cfg {
            let resolved = resolve_driver_bin(
                &spec.driver,
                host.claude_bin.as_deref(),
                &host.agent_commands,
            );
            // Only set it when the resolved value differs from the bare driver name
            // (i.e., there IS an override); the agent defaults to the bare name too.
            if resolved != spec.driver {
                Some(resolved)
            } else {
                None
            }
        } else {
            None
        };
        let setup = spec
            .setup
            .clone()
            .or_else(|| host_cfg.and_then(|h| h.setup.clone()));
        let env = if !spec.env.is_empty() {
            spec.env.clone()
        } else {
            host_cfg.map(|h| h.env.clone()).unwrap_or_default()
        };
        // Forward the custom name so the agent persists it. Trim and treat
        // whitespace-only as absent (pass nothing rather than a blank name).
        let custom_name: Option<String> = spec
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let msg = IpcMsg::LaunchSession {
            session_id: Some(id.clone()),
            project: spec.project.clone(),
            cwd: spec.project.clone(),
            host_id: Some(spec.host_id.clone()),
            host_target,
            claude_bin,
            setup,
            env,
            model: spec.model.clone(),
            posture: spec.posture.clone(),
            permission_mode: permission_mode.to_string(),
            first_prompt: spec.first_prompt.clone(),
            continue_: matches!(spec.mode, LaunchMode::Continue).then_some(true),
            resume_session_id: match &spec.mode {
                LaunchMode::Resume(sid) => Some(sid.clone()),
                _ => None,
            },
            name: custom_name.clone(),
            disallowed_tools: spec.disallowed_tools.clone(),
            // X4a (docs/specs/wasl-connector.md §X4a): forward the caller's
            // request verbatim so the agent can inject a Wasl MCP-server
            // entry at spawn time.
            wasl: spec.wasl,
            // Stage 1 codex launch: forward the driver discriminator so the
            // agent spawns the right binary. Defaults to "claude" on older
            // wire messages (handled by `default_driver` in marsad-proto).
            driver: spec.driver.clone(),
        };
        self.send_ipc(&msg)?;

        // Seed the transcript with prior history when resuming (loaded once — old,
        // fully-flushed JSONL, so no race), then the first prompt as a user turn.
        let mut transcript = match &spec.mode {
            LaunchMode::Resume(sid) => self.transcript.session(&spec.project, sid),
            LaunchMode::Continue => self.transcript.most_recent(&spec.project),
            LaunchMode::Fresh => Vec::new(),
        };
        if let Some(fp) = spec.first_prompt.as_deref() {
            let fp = fp.trim();
            if !fp.is_empty() {
                transcript.push(marsad_events::TranscriptEntry::User {
                    text: fp.to_string(),
                    at: Some(now_ms()),
                });
            }
        }

        // Upsert: if the id already exists (restart path), reset the record;
        // if new, push onto the order list first.
        if !self.sessions.contains_key(&id) {
            self.session_order.push(id.clone());
        }
        // Seed permission_mode from the launch posture so the Config tab shows
        // the correct posture immediately — before the authoritative system/init
        // event arrives (which will override it when it does).
        // NOTE: We use `posture_name_to_permission_mode` (which maps proto enum
        // variant names to the `permissionMode` values claude emits) rather than
        // reusing `permission_mode` from `posture_to_permission_mode` above,
        // because the IPC wire value ("autoApprove") differs from the value that
        // claude reports in system/init ("acceptEdits"). The state value must
        // match what `permission_mode_to_posture` recognises.
        let display_permission_mode = posture_name_to_permission_mode(match &spec.posture {
            Posture::Ask => "Ask",
            Posture::AutoApproveSafe => "AutoApproveSafe",
            Posture::FullAutonomy => "FullAutonomy",
        });
        let mut initial_state = SessionState::initial();
        initial_state.permission_mode = display_permission_mode.map(str::to_string);
        let driver = spec.driver.clone();
        self.sessions.insert(
            id.clone(),
            SessionRecord {
                project: spec.project,
                host_id: spec.host_id,
                state: initial_state,
                changes: Vec::new(),
                transcript,
                seen_msg_ids: std::collections::HashSet::new(),
                name: custom_name,
                shell_running: false,
                driver,
                // A launch (or a resume, which reuses this path) is starting the
                // process now → Resumed. The agent's SessionLiveness will correct
                // this to Paused if the launch never actually comes up.
                live: true,
                conn_health: marsad_proto::ConnHealth::Connected,
                // A MarsadManaged launch (today's path) — unchanged behavior.
                runtime_owner: RuntimeOwner::MarsadManaged,
                wasl_last_seq: 0,
                wasl_seen_seqs: std::collections::HashSet::new(),
                wasl_open_assistant: None,
                wasl_turn_closed: false,
                wasl_native_gates: HashMap::new(),
            },
        );

        // Auto-create a single-node fast-lane workstream for newly launched sessions
        // that have no existing membership (fresh launches from NewSession; NodeModal
        // calls assign_session separately so those sessions already have a workstream).
        if !self.membership.contains_key(&id) {
            let record = self.sessions.get(&id).unwrap();
            let ws_name = record
                .name
                .clone()
                .unwrap_or_else(|| session_display_name(&record.project, &id));
            let ws_id = format!("ws-{id}");
            let ws = marsad_workstream::Workstream {
                id: ws_id.clone(),
                name: ws_name,
                goal: String::new(),
                profile: None,
            };
            let _ = self.store.upsert_workstream(&ws);
            self.workstreams.push(ws);
            self.membership.insert(id.clone(), ws_id.clone());
            self.ws_leads.insert(ws_id, id.clone());
            let _ = self.store.set_membership(&self.membership);
        }

        Ok(id)
    }

    /// Restart a session with a new model, posture, and optionally a new set of
    /// disallowed tools.
    ///
    /// Sends `StopSession` to kill the running claude process, then re-launches
    /// the session with the **same `id`** so the UI row stays in place.  If a
    /// claude session-UUID is known (from a prior `system/init` event), the
    /// relaunch uses `Resume(<uuid>)` so the conversation history is preserved;
    /// otherwise it falls back to `Continue` (most-recent session in that project).
    ///
    /// Returns `UnknownSession` for an unrecognised `id`.
    pub fn restart_session(
        &mut self,
        id: &str,
        model: &str,
        posture: Posture,
        disallowed_tools: Vec<String>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;

        // Snapshot what we need before mutating state.
        let project = self.sessions[id].project.clone();
        let host_id = self.sessions[id].host_id.clone();
        let claude_uuid = self.sessions[id].state.session_id.clone();
        // Preserve any custom name so a restart doesn't lose it.
        let existing_name = self.sessions[id].name.clone();

        // 1. Stop the running claude process.
        self.send_ipc(&IpcMsg::StopSession {
            session_id: id.to_string(),
        })?;

        // 2. Choose the launch mode: Resume if we have the claude UUID, else Continue.
        let mode = if let Some(uuid) = claude_uuid.filter(|u| !u.is_empty()) {
            LaunchMode::Resume(uuid)
        } else {
            LaunchMode::Continue
        };

        // 3. Relaunch with the SAME marsad session id.
        // Preserve the session's original host so a restart re-launches on the
        // same machine (WSL/SSH), not silently back on local.
        //
        // Load the persisted session to recover its per-session launch config
        // (setup / env / claude_bin). The store is the authoritative source: it
        // was written at launch time with the resolved values, so a restart
        // reproduces EXACTLY what the initial launch used — even if the host
        // registry has since been modified or the host removed.  If the record
        // is absent (e.g. session was never persisted, or was already deleted),
        // fall back to None/empty so launch_session_with_id re-resolves from
        // the registry as before.
        let (persisted_setup, persisted_env, persisted_claude_bin, persisted_driver) = self
            .store
            .restore()
            .ok()
            .and_then(|r| r.sessions.into_iter().find(|s| s.id == id))
            .map(|s| (s.setup, s.env, s.claude_bin, s.driver))
            .unwrap_or((None, Vec::new(), None, "claude".to_string()));

        let spec = LaunchSpec {
            project,
            host_id,
            model: model.to_string(),
            posture,
            first_prompt: None,
            mode,
            host_target: None,
            claude_bin: persisted_claude_bin,
            setup: persisted_setup,
            env: persisted_env,
            name: existing_name,
            disallowed_tools,
            // A restart never re-attaches a Wasl MCP-server entry this cycle
            // (mirrors the agent's own resume path, `marsad-agent/src/ipc.rs`
            // — out of X4's scope; start a fresh `wasl:true` launch instead).
            wasl: false,
            // Preserve the original driver so a restart uses the same agent
            // binary. Defaults to "claude" for pre-Stage-1 persisted records.
            driver: persisted_driver,
        };
        self.launch_session_with_id(id.to_string(), spec)?;

        // 4. Return a session view so the UI can update immediately.
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        Ok(vec![VmUpdate::Session(sv), VmUpdate::Overview(groups)])
    }

    /// Persist a named config template (model + posture).
    ///
    /// Overwrites any existing template with the same name.
    pub fn save_template(
        &mut self,
        name: &str,
        model: &str,
        posture: &str,
    ) -> Result<(), CoreError> {
        let tmpl = marsad_store::PersistedTemplate {
            name: name.to_string(),
            model: model.to_string(),
            posture: posture.to_string(),
        };
        self.store
            .upsert_template(&tmpl)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))
    }

    /// Return all persisted config templates.
    pub fn list_templates(&self) -> Result<Vec<marsad_store::PersistedTemplate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Agent(format!("restore templates: {e:?}")))?;
        Ok(restored.templates)
    }

    /// Resume a stopped session: tell the agent to relaunch/re-attach it, and
    /// flip the UI badge OFF Stale immediately.
    ///
    /// Previously this only fired the IPC and returned `()`, so the dashboard
    /// never changed — the user saw "resume did nothing, still stopped" (the same
    /// bug `stop_session` had). The agent does relaunch claude, but a resumed
    /// session has no first prompt, so claude stays idle and emits no
    /// `system/init` — meaning NO inbound stream event would ever move the phase
    /// off Stale. We therefore set `Starting` optimistically and return the
    /// VmUpdates the bridge emits; typing a prompt then drives it to Active.
    pub fn resume_session(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::ResumeSession {
            session_id: id.to_string(),
            first_prompt: None,
        })?;
        // require_session above guarantees it exists.
        {
            let rec = self.sessions.get_mut(id).unwrap();
            rec.state.phase = Phase::Starting;
            // Optimistically Resumed: we are (re)starting the process now. The
            // agent's SessionLiveness corrects this to Paused if it never comes
            // up. (For an already-live session the agent no-ops the relaunch —
            // see handle_resume_session — and live stays true anyway.)
            rec.live = true;
        }
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        // Resuming moves the session from Paused → live on the board.
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    pub fn attach_session(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        // X6 (§6.1): route by RuntimeOwner — MarsadManaged emits today's
        // AttachSession; WaslManaged emits AttachWaslSession{after_seq}.
        self.attach_session_events(id)
    }

    /// Stop a session: kill its running claude process (the agent drops the
    /// pipe/container) and reflect it in the UI by marking the row not-live.
    /// The session stays in the list and is resumable via `restart_session`.
    ///
    /// Previously this only fired the IPC and returned `()`, so the dashboard
    /// never changed — the user saw "stop did nothing". Now it returns the
    /// VmUpdates the bridge emits so the badge flips to Stale immediately.
    pub fn stop_session(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        // X6 (§6.1): route by RuntimeOwner — MarsadManaged emits StopSession;
        // WaslManaged emits StopWaslSession.
        self.stop_session_routed(id)?;
        // Mark the session not-live (Paused). require_session guarantees it
        // exists. `live=false` is the authoritative lifecycle flip; the phase is
        // also set Stale for legacy consumers, but the board/affordance now key
        // off `live`.
        {
            let rec = self.sessions.get_mut(id).unwrap();
            rec.state.phase = Phase::Stale;
            rec.live = false;
        }
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        // Stopping moves the session into the board's Paused group.
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Remove a session entirely (the user's explicit declutter action). Stops
    /// the process best-effort, drops the session from the in-memory list AND the
    /// persisted store (so it does NOT reappear on relaunch), and tells the UI to
    /// delete the row. Unlike `stop_session` (which keeps it resumable), this is
    /// permanent — the conversation is still re-creatable as a fresh resume if the
    /// claude JSONL survives.
    pub fn remove_session(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        // Best-effort stop so we don't orphan a running process. The session is
        // going away regardless, so a transport error here is non-fatal.
        let _ = self.send_ipc(&IpcMsg::StopSession {
            session_id: id.to_string(),
        });
        // Delete from the store FIRST so an error leaves in-memory state intact.
        self.store
            .delete_session(id)
            .map_err(|e| CoreError::Agent(format!("delete session: {e:?}")))?;
        // X6: also drop any Wasl binding record so a removed WaslManaged session
        // leaves no stale reconnect binding behind. Best-effort (its absence for a
        // MarsadManaged session is a no-op).
        let _ = self.store.delete_wasl_binding(id);
        self.sessions.remove(id);
        self.session_order.retain(|s| s != id);

        // Clean up membership and the auto-created single-node workstream (ws-<id>)
        // so the board doesn't keep a ghost card after deletion from SessionDetail.
        // We do NOT emit a Workstreams update here — callers that need one (bridge
        // remove_session, delete_workstream) emit it themselves after we return.
        let auto_ws_id = format!("ws-{id}");
        if self.workstreams.iter().any(|w| w.id == auto_ws_id) {
            let _ = self.store.delete_workstream(&auto_ws_id);
            self.workstreams.retain(|w| w.id != auto_ws_id);
        }
        self.membership.remove(id);
        self.ws_leads.remove(&auto_ws_id);
        let _ = self.store.set_membership(&self.membership);

        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        Ok(vec![
            VmUpdate::SessionRemoved { id: id.to_string() },
            VmUpdate::Overview(groups),
        ])
    }

    /// Inject a shell command and its output into the session transcript.
    ///
    /// Called by the bridge after it runs `!<command>` on the session's host
    /// (local or remote). Core is IO-free: the bridge does the actual execution
    /// and passes the command string + captured output here. Two transcript
    /// entries are appended:
    ///   1. `User { text: "! <command>" }` — the issued command (matches how
    ///      Claude Code shows shell commands: `!cmd` prefix).
    ///   2. `Tool { name: "shell", input: <output> }` — the captured output,
    ///      rendered in the Activity tab as a ⚙ tool row with monospaced output.
    ///
    /// Returns `UnknownSession` for an unrecognised `id`.
    pub fn inject_shell_output(
        &mut self,
        id: &str,
        command: &str,
        output: &str,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        let record = self.sessions.get_mut(id).unwrap();
        let at = Some(now_ms());
        record
            .transcript
            .push(marsad_events::TranscriptEntry::User {
                text: format!("! {}", command.trim()),
                at,
            });
        record
            .transcript
            .push(marsad_events::TranscriptEntry::Tool {
                name: "shell".to_string(),
                input: output.to_string(),
                at,
            });
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Step 1 of an ASYNC `!` shell command: append the `! <command>` line to the
    /// transcript immediately (so it shows the instant the user hits enter) and
    /// mark a shell command as running, so the UI can show a working indicator
    /// while the command executes off-thread. Paired with
    /// [`inject_shell_result`], which appends the output when the command
    /// finishes. Returns a Session update to render right away.
    pub fn inject_shell_command(
        &mut self,
        id: &str,
        command: &str,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        let record = self.sessions.get_mut(id).unwrap();
        record
            .transcript
            .push(marsad_events::TranscriptEntry::User {
                text: format!("! {}", command.trim()),
                at: Some(now_ms()),
            });
        record.shell_running = true;
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Step 2 of an ASYNC `!` shell command: append the captured output as a
    /// shell tool row and clear the running flag. The `! <command>` line was
    /// already appended by [`inject_shell_command`], so only the output is added.
    pub fn inject_shell_result(
        &mut self,
        id: &str,
        output: &str,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        let record = self.sessions.get_mut(id).unwrap();
        record.shell_running = false;
        record
            .transcript
            .push(marsad_events::TranscriptEntry::Tool {
                name: "shell".to_string(),
                input: output.to_string(),
                at: Some(now_ms()),
            });
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    pub fn send_prompt(&mut self, id: &str, text: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        // Record the user's prompt immediately — claude does not echo it on the
        // stream, so this is the only place it can enter the transcript. It surfaces
        // on the next agent event (claude's reply) which re-emits the SessionView.
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            // Safe: require_session above guarantees the entry exists.
            let rec = self.sessions.get_mut(id).unwrap();
            rec.transcript.push(marsad_events::TranscriptEntry::User {
                text: trimmed.to_string(),
                at: Some(now_ms()),
            });
            // Optimistically enter Working the instant the user sends — don't wait
            // for claude's first token. Clears any prior end reason / stale
            // interrupt so a fresh turn starts clean. The stream re-drives phase
            // authoritatively from here (assistant → Working, result → Done).
            rec.state.phase = Phase::Working;
            rec.state.end_reason = None;
            rec.state.interrupt_requested = false;
        }
        // X6 (§6.1): route by RuntimeOwner — MarsadManaged emits SendPrompt;
        // WaslManaged emits SendWaslPrompt (fresh per-send UUID idempotency key).
        self.send_session_prompt(id, text)
    }

    /// Change a session config option (opencode: model / mode) on the running
    /// agent. Fire-and-forget: the agent applies it via ACP and the reader
    /// surfaces the agent-confirmed new state — we do NOT optimistically mutate
    /// local state (truthful; the agent could reject or normalize the value).
    pub fn set_session_config_option(
        &mut self,
        id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::SetSessionConfigOption {
            session_id: id.to_string(),
            config_id: config_id.to_string(),
            value: value.to_string(),
        })
    }

    pub fn interrupt(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        // Record that WE initiated this interrupt so the resulting error turn is
        // classified as a calm user Cancel (Done, "Cancelled by the user") rather
        // than a failure. require_session guarantees the entry exists.
        self.sessions.get_mut(id).unwrap().state.interrupt_requested = true;
        // Emit a dedicated `Interrupt` message so the agent routes it to
        // handle_interrupt — which writes claude's `interrupt` control_request to
        // stop the CURRENT TURN (claude then ends the turn with
        // error_during_execution, so the UI leaves "Working").
        //
        // Previously this sent a StopSession message with an extra `interrupt`
        // flag, but the agent dispatches purely on `type`: it hit
        // handle_stop_session, which KILLED the durable session AND never stopped
        // the turn (so the badge stayed "Working").
        //
        // X6 (§6.1): route by RuntimeOwner — WaslManaged emits InterruptWaslSession.
        self.interrupt_session_routed(id)
    }

    /// Answer an `AskUserQuestion` gate by sending the user's chosen answers to
    /// the agent, which forwards them to claude as an `updatedInput` control_response.
    ///
    /// `id` is the marsad session id; `request_id` is the control-request id captured
    /// in the `Gate::Question`; `answers` maps question text to the selected label.
    pub fn answer_question(
        &mut self,
        id: &str,
        request_id: &str,
        answers: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;
        self.send_ipc(&marsad_proto::IpcMsg::AnswerQuestion {
            session_id: id.to_string(),
            request_id: request_id.to_string(),
            answers,
        })?;
        // Clear the answered question gate immediately (same rationale as
        // answer_gate) so its card disappears on answer.
        if let Some(record) = self.sessions.get_mut(id) {
            record.state.clear_gate_on_answer();
        }
        let sv = self.make_session_view(id, self.sessions.get(id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        // Refresh the board too so the answered question's needs-you card clears.
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    pub fn answer_gate(&mut self, decision: GateDecision) -> Result<Vec<VmUpdate>, CoreError> {
        let session_id = marsad_gates::route(&decision).to_string();
        self.require_session(&session_id)?;

        // X6 (§6.1/§8): a WaslManaged session's gate is a native gate — route it
        // to the dedicated native-gate answer path (which relays
        // AnswerWaslNativeGate, not AnswerGate). Identify the gate by the
        // session's current pending Permission's request_id (== the gate_id).
        if matches!(
            self.runtime_owner(&session_id),
            RuntimeOwner::WaslManaged { .. }
        ) {
            let gate_id = self
                .sessions
                .get(&session_id)
                .and_then(|r| match &r.state.gate {
                    Some(marsad_gates::Gate::Permission { request_id, .. }) => {
                        Some(request_id.clone())
                    }
                    _ => None,
                });
            if let Some(gate_id) = gate_id {
                return self.answer_wasl_native_gate(&session_id, &gate_id, decision.decision);
            }
            // No pending native gate to answer → silent no-op (mirrors the
            // benign "nothing to answer" disposition).
            return Ok(vec![]);
        }

        // Idempotency: already recorded → silent no-op (no duplicate IPC).
        if self.ledger.seen(&decision.id) {
            return Ok(vec![]);
        }
        // record() cannot return DuplicateDecision here because seen() is false.
        // unwrap() is infallible in this code path.
        self.ledger.record(&decision).unwrap();

        let ctrl = marsad_gates::to_control_response(&decision);
        self.send_ipc(&IpcMsg::AnswerGate {
            session_id: session_id.clone(),
            request_id: ctrl.request_id,
            allow: ctrl.allow,
            deny_reason: ctrl.deny_reason,
            // For an accepted scope-elicitation, forward the chosen content
            // (e.g. { "grant": "session" }); None for everything else.
            elicitation_content: decision.elicitation_content.clone(),
            // For an accepted "always allow …" suggestion, forward the chosen
            // rule verbatim → claude's updatedPermissions; None otherwise.
            permission_updates: decision.permission_updates.clone(),
        })?;

        // Optimistically clear the pending gate NOW so the card disappears the
        // instant the human answers — don't wait for a later stream message
        // (a bare permission approval may not emit one that clears it, leaving
        // the answered gate stuck on screen). The claude stream stays the source
        // of truth: its next messages re-drive phase/gate as usual.
        if let Some(record) = self.sessions.get_mut(&session_id) {
            record.state.clear_gate_on_answer();
        }
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        // Refresh the board too: clearing the gate removes the attention card's
        // needs-you headline, so the dashboard card must update immediately.
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    /// X3 (docs/specs/wasl-connector.md §5.2): relay ONE explicit human
    /// decision on a pending Wasl gate. NO session lookup (Wasl gates are
    /// host-scoped, not session-scoped — deliberately not `require_session`).
    pub fn answer_wasl_gate(
        &mut self,
        host_id: &str,
        gate_id: &str,
        decision: marsad_gates::Decision,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let view = self
            .wasl_gates
            .get(&(host_id.to_string(), gate_id.to_string()))
            .ok_or_else(|| CoreError::UnknownWaslGate(format!("{host_id}/{gate_id}")))?;
        let gate = marsad_gates::Gate::Handoff {
            gate_id: view.gate_id.clone(),
            kind: view.kind.clone(),
            from: view.from.clone(),
            summary: view.summary.clone(),
            preview: view.preview.clone(),
        };
        let scope = format!("wasl:{host_id}");
        let gd = match &decision {
            marsad_gates::Decision::Approve => marsad_gates::approve(&scope, &gate),
            marsad_gates::Decision::Reject { reason } => {
                marsad_gates::reject(&scope, &gate, reason)
            }
        };

        // Idempotency: identical to answer_gate — seen -> silent no-op, no
        // duplicate IPC. `decision_id` is content-stable over (scope,
        // gate_id), so approve-then-deny of the same gate is ALSO deduped.
        if self.ledger.seen(&gd.id) {
            return Ok(vec![]);
        }
        // record() cannot return DuplicateDecision here because seen() is
        // false. unwrap() is infallible in this code path.
        self.ledger.record(&gd).unwrap();

        let (outcome, reason) = match gd.decision {
            marsad_gates::Decision::Approve => ("approved".to_string(), None),
            marsad_gates::Decision::Reject { reason } => ("denied".to_string(), Some(reason)),
        };
        // Node-authoritative: the pending list changes only on WaslGateDecided,
        // so the success value is always an empty update list; `.map` (rather
        // than `send_ipc(...)?; Ok(vec![])`) keeps that a single expression.
        self.send_ipc(&IpcMsg::AnswerWaslGate {
            host_id: host_id.to_string(),
            gate_id: gate_id.to_string(),
            outcome,
            reason,
        })
        .map(|()| vec![])
    }

    /// X6 (Wasl Backend, §8): answer a NATIVE gate on a WaslManaged session.
    /// SESSION-scoped (not host-scoped like the X1 operator gate): the gate is a
    /// `Gate::Permission` in the session's own gate slot, exactly as an opencode
    /// ACP permission is. Records the decision in the EXISTING `DecisionLedger`
    /// (idempotent — a second answer, even a different verdict, is a silent
    /// no-op) and emits ONE `AnswerWaslNativeGate`. Answering a gate that was
    /// never requested is an error (the session-scoped analogue of
    /// `UnknownWaslGate`), not a silent success.
    ///
    /// Does NOT touch the host-scoped `wasl_gates` / `VmUpdate::WaslGates` list —
    /// that stays exclusively for X1 operator handoff gates (regression pin).
    pub fn answer_wasl_native_gate(
        &mut self,
        session_id: &str,
        gate_id: &str,
        decision: marsad_gates::Decision,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(session_id)?;

        // Reconstruct the exact Gate::Permission this native gate maps to, so the
        // derived DecisionId is the SAME as the one the request path would use
        // (the command encodes gate_id via native_gate_command → stable+unique).
        // A gate that was never requested (no stored detail) is an error.
        let detail = self
            .sessions
            .get(session_id)
            .and_then(|r| r.wasl_native_gates.get(gate_id).cloned())
            .ok_or_else(|| CoreError::UnknownWaslGate(format!("{session_id}/{gate_id}")))?;
        let gate = native_gate_permission(gate_id, &detail);

        let gd = match &decision {
            marsad_gates::Decision::Approve => marsad_gates::approve(session_id, &gate),
            marsad_gates::Decision::Reject { reason } => {
                marsad_gates::reject(session_id, &gate, reason)
            }
        };

        // Idempotency: identical to answer_gate — seen → silent no-op, no
        // duplicate IPC. `decision_id` is content-stable over (session_id,
        // command+cwd), so approve-then-deny of the same gate is ALSO deduped.
        if self.ledger.seen(&gd.id) {
            return Ok(vec![]);
        }
        // record() cannot return DuplicateDecision here because seen() is false.
        self.ledger.record(&gd).unwrap();

        let (outcome, reason) = match gd.decision {
            marsad_gates::Decision::Approve => ("approved".to_string(), None),
            marsad_gates::Decision::Reject { reason } => ("denied".to_string(), Some(reason)),
        };
        self.send_ipc(&IpcMsg::AnswerWaslNativeGate {
            session_id: session_id.to_string(),
            gate_id: gate_id.to_string(),
            outcome,
            reason,
        })?;

        // Optimistically clear the pending gate now so the card disappears the
        // instant the human answers (mirrors answer_gate). The Wasl observe
        // stream stays the source of truth (a follow-on status/event re-drives
        // phase/gate).
        // NOTE: the gate DETAIL stays in `wasl_native_gates` on purpose — a
        // second answer for the same gate must reconstruct the same DecisionId so
        // the ledger dedup makes it a silent no-op (rather than erroring as an
        // unknown gate). Only the UI-facing gate slot is cleared.
        if let Some(record) = self.sessions.get_mut(session_id) {
            record.state.clear_gate_on_answer();
        }
        let sv = self.make_session_view(session_id, self.sessions.get(session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    pub fn run_slash(
        &mut self,
        id: &str,
        cmd: marsad_viewmodel::SlashCmd,
    ) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::SlashCmd {
            session_id: id.to_string(),
            cmd: slash_cmd_name(&cmd).to_string(),
        })
    }

    pub fn set_workspace(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.active_ws = Some(id.to_string());
        // Persist the active workspace if it exists in our list; best-effort.
        if self.workspaces.iter().any(|w| w.id == id) {
            let _ = self.store.set_active_workspace(id);
        }
        // Best-effort advisory send; ignore errors.
        let line = encode_ipc(&IpcMsg::SetWorkspace {
            workspace_id: id.to_string(),
        });
        let _ = self.transport.send_line(&line);
        Ok(vec![self.workspaces_update()])
    }

    // -----------------------------------------------------------------------
    // Workstream commands (B12 — additive)
    // -----------------------------------------------------------------------

    // (gate_headline is a free fn — see below node projection helpers.)

    /// Project one session into a `marsad_workstream::Node`, mirroring
    /// `make_session_view`. Caller guarantees `id` has a membership entry.
    fn node_of(&self, id: &str, record: &SessionRecord) -> marsad_workstream::Node {
        let ws = self.membership.get(id).cloned().unwrap_or_default();
        let lead = self.ws_leads.get(&ws).map(String::as_str) == Some(id);
        let driver = match record.driver.as_str() {
            "codex" => marsad_workstream::Driver::Codex,
            "gemini" => marsad_workstream::Driver::Gemini,
            _ => marsad_workstream::Driver::Claude,
        };
        marsad_workstream::Node {
            id: id.to_string(),
            ws,
            name: record
                .name
                .clone()
                .unwrap_or_else(|| session_display_name(&record.project, id)),
            driver,
            host: record.host_id.clone(),
            state: record.state.phase.clone(),
            lead,
            branch: String::new(),
            live: record.live,
            end_reason: record.state.end_reason,
            context_pct: record.state.context_pct.round().clamp(0.0, 100.0) as u8,
            // Surface a concise pending-gate summary so the workstream board's
            // attention card can show WHAT needs the human, not just the name.
            status: record
                .state
                .gate
                .as_ref()
                .map(gate_headline)
                .unwrap_or_default(),
        }
    }

    /// One node per session that HAS a membership entry, in `session_order`.
    pub fn nodes(&self) -> Vec<marsad_workstream::Node> {
        self.session_order
            .iter()
            .filter(|id| self.membership.contains_key(id.as_str()))
            .filter_map(|id| self.sessions.get(id).map(|record| self.node_of(id, record)))
            .collect()
    }

    fn workstreams_update(&self) -> VmUpdate {
        VmUpdate::Workstreams(self.workstreams.clone())
    }

    /// Current workstream list for bridge commands that need to push a
    /// Workstreams VmUpdate after mutating state (e.g. remove_session).
    pub fn workstreams(&self) -> &[marsad_workstream::Workstream] {
        &self.workstreams
    }

    /// Restore workstream state from the store, then run the (idempotent)
    /// legacy-workspace / unassigned-session migration.
    ///
    /// Mirrors `load_workspaces`/`load_sessions`/`load_hosts`: the initial
    /// `restore()` is the hard-error path (propagated as `CoreError::Agent`);
    /// every migration write after that is best-effort (mirrors
    /// `set_workspace`'s `set_active_workspace`) so the in-memory Core state
    /// — and therefore the returned `VmUpdate`s and subsequent `board()`/
    /// `nodes()` calls — stays correct even if an underlying persistence
    /// write fails.
    pub fn load_workstreams(&mut self) -> Result<Vec<VmUpdate>, CoreError> {
        let restored = self
            .store
            .restore()
            .map_err(|e| CoreError::Agent(format!("restore workstreams: {e:?}")))?;
        self.workstreams = restored.workstreams;
        self.connections = restored.connections;
        self.profiles = restored.profiles;
        self.membership = restored.membership;
        // Rebuild ws_leads from the restored workstreams' single-node fast lanes
        // is not possible from persisted data alone (lead is a Core-only concept
        // not persisted separately); migration below re-derives leads for any
        // newly-created fast-lane workstreams. Pre-existing leads are inferred
        // from membership: a session that is the ONLY member of its workstream
        // and was previously marked lead by a prior migration run remains so
        // because ws_leads itself is only ever populated by Core, in-memory, and
        // migration is idempotent per the checks below.

        // 1. Seed profiles from legacy workspaces if none exist yet.
        if self.profiles.is_empty() {
            let store_workspaces: Vec<StoreWorkspace> =
                self.workspaces.iter().map(Self::vm_ws_to_store).collect();
            let seeded = marsad_store::profiles_from_workspaces(&store_workspaces);
            for p in &seeded {
                let _ = self.store.upsert_profile(p);
            }
            self.profiles = seeded;
        }

        // 2. Every session with no membership entry becomes a single-node
        //    fast-lane workstream, led by itself.
        let unassigned: Vec<String> = self
            .session_order
            .iter()
            .filter(|id| !self.membership.contains_key(id.as_str()))
            .cloned()
            .collect();
        for id in unassigned {
            // `id` comes from `session_order`, which always has a matching
            // `sessions` entry (the two are only ever mutated together).
            let record = self
                .sessions
                .get(&id)
                .expect("session_order entries always have a sessions record");
            let name = record
                .name
                .clone()
                .unwrap_or_else(|| session_display_name(&record.project, &id));
            let ws_id = format!("ws-{id}");
            let ws = marsad_workstream::Workstream {
                id: ws_id.clone(),
                name,
                goal: String::new(),
                profile: None,
            };
            let _ = self.store.upsert_workstream(&ws);
            self.workstreams.push(ws);
            self.membership.insert(id.clone(), ws_id.clone());
            self.ws_leads.insert(ws_id, id);
        }
        let _ = self.store.set_membership(&self.membership);

        Ok(vec![
            self.workstreams_update(),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Create (or overwrite, upsert semantics) a workstream.
    pub fn create_workstream(
        &mut self,
        id: String,
        name: String,
        goal: String,
        profile: Option<String>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let ws = marsad_workstream::Workstream {
            id: id.clone(),
            name,
            goal,
            profile,
        };
        self.store
            .upsert_workstream(&ws)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        if let Some(pos) = self.workstreams.iter().position(|w| w.id == id) {
            self.workstreams[pos] = ws;
        } else {
            self.workstreams.push(ws);
        }
        Ok(vec![
            self.workstreams_update(),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Pause (stop) all sessions in a workstream. Mirrors `stop_session` for
    /// each member: sends `StopSession` IPC + marks phase Stale in memory.
    /// Sessions remain resumable. Absent workstream or no members → no-op `Ok`.
    pub fn pause_workstream(&mut self, ws_id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        // Collect member session ids (cloned to avoid borrowing self).
        let session_ids: Vec<String> = self
            .membership
            .iter()
            .filter(|(_, ws)| ws.as_str() == ws_id)
            .map(|(sid, _)| sid.clone())
            .collect();
        let mut all_updates: Vec<VmUpdate> = Vec::new();
        for sid in session_ids {
            // stop_session is require_session-guarded: skip unknown sessions gracefully.
            if let Ok(updates) = self.stop_session(&sid) {
                all_updates.extend(updates);
            }
        }
        Ok(all_updates)
    }

    /// Resume all sessions in a workstream. Mirrors `resume_session` for each
    /// member. Absent workstream or no members → no-op `Ok`.
    pub fn resume_workstream(&mut self, ws_id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let session_ids: Vec<String> = self
            .membership
            .iter()
            .filter(|(_, ws)| ws.as_str() == ws_id)
            .map(|(sid, _)| sid.clone())
            .collect();
        let mut all_updates: Vec<VmUpdate> = Vec::new();
        for sid in session_ids {
            if let Ok(updates) = self.resume_session(&sid) {
                all_updates.extend(updates);
            }
        }
        Ok(all_updates)
    }

    /// Delete a workstream: first kills all member sessions (remove_session for
    /// each — StopSession IPC + drops record + removes from store so they don't
    /// reappear on relaunch), THEN deletes the workstream record, cascades its
    /// connections and membership/lead entries. Deleting an absent id is a no-op `Ok`.
    pub fn delete_workstream(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        // 1. Kill + remove all member sessions first (so they don't orphan a
        //    running process, and so deleted sessions vanish from the dashboard).
        let session_ids: Vec<String> = self
            .membership
            .iter()
            .filter(|(_, ws)| ws.as_str() == id)
            .map(|(sid, _)| sid.clone())
            .collect();
        let mut session_updates: Vec<VmUpdate> = Vec::new();
        for sid in session_ids {
            // remove_session is require_session-guarded: skip unknown sessions gracefully.
            if let Ok(updates) = self.remove_session(&sid) {
                session_updates.extend(updates);
            }
        }

        // 2. Delete the workstream record and its connections.
        let _ = self.store.delete_workstream(id);
        self.workstreams.retain(|w| w.id != id);

        let conn_ids: Vec<String> = self
            .connections
            .iter()
            .filter(|c| c.ws == id)
            .map(|c| c.id.clone())
            .collect();
        for cid in conn_ids {
            let _ = self.store.delete_connection(&cid);
        }
        self.connections.retain(|c| c.ws != id);

        self.membership.retain(|_, ws| ws != id);
        let _ = self.store.set_membership(&self.membership);
        self.ws_leads.remove(id);

        // 3. Return session-removed events + board updates so the UI stays consistent.
        let mut updates = session_updates;
        updates.push(self.workstreams_update());
        updates.push(VmUpdate::Board(self.board()));
        Ok(updates)
    }

    /// Validate and add a connection between two nodes in the same workstream.
    pub fn add_connection(
        &mut self,
        id: String,
        ws: String,
        from: String,
        to: String,
        kind: marsad_workstream::ConnKind,
        bi: bool,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let conn = marsad_workstream::Connection {
            id,
            ws,
            from,
            to,
            kind,
            bi,
            gate: None,
        };
        marsad_workstream::validate_connection(&conn, &self.nodes(), &self.connections)
            .map_err(|e| CoreError::Connection(e.to_string()))?;
        self.store
            .upsert_connection(&conn)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        self.connections.push(conn);
        Ok(vec![VmUpdate::Board(self.board())])
    }

    /// Remove a connection by id. Absent id is a no-op `Ok`.
    pub fn remove_connection(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.store
            .delete_connection(id)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        self.connections.retain(|c| c.id != id);
        Ok(vec![VmUpdate::Board(self.board())])
    }

    /// Create (or overwrite, upsert semantics) a profile.
    pub fn create_profile(&mut self, id: String, name: String) -> Result<Vec<VmUpdate>, CoreError> {
        let profile = marsad_workstream::Profile {
            id: id.clone(),
            name,
        };
        self.store
            .upsert_profile(&profile)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        // Upsert: drop any existing entry with the same id, then push — a
        // single straight-line path regardless of whether `id` was already
        // present (matches store upsert semantics without a branch).
        self.profiles.retain(|p| p.id != id);
        self.profiles.push(profile);
        Ok(vec![VmUpdate::Board(self.board())])
    }

    /// Delete a profile: un-sets it on member workstreams and clears
    /// `active_profile` if it pointed at the deleted profile. Absent id is a
    /// no-op `Ok`.
    pub fn delete_profile(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let _ = self.store.delete_profile(id);
        self.profiles.retain(|p| p.id != id);

        let affected: Vec<String> = self
            .workstreams
            .iter()
            .filter(|w| w.profile.as_deref() == Some(id))
            .map(|w| w.id.clone())
            .collect();
        for ws_id in affected {
            let ws = self
                .workstreams
                .iter_mut()
                .find(|w| w.id == ws_id)
                .expect("id collected from self.workstreams above");
            ws.profile = None;
            let _ = self.store.upsert_workstream(ws);
        }

        if self.active_profile.as_deref() == Some(id) {
            self.active_profile = None;
        }

        Ok(vec![
            self.workstreams_update(),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Assign a session to a workstream, optionally making it the lead.
    pub fn assign_session(
        &mut self,
        session_id: &str,
        ws_id: &str,
        lead: bool,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(session_id)?;
        if !self.workstreams.iter().any(|w| w.id == ws_id) {
            return Err(CoreError::Connection(format!(
                "unknown workstream: {ws_id}"
            )));
        }
        // `launch_session_with_id` auto-creates a single-node fast-lane
        // workstream `ws-<id>` for every fresh launch. When the caller (NodeModal
        // build-it-out) then assigns the session to a DIFFERENT workstream, that
        // auto one is orphaned — it would linger as an empty second card on the
        // board. Remove it here so a build-it-out launch shows exactly one card.
        let auto_ws_id = format!("ws-{session_id}");
        if auto_ws_id != ws_id && self.workstreams.iter().any(|w| w.id == auto_ws_id) {
            let _ = self.store.delete_workstream(&auto_ws_id);
            self.workstreams.retain(|w| w.id != auto_ws_id);
            self.ws_leads.remove(&auto_ws_id);
        }
        self.membership
            .insert(session_id.to_string(), ws_id.to_string());
        if lead {
            self.ws_leads
                .insert(ws_id.to_string(), session_id.to_string());
        }
        self.store
            .set_membership(&self.membership)
            .map_err(|e| CoreError::Agent(format!("{e:?}")))?;
        Ok(vec![
            self.workstreams_update(),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Set the board's active profile filter. `None` shows all workstreams.
    pub fn set_active_profile(&mut self, profile: Option<String>) -> Vec<VmUpdate> {
        self.active_profile = profile;
        vec![VmUpdate::Board(self.board())]
    }

    /// Recompute the workstreams board.
    pub fn board(&self) -> marsad_viewmodel::BoardView {
        marsad_viewmodel::board_view(
            &self.workstreams,
            &self.nodes(),
            &self.connections,
            self.hosts.list(),
            &[],
            self.active_profile.as_deref(),
        )
    }

    /// Recompute the Level-2 dependency graph for one workstream.
    pub fn graph(&self, ws_id: &str) -> Option<marsad_viewmodel::GraphView> {
        marsad_viewmodel::graph_view(ws_id, &self.workstreams, &self.nodes(), &self.connections)
    }

    /// Recompute the Usage tab's cost breakdown ("By host" / "By node") for
    /// one workstream's sessions, or `None` if `ws_id` does not match any
    /// workstream.
    pub fn cost_breakdown(&self, ws_id: &str) -> Option<marsad_viewmodel::WorkstreamCostBreakdown> {
        let sessions: Vec<marsad_viewmodel::SessionView> = self
            .session_order
            .iter()
            .filter_map(|id| {
                self.sessions
                    .get(id)
                    .map(|record| self.make_session_view(id, record))
            })
            .collect();
        marsad_viewmodel::workstream_cost_breakdown(
            ws_id,
            &self.workstreams,
            &self.nodes(),
            &sessions,
        )
    }

    // -----------------------------------------------------------------------
    // U1 dynamic capability discovery (docs/specs/dynamic-capabilities.md §3.5)
    // -----------------------------------------------------------------------

    /// Cloned snapshot of every (driver, host) capability entry seen so far
    /// (lock-only, no IO — mirrors `board()`).
    pub fn capabilities(&self) -> Vec<marsad_viewmodel::CapabilitiesView> {
        self.capabilities
            .values()
            .cloned()
            .map(marsad_viewmodel::CapabilitiesView::from)
            .collect()
    }

    /// Ask the agent to (re)probe capabilities: a single IPC line write, same
    /// cost class as [`Core::interrupt`] — never waits for a reply. Results
    /// arrive later as `CapabilitiesUpdate` broadcasts via `on_agent_line`.
    pub fn refresh_capabilities(
        &mut self,
        host_id: Option<String>,
        driver: Option<String>,
        project: Option<String>,
    ) {
        // Resolve the host's launch config from the registry so the agent's probe
        // runs setup-first over the RIGHT connection with the EXACT per-driver
        // command a launch would use (real discovery, not a local guess). A
        // missing host / the local host resolves to empty config + `None` target
        // (the agent then discovers locally, setup still honored). An unknown
        // host id (never registered) leaves everything defaulted — honest.
        let (host_target, setup, agent_commands, claude_bin) = host_id
            .as_deref()
            .and_then(|hid| self.hosts.list().iter().find(|h| h.id == hid))
            .map(|h| {
                (
                    // Only remote hosts carry a shell target; the local host runs
                    // discovery in-process (target `None`).
                    if h.is_local {
                        None
                    } else {
                        Some(h.target.clone())
                    },
                    h.setup.clone(),
                    h.agent_commands.clone(),
                    h.claude_bin.clone(),
                )
            })
            .unwrap_or((None, None, vec![], None));
        let _ = self.send_ipc(&IpcMsg::RefreshCapabilities {
            host_id,
            driver,
            project,
            host_target,
            setup,
            agent_commands,
            claude_bin,
        });
    }

    /// Connect-time discovery (deliverable #4a): kick off a full capabilities
    /// refresh (all drivers) for EVERY registered host, so capabilities are
    /// TRULY populated the moment Marsad comes up — not only when the frontend
    /// asks. Called once at startup after `load_hosts`. Each host is a single
    /// fire-and-forget `RefreshCapabilities` line (no probe IO on this path —
    /// the agent runs the probes on background threads, §4), so this is O(hosts)
    /// cheap sends with no blocking on the Core/agent lock. The agent's own
    /// bounded-probe deadline handles an unreachable host (reports
    /// `cli_available:false` + an error, never a hang).
    pub fn refresh_capabilities_all_hosts(&mut self) {
        let host_ids: Vec<String> = self.hosts.list().iter().map(|h| h.id.clone()).collect();
        for host_id in host_ids {
            self.refresh_capabilities(Some(host_id), None, None);
        }
    }

    /// A clone of the current host registry (read-only snapshot). Lets callers
    /// map a host `target` back to its `id` (e.g. the desktop's
    /// `connect_host_interactive` resolving which host to discover after an
    /// interactive login) without exposing the registry itself.
    pub fn hosts_snapshot(&self) -> Vec<marsad_hosts::Host> {
        self.hosts.list().to_vec()
    }

    // -----------------------------------------------------------------------
    // Inbound IPC parsing — central testable seam
    // -----------------------------------------------------------------------

    pub fn on_agent_line(&mut self, line: &str) -> Result<Vec<VmUpdate>, CoreError> {
        let msg = match marsad_proto::parse_ipc(line) {
            Ok(m) => m,
            // Tolerant skip: a FUTURE inbound type (newer agent) must not error
            // the reader loop. The raw line is preserved via stderr for
            // observability.
            //
            // SPEC CHANGE (docs/specs/typed-ipc.md §7, "Tests that MUST
            // change"): crates/marsad-core/tests/errors.rs:359-369
            // (`on_agent_line_unknown_ipc_type_returns_protocol_error`) was
            // rewritten to `on_agent_line_unknown_ipc_type_is_tolerant_noop`,
            // asserting `Ok(vec![])` here, per §4's unknown-type disposition
            // table.
            Err(marsad_proto::ProtoError::UnknownDiscriminant(t)) => {
                eprintln!("marsad-core: on_agent_line: skipping unknown IPC type '{t}': {line}");
                return Ok(vec![]);
            }
            // Malformed JSON / missing type / wrong-typed known fields stay
            // HARD errors.
            Err(e) => return Err(CoreError::Protocol(e.to_string())),
        };
        match msg {
            IpcMsg::AgentEvent { session_id, event } => self.handle_agent_event(session_id, event),
            IpcMsg::TranscriptBackfill { session_id, jsonl } => {
                self.handle_transcript_backfill(session_id, jsonl)
            }
            IpcMsg::ChangesUpdate {
                session_id,
                changes,
            } => self.handle_changes_update(session_id, changes),
            IpcMsg::UsageUpdate { session_id, usage } => {
                self.handle_usage_update(session_id, usage)
            }
            // Two-axis lifecycle: authoritative Resumed/Paused from the agent.
            IpcMsg::SessionLiveness { session_id, live } => {
                self.handle_session_liveness(&session_id, live)
            }
            // Per-session connection health (Connected/Reconnecting/Unstable) from
            // the agent's reader — drives the disconnected UI + composer hard-block.
            IpcMsg::SessionConn { session_id, health } => {
                self.handle_session_conn(&session_id, health)
            }
            // U1: one (driver, host) snapshot — always a COMPLETE replace, never
            // a merge (dynamic-capabilities.md §2/§3.5).
            IpcMsg::CapabilitiesUpdate {
                host_id,
                driver,
                mut capabilities,
            } => {
                // Merge sessions by project so a path-scoped probe and the
                // connect-time capability probe (which runs concurrently and
                // carries an EMPTY session list) don't clobber each other. The
                // incoming `session_scope` says what this update authoritatively
                // covers:
                //   * AllKnown  → a capability probe: keep the sessions we already
                //     had (its own list is empty; overwriting would flip the picker
                //     to "none discovered" right after a real result landed).
                //   * Projects(p) → replace exactly project p's rows with this
                //     update's rows (an empty result correctly clears p), leaving
                //     every other project's rows intact.
                let key = (driver.clone(), host_id.clone());
                if let Some(existing) = self.capabilities.get(&key) {
                    match &capabilities.session_scope {
                        Some(marsad_proto::SessionScope::Projects(ps)) => {
                            let replacing: std::collections::HashSet<&str> =
                                ps.iter().map(|p| p.as_str()).collect();
                            let mut merged: Vec<marsad_proto::DiscoveredSession> = existing
                                .sessions
                                .iter()
                                .filter(|s| {
                                    s.project
                                        .as_deref()
                                        .map(|p| !replacing.contains(p))
                                        .unwrap_or(false)
                                })
                                .cloned()
                                .collect();
                            merged.extend(capabilities.sessions.iter().cloned());
                            capabilities.sessions = merged;
                        }
                        _ => {
                            capabilities.sessions = existing.sessions.clone();
                        }
                    }
                }
                self.capabilities.insert(key, capabilities.clone());
                let mut updates = vec![VmUpdate::Capabilities(capabilities.into())];
                // Record this driver as discovered for the host. Once every probed
                // driver has landed, a host that was Connecting (reachable, discovery
                // in flight) is promoted to Connected — agents + versioned models are
                // now loaded and the launch modals will show the real truth.
                self.caps_loaded
                    .entry(host_id.clone())
                    .or_default()
                    .insert(driver);
                if self.host_caps_fully_loaded(&host_id) {
                    let is_connecting = self
                        .hosts
                        .list()
                        .iter()
                        .find(|h| h.id == host_id)
                        .map(|h| h.status == HostStatus::Connecting)
                        .unwrap_or(false);
                    if is_connecting {
                        self.hosts.set_status(&host_id, HostStatus::Connected);
                        updates.push(VmUpdate::Hosts(self.hosts.list().to_vec()));
                    }
                }
                Ok(updates)
            }
            // X3 (docs/specs/wasl-connector.md §5.2): a Wasl gate surfaced (or
            // re-surfaced via resnapshot reconcile).
            IpcMsg::WaslGateRequested {
                host_id,
                gate_id,
                decidable,
                request,
                requested_at,
                deadline,
                ..
            } => self.handle_wasl_gate_requested(
                host_id,
                gate_id,
                decidable,
                request,
                requested_at,
                deadline,
            ),
            // X3: a Wasl gate resolved.
            IpcMsg::WaslGateDecided {
                host_id,
                gate_id,
                outcome,
                decided_by,
                reason,
                ..
            } => self.handle_wasl_gate_decided(host_id, gate_id, outcome, decided_by, reason),
            // X3: one audit-feed entry (the observation feed).
            IpcMsg::WaslObservation { host_id, entry } => {
                self.handle_wasl_observation(host_id, entry)
            }
            // X3: serve-loop ack for `AnswerWaslGate` — joins the existing
            // ack no-op list.
            IpcMsg::WaslGateAnswered { .. } => Ok(vec![]),
            // X6 (Wasl Backend, §6.3/§7): a WaslManaged session's advisory status
            // changed — project it onto the existing Phase (NO new variant).
            IpcMsg::WaslSessionStatus { session_id, status } => {
                self.handle_wasl_session_status(session_id, status)
            }
            // X6 (§7/§8/§10.4): one normalized observe event — fold it into the
            // transcript + native-gate projection.
            IpcMsg::WaslSessionEvent {
                session_id,
                seq,
                event,
            } => self.handle_wasl_session_event(session_id, seq, event),
            // X6 (§10.4): a prompt receipt — surface queued/rejected outcomes
            // visibly (not a silent drop).
            IpcMsg::WaslPromptReceipt {
                session_id,
                receipt,
            } => self.handle_wasl_prompt_receipt(session_id, receipt),
            // X6: serve-loop ack for `AnswerWaslNativeGate` — no-op (the follow-on
            // status/event is authoritative), joining the ack no-op list.
            IpcMsg::WaslNativeGateAnswered { .. } => Ok(vec![]),
            // Command acks legitimately arrive on the subscriber connection.
            IpcMsg::SessionLaunched { .. }
            | IpcMsg::SessionResumed { .. }
            | IpcMsg::SessionAttached { .. }
            | IpcMsg::SessionStopped { .. }
            | IpcMsg::PromptSent { .. }
            | IpcMsg::Interrupted { .. }
            | IpcMsg::GateAnswered { .. }
            | IpcMsg::QuestionAnswered { .. }
            | IpcMsg::Subscribed { .. }
            | IpcMsg::CapabilitiesRefreshQueued { .. }
            | IpcMsg::Error { .. } => Ok(vec![]),
            // A GUI→agent message arriving inbound is a routing bug: hard error
            // (same observable class as today's "unknown IPC type" Protocol error).
            other => Err(CoreError::Protocol(format!(
                "misdirected IPC message on agent channel: {other:?}"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // X6 — Wasl Backend inbound folding (wasl-backend-plan.md §6.3/§7/§8/§10.4)
    // -----------------------------------------------------------------------

    /// Project a WaslManaged session's advisory status onto the EXISTING `Phase`
    /// (§6.3), using the REAL `marsad_wasl_session::project_status_to_phase` (not
    /// a reimplementation) — so the projection stays in one place. Unknown
    /// session → tolerant no-op; malformed status Value → tolerant no-op (a
    /// future/garbled status must not error the reader loop).
    fn handle_wasl_session_status(
        &mut self,
        session_id: String,
        status: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if !self.sessions.contains_key(&session_id) {
            return Ok(vec![]);
        }
        let st: marsad_wasl_session::WaslTargetStatus = match serde_json::from_value(status) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("marsad-core: WaslSessionStatus for {session_id}: bad status: {e}");
                return Ok(vec![]);
            }
        };
        let phase = marsad_wasl_session::project_status_to_phase(&st);
        let live = marsad_wasl_session::status_is_live(&st);
        let record = self.sessions.get_mut(&session_id).unwrap();
        record.state.phase = phase;
        record.live = live;
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Fold ONE normalized Wasl observe event into the session's transcript +
    /// native-gate projection (§7/§8/§10.4). Handles every §10.4 edge case:
    /// dedup by `seq`, delta-append to one entry, single terminal close,
    /// post-terminal delta ignore, and native-gate → `Gate::Permission`. Unknown
    /// session / malformed event / duplicate seq → tolerant no-op.
    fn handle_wasl_session_event(
        &mut self,
        session_id: String,
        seq: u64,
        event: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if !self.sessions.contains_key(&session_id) {
            // An event for a session core never launched: tolerant no-op (no
            // panic, no cross-session corruption).
            return Ok(vec![]);
        }
        let ev: marsad_wasl_session::WaslSessionEvent = match serde_json::from_value(event) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("marsad-core: WaslSessionEvent for {session_id}: bad event: {e}");
                return Ok(vec![]);
            }
        };

        // Dedup by seq: a replay overlap after an ObserveLost/reattach must fold
        // exactly once. A duplicate seq is a silent no-op (no updates).
        {
            let record = self.sessions.get_mut(&session_id).unwrap();
            if !record.wasl_seen_seqs.insert(seq) {
                return Ok(vec![]);
            }
            // Advance the "last observed seq" (§6.1 step 2) and persist it so a
            // reconnect re-attaches from here. Best-effort persist.
            if seq > record.wasl_last_seq {
                record.wasl_last_seq = seq;
            }
        }
        self.persist_wasl_last_seq(&session_id);

        use marsad_wasl_session::WaslSessionEventKind as K;
        let record = self.sessions.get_mut(&session_id).unwrap();
        match ev.kind {
            K::TurnAccepted { .. } => {
                // A new turn begins: it is now in-flight (Working), a fresh
                // assistant entry will open on the first delta, and the
                // post-terminal guard is cleared.
                record.state.phase = Phase::Working;
                record.wasl_turn_closed = false;
                record.wasl_open_assistant = None;
            }
            K::AssistantDelta { text, .. } => {
                // Post-terminal stray (a delta after this turn's TurnCompleted /
                // TurnFailed with no newer TurnAccepted) → ignored, never folded.
                if !record.wasl_turn_closed {
                    match record.wasl_open_assistant {
                        Some(idx) => {
                            if let Some(marsad_events::TranscriptEntry::Assistant {
                                text: existing,
                                ..
                            }) = record.transcript.get_mut(idx)
                            {
                                existing.push_str(&text);
                            }
                        }
                        None => {
                            record
                                .transcript
                                .push(marsad_events::TranscriptEntry::Assistant {
                                    text,
                                    at: Some(now_ms()),
                                });
                            record.wasl_open_assistant = Some(record.transcript.len() - 1);
                        }
                    }
                }
            }
            K::ToolStarted { name, .. } => {
                record
                    .transcript
                    .push(marsad_events::TranscriptEntry::Tool {
                        name,
                        input: String::new(),
                        at: Some(now_ms()),
                    });
            }
            K::ToolFinished { .. } => {
                // Tool completion carries no new transcript content Marsad folds;
                // the ToolStarted row already recorded the activity.
            }
            K::NativeGateRequested { turn_id, detail } => {
                // Map to the EXISTING Gate::Permission pipeline (§8), session
                // scoped. Per-occurrence identity is (turn_id, seq) (§8, v3): the
                // real detail has no gate_id, and turn_id alone is NOT unique per
                // gate (two native gates can fire in one turn), so the event's own
                // `seq` is combined with turn_id via native_gate_key. Store the
                // detail keyed by that gate_key so answer_wasl_native_gate can
                // reconstruct the same gate (→ stable+unique DecisionId).
                let gate_key = marsad_wasl_session::native_gate_key(&turn_id, seq);
                let gate = native_gate_permission(&gate_key, &detail);
                record.wasl_native_gates.insert(gate_key, detail);
                record.state.gate = Some(gate);
                record.state.phase = Phase::Needs;
            }
            K::TurnCompleted { reply, .. } => {
                close_wasl_turn(record, &reply);
                // A completed turn is not awaiting input.
                record.state.phase = Phase::Done;
            }
            K::TurnFailed { error, .. } => {
                close_wasl_turn(record, &error);
                record.state.phase = Phase::Error;
            }
            // Informational lifecycle/correlation events Marsad folds no
            // transcript content from (§4.4, v3): SessionReady is the connect
            // ping; TurnQueued/StatusChanged/NativeGateResolved/TurnCancelled/
            // TurnExpired are correlation/status signals whose UI effect (queued
            // indicator, phase) is already driven by the WaslSessionStatus
            // projection and the receipt path, so folding them here would
            // double-count. No-op (never a transcript row, never a panic).
            K::SessionReady { .. }
            | K::TurnQueued { .. }
            | K::NativeGateResolved { .. }
            | K::TurnCancelled { .. }
            | K::TurnExpired { .. }
            | K::StatusChanged { .. } => {}
            K::SessionExited { .. } => {
                record.live = false;
                record.state.phase = Phase::Done;
            }
            K::ObserveLost { .. } => {
                record.state.phase = Phase::Detached;
            }
            K::Unknown(tag) => {
                eprintln!("marsad-core: WaslSessionEvent for {session_id}: unknown kind '{tag}'");
            }
        }

        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Persist the WaslManaged session's advanced `wasl_last_seq` into its store
    /// binding so a reconnect re-attaches from the last observed seq (§6.1).
    /// Best-effort — a store error never fails event folding.
    fn persist_wasl_last_seq(&mut self, session_id: &str) {
        // Caller (`handle_wasl_session_event`) verified the session exists
        // immediately before calling, and the single-threaded `&mut self` call
        // path removes nothing in between — so the session is always present.
        let r = self
            .sessions
            .get(session_id)
            .expect("persist_wasl_last_seq called with a session verified present by the caller");
        let (binding, last_seq) = match &r.runtime_owner {
            RuntimeOwner::WaslManaged { binding } => (binding.clone(), r.wasl_last_seq),
            RuntimeOwner::MarsadManaged => return,
        };
        let _ = self
            .store
            .upsert_wasl_binding(&marsad_store::PersistedWaslBinding {
                session_id: session_id.to_string(),
                target_id: binding.target_id,
                host_id: binding.host_id,
                provider_key: binding.provider_key,
                last_seq,
            });
    }

    /// Surface a Wasl prompt receipt VISIBLY (§10.4): a `Queued`/`RejectedBusy`
    /// receipt is a toast + session update, not a silent drop. A `Queued` prompt
    /// is NOT shown as an in-flight (Working) turn — only `TurnAccepted`/`Started`
    /// makes it in-flight. Unknown session / malformed receipt → tolerant no-op.
    fn handle_wasl_prompt_receipt(
        &mut self,
        session_id: String,
        receipt: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if !self.sessions.contains_key(&session_id) {
            return Ok(vec![]);
        }
        let r: marsad_wasl_session::WaslTurnReceipt = match serde_json::from_value(receipt) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("marsad-core: WaslPromptReceipt for {session_id}: bad receipt: {e}");
                return Ok(vec![]);
            }
        };
        use marsad_wasl_session::WaslTurnReceipt as R;
        // v3: a `Duplicate { original }` is a benign resent idempotency key — Wasl
        // returned the ORIGINAL receipt wrapped verbatim with no second injection.
        // The wrapper is transparent to disposition: unwrap it (recursively, in
        // case of nesting) and surface whatever the original was (a Started stays
        // Working, a RejectedBusy still surfaces the busy signal, …), never a
        // spurious error.
        let effective = unwrap_duplicate_receipt(&r);
        let (toast, phase): (Option<(String, String)>, Option<Phase>) = match effective {
            R::Started { .. } => (None, Some(Phase::Working)),
            R::Queued { position, .. } => (
                Some((
                    "⏳".to_string(),
                    format!("Prompt queued (position {position})"),
                )),
                // A merely-queued prompt is NOT in-flight — leave the calm bucket.
                Some(Phase::Done),
            ),
            R::RejectedBusy { .. } => (
                Some((
                    "⚠".to_string(),
                    "Session busy — prompt rejected".to_string(),
                )),
                Some(Phase::Done),
            ),
            R::RejectedUnavailable { .. } => (
                Some((
                    "⚠".to_string(),
                    "Session unavailable — prompt rejected".to_string(),
                )),
                Some(Phase::Error),
            ),
            // `unwrap_duplicate_receipt` already peeled every Duplicate layer, so
            // the effective receipt is never a Duplicate here. Keep the match
            // exhaustive without a real branch.
            R::Duplicate { .. } => (None, None),
        };
        if let (Some(p), Some(record)) = (phase, self.sessions.get_mut(&session_id)) {
            record.state.phase = p;
        }
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        let mut updates = vec![VmUpdate::Session(sv)];
        if let Some((icon, text)) = toast {
            updates.push(VmUpdate::Toast { icon, text });
        }
        Ok(updates)
    }

    /// Apply a remote transcript backfill: parse the agent-shipped `.jsonl` into
    /// transcript entries and prepend them to the session's transcript (history
    /// first, ahead of any live entries). This is how a resumed/restored REMOTE
    /// session gets its prior conversation — the file lives on the remote host,
    /// so the agent reads it over the session's connection and forwards the text.
    /// Also aggregates per-model token usage (including subagent turns recorded as
    /// `isSidechain: true` entries in the JSONL) and merges it into the session's
    /// `model_usage` map (marked `estimated: true`) so the Usage tab shows a full
    /// multi-model breakdown. U4: this is a BACKFILL-ONLY estimate — it never
    /// overwrites or deletes a row a live `result` event already set for the
    /// same or a different model (see the merge logic below).
    /// Unknown session or empty history+usage → no-op (no update).
    fn handle_transcript_backfill(
        &mut self,
        session_id: String,
        jsonl_text: String,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let history = discovery::parse_transcript(&jsonl_text);
        // Aggregate per-model usage from the full JSONL (includes subagent turns).
        let model_usage_agg = discovery::aggregate_model_usage(&jsonl_text);

        if history.is_empty() && model_usage_agg.is_empty() {
            return Ok(vec![]);
        }
        let record = match self.sessions.get_mut(&session_id) {
            Some(r) => r,
            None => return Ok(vec![]),
        };
        // Re-derive activity on reconnect: a session we just re-attached to, which
        // has prior history and no active turn, is idle → Done. Backfill runs
        // before any live stream events (is_first_attach), so a truly-working
        // reconnected session's next assistant/result event re-drives the phase.
        // Only supersede a restored/unknown phase (Starting/Stale/Detached) — never
        // clobber an already-active Working/Needs/Error.
        if matches!(
            record.state.phase,
            Phase::Starting | Phase::Stale | Phase::Detached
        ) {
            record.state.phase = Phase::Done;
        }
        // Prepend history so it precedes any live entries already streamed.
        if !history.is_empty() {
            let mut combined = history;
            combined.append(&mut record.transcript);
            record.transcript = combined;
        }
        // Populate model_usage from JSONL aggregation. U4 cost truthfulness: this
        // is a BACKFILL-ONLY ESTIMATE (recomputed from the local litellm/hand
        // pricing table by `discovery::aggregate_model_usage`) — it must never
        // clobber a row a live `result` event already set (agent-reported,
        // authoritative). So: drop only the PREVIOUSLY estimated rows (a fresh
        // backfill supersedes a stale one), keep every live row untouched, and
        // skip inserting an estimated row for any model that still has a live
        // row after that.
        if !model_usage_agg.is_empty() {
            // Seed total tokens from the JSONL aggregation if no live `result`
            // event has set them yet (tokens == 0). This ensures counters show
            // the accumulated total from the transcript history immediately on
            // attach/reconnect, and then GROW as new result events arrive
            // (reduce_result replaces state.tokens with the authoritative value).
            if record.state.tokens == 0 {
                let total_tokens: u64 = model_usage_agg
                    .values()
                    .map(|s| {
                        s.input_tokens
                            .saturating_add(s.output_tokens)
                            .saturating_add(s.cache_read_input_tokens)
                            .saturating_add(s.cache_creation_input_tokens)
                    })
                    .sum();
                if total_tokens > 0 {
                    record.state.tokens = total_tokens;
                    // Also seed cost_usd (used directly in make_session_view when
                    // cost_estimated is false — but here cost_estimated will be true
                    // since we're only setting estimated rows, so make_session_view
                    // will sum the model_usage rows instead; leave cost_usd = 0.0).
                    // Recompute context_pct from the seeded token total.
                    let window = record
                        .state
                        .model
                        .as_deref()
                        .map(marsad_events::SessionState::context_window_for)
                        .unwrap_or(200_000);
                    let pct = (total_tokens as f64 / window as f64 * 100.0) as f32;
                    record.state.context_pct = pct.min(100.0);
                }
            }
            record.state.model_usage.retain(|_, mu| !mu.estimated);
            for (model, summary) in model_usage_agg {
                if record.state.model_usage.contains_key(&model) {
                    // A live row survived the retain above — never overwrite it
                    // with an estimate.
                    continue;
                }
                record.state.model_usage.insert(
                    model,
                    marsad_proto::ModelUsage {
                        input_tokens: summary.input_tokens,
                        output_tokens: summary.output_tokens,
                        cost_usd: summary.cost_usd,
                        estimated: true,
                    },
                );
            }
        }
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Apply a remote changes update: parse the agent-shipped file-change list
    /// into `Vec<FileChange>` and replace the session's changes list. This is
    /// how a REMOTE session's Changes tab stays populated — the file lives on
    /// the remote host, so the agent probes git after each result event and
    /// ships the file list here. Unknown session or empty list → no-op.
    fn handle_changes_update(
        &mut self,
        session_id: String,
        changes_val: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let parsed: Vec<FileChange> = serde_json::from_value(changes_val).unwrap_or_default();
        let record = match self.sessions.get_mut(&session_id) {
            Some(r) => r,
            None => return Ok(vec![]),
        };
        record.changes = parsed;
        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    /// Apply a project-wide usage update from the agent's python3 aggregation
    /// script.  The `usage` field is a JSON object keyed by model name with
    /// `{input, output, cache_read, cache_creation}` integer values. U4 cost
    /// truthfulness: the cost figure here is recomputed via the local
    /// litellm/hand pricing table (`discovery::model_cost_usd`) — an ESTIMATE,
    /// not agent-reported. It supersedes any previously ESTIMATED row (backfill
    /// or an earlier usage probe) but must never overwrite or delete a row a
    /// live `result` event already set for the same or a different model.
    ///
    /// Unknown session or missing session_id → error / no-op.
    /// Two-axis lifecycle: apply an authoritative liveness signal from the
    /// agent's reader thread. `true` = Resumed (process alive on its host),
    /// `false` = Paused (host reboot / OOM / scheduler kill / crash / claude
    /// exit / reconnect gave up). This is the truthful source of the
    /// Paused-on-host-death transition — never inferred from the turn phase.
    /// Unknown session or no change → no-op (avoid board churn).
    fn handle_session_liveness(
        &mut self,
        session_id: &str,
        live: bool,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        match self.sessions.get_mut(session_id) {
            Some(rec) if rec.live != live => {
                rec.live = live;
                // Connection health is meaningful only while the session is running.
                // Reset it to Connected on ANY liveness flip: coming back Resumed
                // starts clean (the reader emits Connected on attach right after);
                // going Paused/finalized must not linger on a stale
                // "Reconnecting"/"Unstable"/"Disconnected" hint.
                rec.conn_health = marsad_proto::ConnHealth::Connected;
                // Pausing kills the process → any pending gate/question is wasted
                // (unanswerable until a cold resume). Discard it so it neither
                // lingers in state nor resurfaces on resume.
                if !live {
                    rec.state.drop_gates_on_pause();
                }
            }
            _ => return Ok(vec![]),
        }
        let sv = self.make_session_view(session_id, self.sessions.get(session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    /// Per-session connection health from the agent's reader (real stream events,
    /// not the host poll). Mirrors `handle_session_liveness`. A no-op when the
    /// value is unchanged so it doesn't churn the board.
    fn handle_session_conn(
        &mut self,
        session_id: &str,
        health: marsad_proto::ConnHealth,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        match self.sessions.get_mut(session_id) {
            Some(rec) if rec.conn_health != health => rec.conn_health = health,
            _ => return Ok(vec![]),
        }
        let sv = self.make_session_view(session_id, self.sessions.get(session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        Ok(vec![
            VmUpdate::Session(sv),
            VmUpdate::Overview(groups),
            VmUpdate::Board(self.board()),
        ])
    }

    fn handle_usage_update(
        &mut self,
        session_id: String,
        usage_val: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if usage_val.is_null() {
            return Ok(vec![]); // no usage data → no-op
        }
        let usage_obj = match usage_val.as_object() {
            Some(o) => o,
            None => return Ok(vec![]), // malformed → no-op
        };

        // Parse into ModelUsageRow vec, skipping "<synthetic>" and zero-usage rows.
        let mut rows: Vec<marsad_viewmodel::ModelUsageRow> = Vec::new();
        for (model, val) in usage_obj {
            if model == "<synthetic>" {
                continue;
            }
            let input = val.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
            let output = val.get("output").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_read = val.get("cache_read").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_creation = val
                .get("cache_creation")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            // Skip zero-usage rows.
            if input == 0 && output == 0 && cache_read == 0 && cache_creation == 0 {
                continue;
            }
            let cost = discovery::model_cost_usd(model, input, output, cache_read, cache_creation);
            rows.push(marsad_viewmodel::ModelUsageRow {
                model: model.clone(),
                input_tokens: input,
                output_tokens: output,
                cost_usd: cost,
                estimated: true,
            });
        }
        rows.sort_by(|a, b| a.model.cmp(&b.model));

        // Guard: an empty rows vec means the probe returned no real usage data
        // (e.g. an empty project dir, or the script exited early). Do NOT clear
        // the existing model_usage — that would wipe good data from a previous
        // result-event or backfill. Return no update so the UI is unchanged.
        if rows.is_empty() {
            return Ok(vec![]);
        }

        // Find the session and update its model_usage.
        let record = match self.sessions.get_mut(&session_id) {
            Some(r) => r,
            None => return Ok(vec![]), // unknown session → no-op
        };
        // Seed total tokens from the usage probe if no live `result` event has
        // set them yet (tokens == 0). The usage probe gives the CUMULATIVE
        // per-project total — use it as the token baseline so counters show
        // the accumulated total on attach rather than starting from zero.
        // A subsequent live `result` event will authoritatively replace this.
        if record.state.tokens == 0 {
            let total_tokens: u64 = rows
                .iter()
                .map(|r| r.input_tokens.saturating_add(r.output_tokens))
                .sum();
            if total_tokens > 0 {
                record.state.tokens = total_tokens;
                let window = record
                    .state
                    .model
                    .as_deref()
                    .map(marsad_events::SessionState::context_window_for)
                    .unwrap_or(200_000);
                let pct = (total_tokens as f64 / window as f64 * 100.0) as f32;
                record.state.context_pct = pct.min(100.0);
            }
        }
        // Merge the estimated rows into the in-state model_usage map so
        // make_session_view picks it up. U4: drop only the PREVIOUSLY estimated
        // rows (this probe supersedes a stale one) and never touch — or
        // overwrite with an estimate — a row a live `result` event set.
        record.state.model_usage.retain(|_, mu| !mu.estimated);
        for row in &rows {
            if record.state.model_usage.contains_key(&row.model) {
                // A live row survived the retain above — never overwrite it
                // with an estimate.
                continue;
            }
            record.state.model_usage.insert(
                row.model.clone(),
                marsad_proto::ModelUsage {
                    input_tokens: row.input_tokens,
                    output_tokens: row.output_tokens,
                    cost_usd: row.cost_usd,
                    estimated: true,
                },
            );
        }

        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        Ok(vec![VmUpdate::Session(sv)])
    }

    // -----------------------------------------------------------------------
    // X3 Wasl inbound handlers (docs/specs/wasl-connector.md §5.2)
    // -----------------------------------------------------------------------

    /// Snapshot `wasl_gates` as a deterministically ordered `Vec` (the
    /// `BTreeMap` key is `(host_id, gate_id)`, so iteration order is
    /// lexicographic).
    fn wasl_gates_list(&self) -> Vec<WaslGateView> {
        self.wasl_gates.values().cloned().collect()
    }

    /// A Wasl gate surfaced (or re-surfaced via resnapshot reconcile).
    /// `decidable == false` (observe-only) is a tolerant no-op — those gates
    /// are stream-visible but never pending/answerable. A malformed `request`
    /// Value is also a tolerant skip (observational data must never wedge the
    /// reader loop, same disposition as `ChangesUpdate`).
    fn handle_wasl_gate_requested(
        &mut self,
        host_id: String,
        gate_id: String,
        decidable: bool,
        request: serde_json::Value,
        requested_at: String,
        deadline: String,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        if !decidable {
            return Ok(vec![]);
        }
        let req: marsad_wasl::GateRequest = match serde_json::from_value(request) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "marsad-core: malformed WaslGateRequested.request for {host_id}/{gate_id}: {e}"
                );
                return Ok(vec![]);
            }
        };
        self.wasl_gates.insert(
            (host_id.clone(), gate_id.clone()),
            WaslGateView {
                host_id,
                gate_id,
                kind: req.kind,
                from: req.from,
                summary: req.summary,
                preview: req.preview,
                requested_at,
                deadline,
            },
        );
        Ok(vec![VmUpdate::WaslGates(self.wasl_gates_list())])
    }

    /// A Wasl gate resolved. Removal is unconditional-attempt: if the gate
    /// was tracked, the pending list update is also emitted; an
    /// already-cleared/unknown gate still gets the `WaslGateDecided` event
    /// (silent-consistent with a race the node itself already resolved).
    fn handle_wasl_gate_decided(
        &mut self,
        host_id: String,
        gate_id: String,
        outcome: String,
        decided_by: String,
        reason: Option<String>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let removed = self
            .wasl_gates
            .remove(&(host_id.clone(), gate_id.clone()))
            .is_some();
        let mut updates = vec![VmUpdate::WaslGateDecided {
            host_id,
            gate_id,
            outcome,
            decided_by,
            reason,
        }];
        if removed {
            updates.push(VmUpdate::WaslGates(self.wasl_gates_list()));
        }
        Ok(updates)
    }

    /// One audit-feed entry. A malformed `entry` Value is a tolerant skip
    /// (same disposition as the malformed-request case above).
    fn handle_wasl_observation(
        &mut self,
        host_id: String,
        entry: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let parsed: marsad_wasl::AuditEntry = match serde_json::from_value(entry) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("marsad-core: malformed WaslObservation.entry for {host_id}: {e}");
                return Ok(vec![]);
            }
        };
        Ok(vec![VmUpdate::WaslObservation {
            host_id,
            entry: parsed,
        }])
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn handle_agent_event(
        &mut self,
        session_id: String,
        event: serde_json::Value,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        // An agent_event for a session this core never registered (e.g. one the
        // agent supervises but we have not launched/adopted) is an UnknownSession,
        // not a panic.
        self.require_session(&session_id)?;

        // Evidence of life: a live stream event proves the process is running, so
        // mark it Resumed. This is the robust fallback for the SessionLiveness
        // one-shot (e.g. if core subscribed just after the reader emitted it):
        // any inbound event flips a restored/Paused session to Resumed. The
        // SessionView built below picks this up.
        if let Some(rec) = self.sessions.get_mut(&session_id) {
            rec.live = true;
        }

        // Parse the event into a StreamMsg; treat unparseable events as Unknown
        // (forward-compat, no error).
        let event_json = serde_json::to_string(&event).unwrap(); // infallible
        let stream_msg = marsad_proto::parse_stream_line(&event_json)
            .unwrap_or(marsad_proto::StreamMsg::Unknown(event));

        // Extract assistant prose + tool invocations for the transcript (from the
        // live stream — immediate, no JSONL flush race). claude does not echo user
        // prompts here; those are injected by launch/send_prompt. Deduped below by
        // message id.
        // Pair each entry with a content-based dedup key computed inline (we know
        // each block is Text→Assistant or ToolUse→Tool here), so dedup works per
        // BLOCK rather than per message id — see the append loop below.
        let assistant: Option<(String, Vec<(String, marsad_events::TranscriptEntry)>)> =
            if let marsad_proto::StreamMsg::Assistant(am) = &stream_msg {
                let mut entries = Vec::new();
                for block in &am.message.content {
                    match block {
                        marsad_proto::ContentBlock::Text { text } => {
                            let t = text.trim();
                            if !t.is_empty() {
                                entries.push((
                                    format!("a\u{0}{t}"),
                                    marsad_events::TranscriptEntry::Assistant {
                                        text: t.to_string(),
                                        at: Some(now_ms()),
                                    },
                                ));
                            }
                        }
                        marsad_proto::ContentBlock::ToolUse { name, input, .. } => {
                            let input = serde_json::to_string(input).unwrap_or_default();
                            entries.push((
                                format!("t\u{0}{name}\u{0}{input}"),
                                marsad_events::TranscriptEntry::Tool {
                                    name: name.clone(),
                                    input,
                                    at: Some(now_ms()),
                                },
                            ));
                        }
                        _ => {}
                    }
                }
                Some((am.message.id.clone(), entries))
            } else {
                None
            };

        // Capture compaction metadata BEFORE stream_msg is consumed by reduce, so
        // the live transcript gets an in-sequence Compaction entry (mirrors what
        // parse_transcript does for the backfill/JSONL path).
        let compact_entry: Option<marsad_events::TranscriptEntry> =
            if let marsad_proto::StreamMsg::CompactBoundary(cb) = &stream_msg {
                Some(marsad_events::TranscriptEntry::Compaction {
                    trigger: cb.compact_metadata.trigger.clone(),
                    dropped_tokens: cb.compact_metadata.cumulative_dropped_tokens,
                    at: Some(now_ms()),
                })
            } else {
                None
            };

        // Detect a SystemInit BEFORE consuming the stream_msg — we need to
        // know if slash_commands are about to be set so we can persist them.
        let is_system_init = matches!(stream_msg, marsad_proto::StreamMsg::SystemInit(_));
        // Detect a Result BEFORE consuming stream_msg — after reduce we re-aggregate
        // usage from the full JSONL for LOCAL sessions so the displayed total reflects
        // the whole conversation, not just the current run (which is all result.total_cost_usd
        // contains for a resumed session).
        let is_result = matches!(stream_msg, marsad_proto::StreamMsg::Result(_));

        // Reduce the event, then decide whether the working tree may have changed.
        // Safe: require_session above guarantees the entry exists.
        let (project, host_id, should_probe, board_changed) = {
            let record = self.sessions.get_mut(&session_id).unwrap();
            let old_state = record.state.clone();
            let old_last_action = old_state.last_action.clone();
            let was_finished = matches!(old_state.phase, Phase::Done | Phase::Error);
            let old_phase = old_state.phase.clone();
            let old_gate_present = old_state.gate.is_some();
            record.state = reduce(old_state, Event::Stream(stream_msg));
            // Re-probe git when a tool just ran (files may have changed) or the
            // session just reached a terminal phase — not on every token.
            let tool_ran = record.state.last_action != old_last_action;
            let just_finished =
                matches!(record.state.phase, Phase::Done | Phase::Error) && !was_finished;
            // The workstream board's attention card derives from phase + pending
            // gate; refresh it only when one of those actually changed (a gate
            // appeared/cleared or the phase moved) — never on every token.
            let board_changed =
                record.state.phase != old_phase || record.state.gate.is_some() != old_gate_present;
            (
                record.project.clone(),
                record.host_id.clone(),
                tool_ran || just_finished,
                board_changed,
            )
        };

        // Persist the claude session UUID, slash_commands, tools, and mcp_servers
        // after a SystemInit so that on the next attach/reconnect (when send_init
        // is false) the restored session shows them without waiting for another
        // system/init.
        //
        // The UUID is the critical one: a Fresh launch persists session_id=None
        // (the real claude conversation id is unknown until system/init arrives),
        // and that id lives ONLY in memory (`state.session_id`) unless we write it
        // back here. Without this, on the next app restart two sessions in the SAME
        // directory both load session_id=None and fall back to "most recent
        // .jsonl" (remote_transcript_script `ls -t | head -1`) → they COLLAPSE onto
        // the same claude session. Persisting the id makes each session resume /
        // backfill its OWN conversation. Guarded to the claude driver: codex's
        // synthesized system/init carries the codex thread_id in `session_id`,
        // which is NOT a `claude --resume` target and must not be persisted there.
        // Best-effort: a store error here must not fail the event processing.
        if is_system_init {
            let (claude_sid, slash_cmds, tools, mcp_servers) = self
                .sessions
                .get(&session_id)
                .map(|r| {
                    (
                        r.state.session_id.clone(),
                        r.state.slash_commands.clone(),
                        r.state.tools.clone(),
                        r.state.mcp_servers.clone(),
                    )
                })
                .unwrap_or_default();
            let has_sid = claude_sid
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if has_sid || !slash_cmds.is_empty() || !tools.is_empty() || !mcp_servers.is_empty() {
                // Load the persisted session, update fields, write back.
                if let Ok(restored) = self.store.restore() {
                    if let Some(mut ps) = restored.sessions.into_iter().find(|s| s.id == session_id)
                    {
                        if has_sid
                            && ps.driver == "claude"
                            && ps.session_id.as_deref() != claude_sid.as_deref()
                        {
                            ps.session_id = claude_sid;
                        }
                        ps.slash_commands = slash_cmds;
                        ps.tools = tools;
                        ps.mcp_servers = mcp_servers;
                        let _ = self.store.upsert_session(&ps);
                    }
                }
            }
        }

        // After a Result event, re-aggregate usage from the full session JSONL for
        // LOCAL sessions. reduce_result sets cost/tokens from result.total_cost_usd
        // which covers only the CURRENT run, not prior resumed runs — re-aggregating
        // here ensures the displayed total reflects the whole conversation history.
        // Remote sessions are left untouched (they seed via UsageUpdate IPC).
        if is_result && self.host_is_local(&host_id) {
            let record = self.sessions.get_mut(&session_id).unwrap();
            if let Some(sid) = record.state.session_id.as_deref().filter(|s| !s.is_empty()) {
                let usage_agg = self.transcript.session_usage(&project, sid);
                if !usage_agg.is_empty() {
                    let total_tokens: u64 = usage_agg
                        .values()
                        .map(|u| {
                            u.input_tokens
                                .saturating_add(u.output_tokens)
                                .saturating_add(u.cache_read_input_tokens)
                                .saturating_add(u.cache_creation_input_tokens)
                        })
                        .sum();
                    if total_tokens > 0 {
                        record.state.tokens = total_tokens;
                        let window = record
                            .state
                            .model
                            .as_deref()
                            .map(marsad_events::SessionState::context_window_for)
                            .unwrap_or(200_000);
                        let pct = (total_tokens as f64 / window as f64 * 100.0) as f32;
                        record.state.context_pct = pct.min(100.0);
                        record.state.cost_usd = usage_agg.values().map(|u| u.cost_usd).sum();
                        record.state.model_usage.clear();
                        for (model, summary) in usage_agg {
                            record.state.model_usage.insert(
                                model,
                                marsad_proto::ModelUsage {
                                    input_tokens: summary.input_tokens,
                                    output_tokens: summary.output_tokens,
                                    cost_usd: summary.cost_usd,
                                    estimated: true,
                                },
                            );
                        }
                    }
                }
            }
        }

        // Only probe the LOCAL working tree for LOCAL sessions. REMOTE sessions
        // get their changes from the agent's `ChangesUpdate` IPC — a local `git
        // status` on a remote project path returns nothing and would CLOBBER the
        // real (remote) changes on the next tool_use/result event.
        if should_probe && self.host_is_local(&host_id) {
            let changes = self.git.changes(&project);
            // Safe: entry still exists (single-threaded, no removal in between).
            self.sessions.get_mut(&session_id).unwrap().changes = changes;
        }

        // Append this assistant message's NEW content blocks. claude streams one
        // assistant message as multiple events sharing a message id (one per
        // block), so we dedup per `<msg_id>\0<block>` — deduping by id alone
        // dropped every block after the first, freezing the live transcript.
        if let Some((msg_id, entries)) = assistant {
            let record = self.sessions.get_mut(&session_id).unwrap();
            for (block_key, entry) in entries {
                if record
                    .seen_msg_ids
                    .insert(format!("{msg_id}\u{0}{block_key}"))
                {
                    record.transcript.push(entry);
                }
            }
        }

        // Append a Compaction entry in sequence for a live compact_boundary event
        // so the transcript reflects the chronological position (mirrors what
        // parse_transcript does for the JSONL backfill path).
        if let Some(entry) = compact_entry {
            self.sessions
                .get_mut(&session_id)
                .unwrap()
                .transcript
                .push(entry);
        }

        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        let mut updates = vec![VmUpdate::Session(sv), VmUpdate::Overview(groups)];
        // Refresh the workstream board's attention card when this event changed a
        // board-relevant field (gate appeared/cleared, phase moved) so a needs-you
        // card shows the pending gate live instead of staying stale from startup.
        if board_changed {
            updates.push(VmUpdate::Board(self.board()));
        }
        Ok(updates)
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

/// X6 (§8, v3): build the `marsad_gates::Gate::Permission` a native Wasl gate
/// maps onto. The real `WaslNativeGateDetail` is only `{summary, preview}` (no
/// `gate_id`); per-occurrence identity is the `(turn_id, seq)`-derived `gate_key`
/// (`native_gate_key`), passed in by the caller. The `command` encodes that
/// `gate_key` via `native_gate_command(gate_key, detail)` so the derived
/// `DecisionId` (which hashes `command` + `cwd`) is unique+stable per gate
/// occurrence; `explanation` carries the CLI's preview; the claude-only fields
/// are empty (a native permission prompt is a binary approve/deny with no
/// cwd/risk/elicitation/suggestions).
fn native_gate_permission(
    gate_key: &str,
    detail: &marsad_wasl_session::WaslNativeGateDetail,
) -> marsad_gates::Gate {
    marsad_gates::Gate::Permission {
        request_id: gate_key.to_string(),
        command: marsad_wasl_session::native_gate_command(gate_key, detail),
        cwd: String::new(),
        risk: String::new(),
        explanation: detail.preview.clone(),
        is_elicitation: false,
        elicitation_choices: Vec::new(),
        permission_suggestions: Vec::new(),
    }
}

/// X6 (§4.4, v3): peel every `WaslTurnReceipt::Duplicate { original }` layer and
/// return the underlying original receipt. A `Duplicate` is a benign resent
/// idempotency key — Wasl returns the ORIGINAL wrapped verbatim with no second
/// injection — so its disposition is exactly the original's. Any non-`Duplicate`
/// receipt is returned as-is.
fn unwrap_duplicate_receipt(
    receipt: &marsad_wasl_session::WaslTurnReceipt,
) -> &marsad_wasl_session::WaslTurnReceipt {
    let mut cur = receipt;
    while let marsad_wasl_session::WaslTurnReceipt::Duplicate { original } = cur {
        cur = original;
    }
    cur
}

/// X6 (§10.4): close out a Wasl turn's transcript entry exactly once. If a delta
/// already opened an assistant entry, leave it as-is (no double-close/reopen); if
/// none opened (the reply arrived only in the terminal event), append it now.
/// Marks the post-terminal guard so a stray later delta is ignored.
fn close_wasl_turn(record: &mut SessionRecord, reply: &str) {
    if record.wasl_open_assistant.is_none() && !reply.is_empty() {
        record
            .transcript
            .push(marsad_events::TranscriptEntry::Assistant {
                text: reply.to_string(),
                at: Some(now_ms()),
            });
    }
    record.wasl_open_assistant = None;
    record.wasl_turn_closed = true;
}

/// Map the per-session connection health enum to the string the SessionView
/// carries to the UI ("Connected"/"Reconnecting"/"Unstable").
fn conn_health_str(h: marsad_proto::ConnHealth) -> String {
    match h {
        marsad_proto::ConnHealth::Connected => "Connected",
        marsad_proto::ConnHealth::Reconnecting => "Reconnecting",
        marsad_proto::ConnHealth::Unstable => "Unstable",
        marsad_proto::ConnHealth::Disconnected => "Disconnected",
    }
    .to_string()
}

/// Return a simple timestamp string for workspace `saved_at` fields.
///
/// Uses seconds-since-epoch; good enough for display and round-trips.
/// Epoch milliseconds now — the timestamp stamped on LIVE transcript entries as
/// they arrive (resumed/backfilled entries instead carry the JSONL's own time).
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Format as ISO-8601-ish: "2026-06-24T12:00:00Z" approximation.
    // We produce a simple "just now" placeholder for display; the real timestamp
    // is the epoch seconds encoded in the id for uniqueness.
    format!("{secs}")
}

fn new_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Include the launch time so ids are UNIQUE ACROSS RESTARTS. A pure counter
    // reset to 1 every process start, so each restart minted `session-…0001`
    // again — and the durable remote dir `~/.marsad/<id>` then collided with a
    // prior (possibly removed) session's, making old sessions appear to "come
    // back". The seconds prefix makes every launch's ids distinct.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("session-{secs:x}-{n:x}")
}

/// Human-friendly session name: the project directory's last path component
/// (e.g. "/work/mewais/Marsad" -> "Marsad"). Falls back to the session id when
/// the project is empty or has no usable basename.
fn session_display_name(project: &str, id: &str) -> String {
    match project.trim_end_matches('/').rsplit('/').next() {
        Some(base) if !base.is_empty() => base.to_string(),
        _ => id.to_string(),
    }
}

/// Concise one-line summary of a pending gate, for the workstream board's
/// attention card (so a needs-you card shows WHAT is asked, not just the name).
fn gate_headline(gate: &marsad_gates::Gate) -> String {
    use marsad_gates::Gate;
    match gate {
        Gate::Permission { command, .. } => format!("Wants to run: {command}"),
        Gate::Plan { .. } => "Plan ready for your approval".to_string(),
        Gate::Edit { file, .. } => format!("Wants to edit {file}"),
        Gate::Question { questions, .. } => questions
            .first()
            .map(|q| q.question.clone())
            .unwrap_or_else(|| "Wants your input".to_string()),
        Gate::Handoff { summary, .. } => summary.clone(),
    }
}

/// Map a `permissionMode` string from the claude `system/init` event to the
/// canonical posture id used in `SessionView` and the Config tab.
///
/// - `"default"` → `"ask"` (Ask every time)
/// - `"acceptEdits"` → `"auto"` (Auto-approve safe actions)
/// - `"bypassPermissions"` → `"full"` (Full autonomy)
/// - unknown / `None` → `"ask"` (safe fallback)
fn permission_mode_to_posture(mode: Option<&str>) -> String {
    match mode {
        Some("acceptEdits") => "auto".to_string(),
        Some("bypassPermissions") => "full".to_string(),
        _ => "ask".to_string(),
    }
}

/// Map a persisted proto-enum name (the `Posture` variant name stored in
/// `PersistedSession.posture`) back to the `permissionMode` string that
/// `SessionState` / `make_session_view` understand.
///
/// Values: `"Ask"` → `Some("default")`, `"AutoApproveSafe"` →
/// `Some("acceptEdits")`, `"FullAutonomy"` → `Some("bypassPermissions")`,
/// anything else / unknown → `None` (caller falls back to the default "ask").
fn posture_name_to_permission_mode(name: &str) -> Option<&'static str> {
    match name {
        "Ask" => Some("default"),
        "AutoApproveSafe" => Some("acceptEdits"),
        "FullAutonomy" => Some("bypassPermissions"),
        _ => None,
    }
}

fn posture_to_permission_mode(posture: &Posture) -> &'static str {
    match posture {
        Posture::Ask => "default",
        Posture::AutoApproveSafe => "autoApprove",
        Posture::FullAutonomy => "bypassPermissions",
    }
}

fn slash_cmd_name(cmd: &marsad_viewmodel::SlashCmd) -> &str {
    use marsad_viewmodel::SlashCmd;
    match cmd {
        SlashCmd::Clear => "clear",
        SlashCmd::Compact => "compact",
        SlashCmd::Cost => "cost",
        SlashCmd::Model => "model",
        SlashCmd::Config => "config",
        SlashCmd::Init => "init",
        SlashCmd::Review => "review",
        SlashCmd::Agents => "agents",
        SlashCmd::Resume => "resume",
        SlashCmd::Terminal => "terminal",
        SlashCmd::Help => "help",
        SlashCmd::Unknown(s) => s.as_str(),
    }
}

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
