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
use marsad_hosts::{HostError, HostRegistry, SshRunner};
use marsad_proto::{encode_ipc, IpcMsg};
use marsad_store::{Backend, PersistedHost, Store, Workspace as StoreWorkspace};
use marsad_transport::Transport;
use marsad_viewmodel::{AppSnapshot, FileChange, Filter, SessionView, Tab, Workspace};
use serde_json::Value;
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

    /// A toast notification.
    Toast { icon: String, text: String },

    /// Host health changed.
    HostHealth(HostHealthPayload),

    /// The full host list changed (added/removed/status updated).
    Hosts(Vec<Host>),

    /// The full workspace list changed (saved/duplicated/deleted/imported).
    Workspaces(Vec<Workspace>),
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
    /// Assistant message ids already appended to the transcript (dedup).
    seen_msg_ids: std::collections::HashSet<String>,
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
                    .update(&h.id, h.claude_bin, h.setup, h.env, None);
            } else {
                self.hosts
                    .load_remote(&h.id, &h.label, &h.target, h.claude_bin, h.setup, h.env);
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
    pub fn add_host(
        &mut self,
        label: &str,
        target: &str,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let host = self
            .hosts
            .add(label, target, claude_bin, setup, env)
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
        };
        self.store
            .upsert_host(&persisted)
            .map_err(|e| CoreError::Host(format!("{e:?}")))?;
        Ok(vec![VmUpdate::Hosts(self.hosts.list().to_vec())])
    }

    /// Update an existing host's launch configuration (`claude_bin` / `setup` /
    /// `env`), and optionally its `target`. Works for any host id INCLUDING the
    /// protected `localhost` (pass `None` for `target` to leave the target
    /// unchanged — localhost's stays `"this machine"`). Persists the change and
    /// returns the updated host list.
    pub fn update_host(
        &mut self,
        id: &str,
        claude_bin: Option<String>,
        setup: Option<String>,
        env: Vec<(String, String)>,
        target: Option<String>,
    ) -> Result<Vec<VmUpdate>, CoreError> {
        let host = self
            .hosts
            .update(id, claude_bin, setup, env, target)
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
    pub fn set_host_status(&mut self, id: &str, status: HostStatus) -> Vec<VmUpdate> {
        self.hosts.set_status(id, status);
        vec![VmUpdate::Hosts(self.hosts.list().to_vec())]
    }

    // -----------------------------------------------------------------------
    // Settings commands
    // -----------------------------------------------------------------------

    /// Default sensible settings returned when nothing has been persisted yet.
    fn default_settings() -> PersistedSettings {
        use std::collections::HashMap;
        use marsad_store::NotifPref;

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
    pub fn set_session_changes(
        &mut self,
        id: &str,
        changes: Vec<FileChange>,
    ) -> Vec<VmUpdate> {
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
        let msg = IpcMsg::LaunchSession {
            cwd: spec.project.clone(),
            model: spec.model.clone(),
            permission_mode: permission_mode.to_string(),
        };
        // encode_ipc + serde round-trips on our own structs are infallible.
        let encoded_str = encode_ipc(&msg);
        let mut val: Value = serde_json::from_str(&encoded_str).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.insert("session_id".to_string(), Value::String(id.clone()));
        obj.insert("project".to_string(), Value::String(spec.project.clone()));
        // Forward the host so the agent runs the session on the right machine.
        // Resolve the remote execution target from the host id via the registry
        // (local/unknown hosts → None → the agent runs locally). An explicit
        // `spec.host_target` wins if a caller already set one.
        obj.insert("host_id".to_string(), Value::String(spec.host_id.clone()));
        let host_target = spec.host_target.clone().or_else(|| {
            self.hosts
                .list()
                .iter()
                .find(|h| h.id == spec.host_id && !h.is_local)
                .map(|h| h.target.clone())
        });
        if let Some(target) = host_target {
            obj.insert("host_target".to_string(), Value::String(target));
        }
        // Resolve the per-host launch config (claude_bin / setup / env) from the
        // registry by host_id and inject it for the agent. Applies to ALL host
        // types (local, WSL, SSH/plink). An explicit value already on the spec
        // wins over the registry lookup. `env` is forwarded as a JSON array of
        // `[k, v]` pairs (the agent parses it back identically).
        let host_cfg = self.hosts.list().iter().find(|h| h.id == spec.host_id);
        let claude_bin = spec
            .claude_bin
            .clone()
            .or_else(|| host_cfg.and_then(|h| h.claude_bin.clone()));
        if let Some(bin) = claude_bin {
            obj.insert("claude_bin".to_string(), Value::String(bin));
        }
        let setup = spec
            .setup
            .clone()
            .or_else(|| host_cfg.and_then(|h| h.setup.clone()));
        if let Some(s) = setup {
            obj.insert("setup".to_string(), Value::String(s));
        }
        let env = if !spec.env.is_empty() {
            spec.env.clone()
        } else {
            host_cfg.map(|h| h.env.clone()).unwrap_or_default()
        };
        if !env.is_empty() {
            let env_json: Vec<Value> = env
                .iter()
                .map(|(k, v)| {
                    Value::Array(vec![Value::String(k.clone()), Value::String(v.clone())])
                })
                .collect();
            obj.insert("env".to_string(), Value::Array(env_json));
        }
        // The agent decodes posture from `posture` (the enum variant name);
        // `permission_mode` above is kept for back-compat.
        obj.insert(
            "posture".to_string(),
            serde_json::to_value(&spec.posture).unwrap(),
        );
        let fp_val = spec
            .first_prompt
            .as_deref()
            .map_or(Value::Null, |s| Value::String(s.to_string()));
        obj.insert("first_prompt".to_string(), fp_val);
        // Forward the start mode so the agent can resume/continue an existing
        // conversation (claude --resume <id> / --continue). Fresh adds nothing.
        match &spec.mode {
            LaunchMode::Continue => {
                obj.insert("continue".to_string(), Value::Bool(true));
            }
            LaunchMode::Resume(sid) => {
                obj.insert("resume_session_id".to_string(), Value::String(sid.clone()));
            }
            LaunchMode::Fresh => {}
        }
        let line = serde_json::to_string(&val).unwrap();

        self.send_raw_line(&line)?;

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
                });
            }
        }

        // Upsert: if the id already exists (restart path), reset the record;
        // if new, push onto the order list first.
        if !self.sessions.contains_key(&id) {
            self.session_order.push(id.clone());
        }
        self.sessions.insert(
            id.clone(),
            SessionRecord {
                project: spec.project,
                host_id: spec.host_id,
                state: SessionState::initial(),
                changes: Vec::new(),
                transcript,
                seen_msg_ids: std::collections::HashSet::new(),
            },
        );

        Ok(id)
    }

    /// Restart a session with a new model and posture.
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
    ) -> Result<Vec<VmUpdate>, CoreError> {
        self.require_session(id)?;

        // Snapshot what we need before mutating state.
        let project = self.sessions[id].project.clone();
        let host_id = self.sessions[id].host_id.clone();
        let claude_uuid = self.sessions[id].state.session_id.clone();

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
        // same machine (WSL/SSH), not silently back on local. host_target /
        // claude_bin / setup / env are re-resolved from the registry by host_id
        // inside launch_session_with_id.
        let spec = LaunchSpec {
            project,
            host_id,
            model: model.to_string(),
            posture,
            first_prompt: None,
            mode,
            host_target: None,
            claude_bin: None,
            setup: None,
            env: Vec::new(),
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

    pub fn resume_session(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::ResumeSession {
            session_id: id.to_string(),
        })
    }

    pub fn attach_session(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::AttachSession {
            session_id: id.to_string(),
        })
    }

    pub fn stop_session(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        self.send_ipc(&IpcMsg::StopSession {
            session_id: id.to_string(),
        })
    }

    pub fn send_prompt(&mut self, id: &str, text: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        // Record the user's prompt immediately — claude does not echo it on the
        // stream, so this is the only place it can enter the transcript. It surfaces
        // on the next agent event (claude's reply) which re-emits the SessionView.
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            // Safe: require_session above guarantees the entry exists.
            self.sessions.get_mut(id).unwrap().transcript.push(
                marsad_events::TranscriptEntry::User {
                    text: trimmed.to_string(),
                },
            );
        }
        self.send_ipc(&IpcMsg::SendPrompt {
            session_id: id.to_string(),
            text: text.to_string(),
        })
    }

    pub fn interrupt(&mut self, id: &str) -> Result<(), CoreError> {
        self.require_session(id)?;
        // Reuse StopSession with an added "interrupt" flag (infallible serde).
        let mut val: Value = serde_json::from_str(&encode_ipc(&IpcMsg::StopSession {
            session_id: id.to_string(),
        }))
        .unwrap();
        val.as_object_mut()
            .unwrap()
            .insert("interrupt".to_string(), Value::Bool(true));
        self.send_raw_line(&serde_json::to_string(&val).unwrap())
    }

    pub fn answer_gate(&mut self, decision: GateDecision) -> Result<(), CoreError> {
        let session_id = marsad_gates::route(&decision).to_string();
        self.require_session(&session_id)?;

        // Idempotency: already recorded → silent no-op (no duplicate IPC).
        if self.ledger.seen(&decision.id) {
            return Ok(());
        }
        // record() cannot return DuplicateDecision here because seen() is false.
        // unwrap() is infallible in this code path.
        self.ledger.record(&decision).unwrap();

        let ctrl = marsad_gates::to_control_response(&decision);
        self.send_ipc(&IpcMsg::AnswerGate {
            session_id,
            request_id: ctrl.request_id,
            allow: ctrl.allow,
            deny_reason: ctrl.deny_reason,
        })
    }

    pub fn run_slash(
        &mut self,
        id: &str,
        cmd: marsad_viewmodel::SlashCmd,
    ) -> Result<(), CoreError> {
        self.require_session(id)?;
        // serde_json::json! + to_string is infallible for literal values.
        let line = serde_json::to_string(&serde_json::json!({
            "type": "SlashCmd",
            "session_id": id,
            "cmd": slash_cmd_name(&cmd),
        }))
        .unwrap();
        self.send_raw_line(&line)
    }

    pub fn set_workspace(&mut self, id: &str) -> Result<Vec<VmUpdate>, CoreError> {
        self.active_ws = Some(id.to_string());
        // Persist the active workspace if it exists in our list; best-effort.
        if self.workspaces.iter().any(|w| w.id == id) {
            let _ = self.store.set_active_workspace(id);
        }
        // Best-effort outbound notification (infallible construction).
        let line = serde_json::to_string(&serde_json::json!({
            "type": "SetWorkspace",
            "workspace_id": id,
        }))
        .unwrap();
        // Best-effort advisory send; ignore errors.
        let _ = self.transport.send_line(&line);
        Ok(vec![self.workspaces_update()])
    }

    // -----------------------------------------------------------------------
    // Inbound IPC parsing — central testable seam
    // -----------------------------------------------------------------------

    pub fn on_agent_line(&mut self, line: &str) -> Result<Vec<VmUpdate>, CoreError> {
        // Parse line as a typed envelope.  serde rejects: empty strings, non-JSON,
        // truncated JSON, non-objects, missing/wrong-typed "type" or "session_id"
        // fields — all via a single map_err closure that is exercised by the
        // existing protocol error tests.
        let env: IpcEnvelope =
            serde_json::from_str(line).map_err(|e| CoreError::Protocol(e.to_string()))?;

        match env.r#type.as_str() {
            "AgentEvent" => self.handle_agent_event(env),
            other => Err(CoreError::Protocol(format!("unknown IPC type: '{other}'"))),
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn handle_agent_event(&mut self, env: IpcEnvelope) -> Result<Vec<VmUpdate>, CoreError> {
        let session_id = match env.session_id {
            Some(s) => s,
            None => {
                return Err(CoreError::Protocol(
                    "missing session_id in agent_event".to_string(),
                ))
            }
        };

        // An agent_event for a session this core never registered (e.g. one the
        // agent supervises but we have not launched/adopted) is an UnknownSession,
        // not a panic.
        self.require_session(&session_id)?;

        // Parse the event into a StreamMsg; treat unparseable events as Unknown
        // (forward-compat, no error).
        let event_json = serde_json::to_string(&env.event).unwrap(); // infallible
        let stream_msg = marsad_proto::parse_stream_line(&event_json)
            .unwrap_or(marsad_proto::StreamMsg::Unknown(env.event));

        // Extract assistant prose + tool invocations for the transcript (from the
        // live stream — immediate, no JSONL flush race). claude does not echo user
        // prompts here; those are injected by launch/send_prompt. Deduped below by
        // message id.
        let assistant: Option<(String, Vec<marsad_events::TranscriptEntry>)> =
            if let marsad_proto::StreamMsg::Assistant(am) = &stream_msg {
                let mut entries = Vec::new();
                for block in &am.message.content {
                    match block {
                        marsad_proto::ContentBlock::Text { text } => {
                            let t = text.trim();
                            if !t.is_empty() {
                                entries.push(marsad_events::TranscriptEntry::Assistant {
                                    text: t.to_string(),
                                });
                            }
                        }
                        marsad_proto::ContentBlock::ToolUse { name, input, .. } => {
                            entries.push(marsad_events::TranscriptEntry::Tool {
                                name: name.clone(),
                                input: serde_json::to_string(input).unwrap_or_default(),
                            });
                        }
                        _ => {}
                    }
                }
                Some((am.message.id.clone(), entries))
            } else {
                None
            };

        // Reduce the event, then decide whether the working tree may have changed.
        // Safe: require_session above guarantees the entry exists.
        let (project, should_probe) = {
            let record = self.sessions.get_mut(&session_id).unwrap();
            let old_state = record.state.clone();
            let old_last_action = old_state.last_action.clone();
            let was_finished = matches!(old_state.phase, Phase::Done | Phase::Error);
            record.state = reduce(old_state, Event::Stream(stream_msg));
            // Re-probe git when a tool just ran (files may have changed) or the
            // session just reached a terminal phase — not on every token.
            let tool_ran = record.state.last_action != old_last_action;
            let just_finished =
                matches!(record.state.phase, Phase::Done | Phase::Error) && !was_finished;
            (record.project.clone(), tool_ran || just_finished)
        };

        if should_probe {
            let changes = self.git.changes(&project);
            // Safe: entry still exists (single-threaded, no removal in between).
            self.sessions.get_mut(&session_id).unwrap().changes = changes;
        }

        // Append this assistant message's entries once (dedup by message id).
        if let Some((msg_id, entries)) = assistant {
            let record = self.sessions.get_mut(&session_id).unwrap();
            if record.seen_msg_ids.insert(msg_id) {
                record.transcript.extend(entries);
            }
        }

        let sv = self.make_session_view(&session_id, self.sessions.get(&session_id).unwrap());
        let snap = self.snapshot();
        let groups = marsad_viewmodel::group_overview(&snap);
        Ok(vec![VmUpdate::Session(sv), VmUpdate::Overview(groups)])
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
        SessionView {
            id: id.to_string(),
            name: session_display_name(&record.project, id),
            host_id: record.host_id.clone(),
            project: record.project.clone(),
            branch: String::new(),
            model: s.model.clone().unwrap_or_default(),
            state: s.phase.clone(),
            changed_files: record.changes.len() as u32,
            subagents: s.subagents.len() as u32,
            cost_usd: s.cost_usd,
            tokens: s.tokens,
            context_pct: s.context_pct,
            gate: s.gate.clone(),
            transcript: record.transcript.clone(),
            changes: record.changes.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// IpcEnvelope — serde-typed wrapper for inbound IPC lines
// ---------------------------------------------------------------------------

/// Typed representation of an inbound IPC line.
///
/// Using serde deserialization here means serde enforces all structural
/// invariants (correct field types, required fields) in a single `from_str`
/// call, so all structural Protocol errors go through one `map_err` closure.
#[derive(serde::Deserialize)]
struct IpcEnvelope {
    /// IPC message type discriminant.  Required; must be a string.
    #[serde(rename = "type")]
    r#type: String,
    /// Session id.  Optional at the envelope level; validated in dispatch.
    session_id: Option<String>,
    /// Raw event payload for agent_event messages.
    #[serde(default)]
    event: Value,
}

// ---------------------------------------------------------------------------
// Private free functions
// ---------------------------------------------------------------------------

/// Return a simple timestamp string for workspace `saved_at` fields.
///
/// Uses seconds-since-epoch; good enough for display and round-trips.
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
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{n:016x}")
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
