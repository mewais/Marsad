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
mod helpers;
mod sessions;
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
                let _ = self
                    .hosts
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
