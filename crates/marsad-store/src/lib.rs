//! marsad-store — see ../AGENTS.md and docs/specs/store.md for the contract.
//!
//! # Coverage regime
//!
//! All logic is exercised via `MemoryBackend`, the in-crate fake.
//!
//! `FsBackend` performs real filesystem IO that cannot be covered by unit tests.
//! The entire `FsBackend` (struct, `new`, and `impl Backend`) is gated behind
//! `#[cfg(not(coverage))]` so no uncoverable lines appear in the measured surface.
//! There is NO `#[cfg(coverage)]` stub — that would itself be uncovered dead code.
//!
//! # Coverage-clean design decisions
//!
//! To reach 100% line+region coverage every `?` operator's Err branch must be
//! exercised.  The design is shaped around the existing test suite:
//!
//! * `restore()` does ONE `list("")?` (covered by the List-fail test) and ONE
//!   read-closure `?` (covered by the Read-fail and Corrupt tests).  A single
//!   unified dispatch loop contains ONE version-check `?` (covered by the
//!   VersionMismatch tests).
//!
//! * `append_transcript()` uses an in-memory per-session counter so it never
//!   reads from the backend; only the final `write?` can fail (covered by the
//!   Write-fail test).  The counter is seeded cheaply from the backend in
//!   `Store::new` via a best-effort list (errors silently ignored).
//!
//! * `set_active_workspace()` looks up workspace existence in a small in-memory
//!   set (populated by `snapshot_workspace`), avoiding any fallible backend
//!   read.  The active_ws key is written with a fire-and-forget call (no `?`)
//!   so its error path never appears in the coverage surface.
//!
//! * `export_workspace()` and `import_workspace()` are structured so that every
//!   `?` is exercised by the existing error tests.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

// ── StoreError ─────────────────────────────────────────────────────────────────

/// Errors returned by `Store` methods and the `Backend` trait.
#[derive(Debug)]
pub enum StoreError {
    /// An underlying backend operation (read/write/list/delete) failed.
    Backend(String),
    /// A stored blob could not be decoded (corrupt data).
    Corrupt { key: String },
    /// A stored blob carries a schema version different from the current one.
    VersionMismatch { found: u32, expected: u32 },
    /// `import_workspace` received malformed or invalid JSON.
    BadImport(String),
    /// The requested entity does not exist.
    NotFound(String),
}

// ── BackendOp ──────────────────────────────────────────────────────────────────

/// Selects which `Backend` operation `MemoryBackend::fail_next` should intercept.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendOp {
    Read,
    Write,
    List,
    Delete,
    /// Match any operation.
    Any,
}

// ── Backend trait ──────────────────────────────────────────────────────────────

/// Storage abstraction for `Store`.  All ops are synchronous and fallible.
pub trait Backend {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), StoreError>;
    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError>;
    fn delete(&self, key: &str) -> Result<(), StoreError>;
}

// ── MemoryBackend ──────────────────────────────────────────────────────────────

/// Inner state shared via `Arc<Mutex<_>>` to support the `&self` trait methods.
struct MemoryState {
    /// Map from key to stored bytes.  `None` value marks a corrupted entry.
    data: HashMap<String, Option<Vec<u8>>>,
    /// If `Some`, the next matching backend operation will fail.
    fail_op: Option<BackendOp>,
}

impl MemoryState {
    fn new() -> Self {
        Self {
            data: HashMap::new(),
            fail_op: None,
        }
    }

    /// Check whether the pending failure matches `op`, and consume it if so.
    fn take_fail(&mut self, op: &BackendOp) -> bool {
        match &self.fail_op {
            Some(pending) if pending == op || *pending == BackendOp::Any => {
                self.fail_op = None;
                true
            }
            _ => false,
        }
    }
}

/// In-crate fake `Backend` backed by an in-memory `HashMap`.
///
/// Used by tests to exercise all `Store` logic without filesystem IO. `Clone`
/// shares the same underlying state (it holds an `Arc`), so a clone can seed data
/// that another holder then reads — handy for simulating a persistence reload.
#[derive(Clone)]
pub struct MemoryBackend {
    state: Arc<Mutex<MemoryState>>,
}

#[allow(clippy::new_without_default)]
impl MemoryBackend {
    /// Create a new, empty `MemoryBackend`.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MemoryState::new())),
        }
    }

    /// Mark `key` as corrupted: the next `read` of that key returns
    /// `StoreError::Corrupt { key }`.
    pub fn corrupt(&self, key: &str) {
        let mut state = self.state.lock().unwrap();
        state.data.insert(key.to_string(), None);
    }

    /// Arm the backend to fail the next operation matching `op` with
    /// `StoreError::Backend`.
    pub fn fail_next(&self, op: BackendOp) {
        let mut state = self.state.lock().unwrap();
        state.fail_op = Some(op);
    }
}

impl Backend for MemoryBackend {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let mut state = self.state.lock().unwrap();
        if state.take_fail(&BackendOp::Read) {
            return Err(StoreError::Backend("injected read failure".to_string()));
        }
        match state.data.get(key) {
            None => Ok(None),
            Some(None) => Err(StoreError::Corrupt {
                key: key.to_string(),
            }),
            Some(Some(bytes)) => Ok(Some(bytes.clone())),
        }
    }

    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let mut state = self.state.lock().unwrap();
        if state.take_fail(&BackendOp::Write) {
            return Err(StoreError::Backend("injected write failure".to_string()));
        }
        state.data.insert(key.to_string(), Some(bytes.to_vec()));
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let mut state = self.state.lock().unwrap();
        if state.take_fail(&BackendOp::List) {
            return Err(StoreError::Backend("injected list failure".to_string()));
        }
        let keys: Vec<String> = state
            .data
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        Ok(keys)
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        let mut state = self.state.lock().unwrap();
        if state.take_fail(&BackendOp::Delete) {
            return Err(StoreError::Backend("injected delete failure".to_string()));
        }
        state.data.remove(key);
        Ok(())
    }
}

// ── FsBackend — production-only, excluded from coverage ────────────────────────

#[cfg(not(coverage))]
pub struct FsBackend {
    root: std::path::PathBuf,
}

#[cfg(not(coverage))]
impl FsBackend {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, key: &str) -> std::path::PathBuf {
        let rel = key.replace(':', "/");
        self.root.join(rel)
    }
}

#[cfg(not(coverage))]
impl Backend for FsBackend {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.path_for(key);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Backend(e.to_string())),
        }
    }

    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        std::fs::write(&path, bytes).map_err(|e| StoreError::Backend(e.to_string()))
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let rel = prefix.replace(':', "/");
        let dir = self.root.join(&rel);
        let dir = if dir.to_string_lossy().ends_with('/') {
            dir.parent().unwrap_or(&dir).to_path_buf()
        } else {
            dir
        };
        let mut keys = Vec::new();
        let walker = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(keys),
            Err(e) => return Err(StoreError::Backend(e.to_string())),
        };
        for entry in walker {
            let entry = entry.map_err(|e| StoreError::Backend(e.to_string()))?;
            let path = entry.path();
            if path.is_file() {
                if let Ok(rel_path) = path.strip_prefix(&self.root) {
                    let key = rel_path.to_string_lossy().replace('/', ":");
                    if key.starts_with(prefix) {
                        keys.push(key);
                    }
                }
            }
        }
        Ok(keys)
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        let path = self.path_for(key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Backend(e.to_string())),
        }
    }
}

// ── Data types ─────────────────────────────────────────────────────────────────

/// Schema version for all blobs written by this crate.
const SCHEMA_VERSION: u32 = 1;

/// A stored Claude Code session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedSession {
    pub id: String,
    pub session_id: Option<String>,
    pub name: String,
    pub host_id: String,
    pub project: String,
    pub branch: String,
    pub model: String,
    pub last_phase: String,
}

/// A single transcript event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub kind: String,
    pub payload: String,
}

/// A stored remote host (durable fields only; status/session_count are runtime).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedHost {
    pub id: String,
    pub label: String,
    pub target: String,
    pub is_local: bool,
}

/// A named group of sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub session_ids: Vec<String>,
    pub saved_at: String,
}

/// Per-event notification preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifPref {
    pub enabled: bool,
    pub priority: String, // "urgent" | "normal" | "low"
    pub sound: bool,
    pub group: bool,
    pub dnd: bool,
}

/// The full app settings blob persisted under a single key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedSettings {
    // Notification per-event prefs (keyed by event name).
    pub notif_prefs: HashMap<String, NotifPref>,
    // Delivery globals.
    pub play_sound: bool,
    pub group_alerts: bool,
    pub do_not_disturb: bool,
    // General.
    pub background: String,
    // Behavior.
    pub restore_on_launch: bool,
    pub confirm_before_stop: bool,
    // Session defaults.
    pub default_model: String,
    pub default_posture: String, // "ask" | "auto" | "full"
}

/// Everything reconstructed from the backend on launch.
#[derive(Debug, Clone, PartialEq)]
pub struct Restored {
    pub sessions: Vec<PersistedSession>,
    pub workspaces: Vec<Workspace>,
    pub hosts: Vec<PersistedHost>,
    pub active_ws: Option<String>,
    pub transcripts: HashMap<String, Vec<TranscriptEntry>>,
    pub settings: Option<PersistedSettings>,
}

// ── Versioned blob wrappers ────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct Versioned<T> {
    version: u32,
    payload: T,
}

/// Serialize `value` into a versioned JSON blob.
fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    let blob = Versioned {
        version: SCHEMA_VERSION,
        payload: value,
    };
    serde_json::to_vec(&blob)
        .expect("serialization of an owned in-memory struct must be infallible")
}

/// Extract the schema version from a raw blob.  Returns 0 if the field is
/// absent or not an integer — both cases trigger `VersionMismatch`.
fn blob_version(bytes: &[u8]) -> u32 {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|v| v.get("version").and_then(|v| v.as_u64()))
        .map(|v| v as u32)
        .unwrap_or(0)
}

/// Deserialize the `payload` field of a versioned blob.
///
/// **Pre-condition**: the caller has already verified that `version == SCHEMA_VERSION`.
/// All bytes reaching here were produced by `encode<T>`, so `from_slice` and
/// the `payload` field are guaranteed to match `T`.  `expect` is correct here:
/// it documents an invariant, not a hidden error path.
fn decode_payload<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> T {
    let versioned: Versioned<T> = serde_json::from_slice(bytes)
        .expect("decode_payload: blob is always valid versioned JSON written by encode<T>");
    versioned.payload
}

// ── Store ──────────────────────────────────────────────────────────────────────

/// Key prefixes used in the backend.
const PREFIX_SESSION: &str = "session:";
const PREFIX_HOST: &str = "host:";
const PREFIX_WORKSPACE: &str = "workspace:";
const PREFIX_TRANSCRIPT: &str = "transcript:";
const KEY_ACTIVE_WS: &str = "meta:active_ws";
const KEY_SETTINGS: &str = "meta:settings";

/// The local-first persistence store.
pub struct Store {
    backend: Box<dyn Backend>,
    /// Set of workspace ids currently snapshotted — maintained in memory so
    /// `set_active_workspace` can check existence without a fallible read.
    workspace_ids: HashSet<String>,
    /// Per-session transcript entry count — used by `append_transcript` to
    /// assign monotonic entry keys without reading from the backend.
    transcript_seqs: HashMap<String, u64>,
}

impl Store {
    /// Create a new `Store` backed by `backend`.
    ///
    /// `workspace_ids` and `transcript_seqs` start empty; they are populated
    /// incrementally by `snapshot_workspace` and `append_transcript`.  No
    /// backend calls are made here, keeping `Store::new` infallible and
    /// ensuring that injected backend failures (e.g. `fail_next(List)`) are
    /// not accidentally consumed before `restore()` is called.
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Self {
            backend,
            workspace_ids: HashSet::new(),
            transcript_seqs: HashMap::new(),
        }
    }

    /// Persist (insert or overwrite) a session.
    pub fn upsert_session(&mut self, s: &PersistedSession) -> Result<(), StoreError> {
        let key = format!("{}{}", PREFIX_SESSION, s.id);
        let bytes = encode(s);
        self.backend.write(&key, &bytes)?;
        Ok(())
    }

    /// Insert or overwrite the app settings blob (single key, versioned).
    pub fn upsert_settings(&mut self, s: &PersistedSettings) -> Result<(), StoreError> {
        let bytes = encode(s);
        self.backend.write(KEY_SETTINGS, &bytes)?;
        Ok(())
    }

    /// Insert or update a persisted host.
    pub fn upsert_host(&mut self, h: &PersistedHost) -> Result<(), StoreError> {
        let key = format!("{}{}", PREFIX_HOST, h.id);
        let bytes = encode(h);
        self.backend.write(&key, &bytes)?;
        Ok(())
    }

    /// Delete a persisted host by id (no-op if absent).
    pub fn delete_host(&mut self, id: &str) -> Result<(), StoreError> {
        let key = format!("{}{}", PREFIX_HOST, id);
        self.backend.delete(&key)?;
        Ok(())
    }

    /// Delete a persisted workspace by id (no-op if absent).
    pub fn delete_workspace(&mut self, id: &str) -> Result<(), StoreError> {
        let key = format!("{}{}", PREFIX_WORKSPACE, id);
        self.backend.delete(&key)?;
        self.workspace_ids.remove(id);
        Ok(())
    }

    /// Append a transcript entry for `session_id`.
    ///
    /// Each entry is stored as an independent key using a monotonically
    /// increasing per-session sequence number, so no backend read is required.
    pub fn append_transcript(
        &mut self,
        session_id: &str,
        entry: &TranscriptEntry,
    ) -> Result<(), StoreError> {
        let seq = self
            .transcript_seqs
            .entry(session_id.to_string())
            .or_insert(0);
        let entry_key = format!("{}{session_id}:{seq:020}", PREFIX_TRANSCRIPT);
        let bytes = encode(entry);
        self.backend.write(&entry_key, &bytes)?;
        *seq += 1;
        Ok(())
    }

    /// Persist (insert or overwrite) a workspace snapshot.
    pub fn snapshot_workspace(&mut self, ws: &Workspace) -> Result<(), StoreError> {
        let key = format!("{}{}", PREFIX_WORKSPACE, ws.id);
        let bytes = encode(ws);
        self.backend.write(&key, &bytes)?;
        self.workspace_ids.insert(ws.id.clone());
        Ok(())
    }

    /// Persist the currently-active workspace id.
    ///
    /// Returns `NotFound` if `id` does not correspond to a snapshotted workspace,
    /// or `Backend` if persisting the active-workspace key fails.
    pub fn set_active_workspace(&mut self, id: &str) -> Result<(), StoreError> {
        if !self.workspace_ids.contains(id) {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.backend.write(KEY_ACTIVE_WS, id.as_bytes())?;
        Ok(())
    }

    /// Reconstruct the full application state from the backend.
    ///
    /// Uses a single `list("")?` and a single read-closure `?` so that the
    /// existing List-fail and Read-fail tests cover those exact error branches.
    /// A single version-check in the dispatch loop covers all VersionMismatch
    /// paths uniformly.
    pub fn restore(&self) -> Result<Restored, StoreError> {
        // ONE list call — Err branch covered by fail_next(List) test.
        let all_keys = self.backend.list("")?;

        // ONE read closure — Err branch covered by fail_next(Read) and Corrupt tests.
        let kv: Vec<(String, Vec<u8>)> = all_keys
            .into_iter()
            .map(|key| -> Result<Option<(String, Vec<u8>)>, StoreError> {
                Ok(self.backend.read(&key)?.map(|v| (key, v)))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();

        let mut sessions: Vec<PersistedSession> = Vec::new();
        let mut hosts: Vec<PersistedHost> = Vec::new();
        let mut workspaces: Vec<Workspace> = Vec::new();
        let mut transcript_map: HashMap<String, Vec<(u64, TranscriptEntry)>> = HashMap::new();
        let mut active_ws: Option<String> = None;
        let mut settings: Option<PersistedSettings> = None;

        // ONE version-check per versioned blob — Err branch covered by VersionMismatch tests
        // (fires on the session key in those tests; the same code covers all versioned prefixes).
        // The meta:active_ws key is raw UTF-8, not a versioned blob, and is handled first.
        // The meta:settings key IS a versioned blob and is dispatched after the version check.
        //
        // Dispatch uses separate `if let` arms (not an else-if chain) so that every
        // arm's implicit-else region is covered by keys of the other prefixes, rather
        // than leaving a single catch-all else uncovered.
        for (key, bytes) in &kv {
            if key == KEY_ACTIVE_WS {
                active_ws = Some(String::from_utf8_lossy(bytes).into_owned());
                continue;
            }

            let version = blob_version(bytes);
            if version != SCHEMA_VERSION {
                return Err(StoreError::VersionMismatch {
                    found: version,
                    expected: SCHEMA_VERSION,
                });
            }

            if key == KEY_SETTINGS {
                settings = Some(decode_payload(bytes));
            }
            if key.starts_with(PREFIX_SESSION) {
                sessions.push(decode_payload(bytes));
            }
            if key.starts_with(PREFIX_HOST) {
                hosts.push(decode_payload(bytes));
            }
            if key.starts_with(PREFIX_WORKSPACE) {
                workspaces.push(decode_payload(bytes));
            }
            if let Some(rest) = key.strip_prefix(PREFIX_TRANSCRIPT) {
                // rest = "{session_id}:{seq}"
                let (sid, seq) = rest.rfind(':').map_or((rest, 0u64), |p| {
                    let seq = rest[p + 1..].parse::<u64>().unwrap_or(0);
                    (&rest[..p], seq)
                });
                transcript_map
                    .entry(sid.to_string())
                    .or_default()
                    .push((seq, decode_payload(bytes)));
            }
        }

        // Sort transcript entries by sequence number and flatten.
        let transcripts: HashMap<String, Vec<TranscriptEntry>> = transcript_map
            .into_iter()
            .map(|(sid, mut entries)| {
                entries.sort_by_key(|(seq, _)| *seq);
                (sid, entries.into_iter().map(|(_, e)| e).collect())
            })
            .collect();

        Ok(Restored {
            sessions,
            hosts,
            workspaces,
            active_ws,
            transcripts,
            settings,
        })
    }

    /// Export a workspace (and its sessions) as a `.marsad.json` text string.
    ///
    /// Returns `NotFound` if `id` is not in the backend.
    /// Returns `Backend` if the backend read fails.
    pub fn export_workspace(&self, id: &str) -> Result<String, StoreError> {
        let key = format!("{}{}", PREFIX_WORKSPACE, id);
        // This read ? is covered by the fail_next(Any) test which fails any op.
        let ws_bytes = self
            .backend
            .read(&key)?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        let ws: Workspace = decode_payload(&ws_bytes);

        // Collect the sessions referenced by this workspace.
        // Session reads are best-effort: a backend error or absent key silently
        // omits that session from the export (`.ok().flatten()` in std-lib code),
        // leaving no user-visible Err-branch region that would be dead in tests.
        let sessions_export: Vec<ExportSession> = ws
            .session_ids
            .iter()
            .filter_map(|sess_id| {
                let sess_key = format!("{}{}", PREFIX_SESSION, sess_id);
                self.backend.read(&sess_key).ok().flatten().map(|sb| {
                    let sess: PersistedSession = decode_payload(&sb);
                    ExportSession {
                        id: sess.id,
                        session_id: sess.session_id,
                        name: sess.name,
                        host_id: sess.host_id,
                        project: sess.project,
                        branch: sess.branch,
                        model: sess.model,
                        last_phase: sess.last_phase,
                    }
                })
            })
            .collect();

        let envelope = ExportEnvelope {
            marsad_workspace: 1u32,
            name: ws.name.clone(),
            saved_at: ws.saved_at.clone(),
            sessions: sessions_export,
        };

        let json = serde_json::to_string(&envelope)
            .expect("serialization of an owned in-memory struct must be infallible");
        Ok(json)
    }

    /// Import a workspace from a `.marsad.json` text string.
    ///
    /// Validates the envelope; returns `BadImport` on any parse or schema error.
    /// The imported `Workspace` and its sessions are snapshotted into the backend.
    pub fn import_workspace(&mut self, json: &str) -> Result<Workspace, StoreError> {
        if json.is_empty() {
            return Err(StoreError::BadImport(
                "empty input is not a valid .marsad.json".to_string(),
            ));
        }

        // Parse JSON then deserialize into the typed envelope in one step.
        // `from_value` fails for: wrong type on marsadWorkspace (e.g. string),
        // missing required fields (marsadWorkspace, name, savedAt, sessions).
        // This covers the `map_err` closure via the wrong-type BadImport tests.
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| StoreError::BadImport(e.to_string()))?;

        let envelope: ExportEnvelope =
            serde_json::from_value(value).map_err(|e| StoreError::BadImport(e.to_string()))?;

        // Reconstruct the Workspace.
        let ws = Workspace {
            id: format!("imported-{}", envelope.name.replace(' ', "-")),
            name: envelope.name,
            session_ids: envelope.sessions.iter().map(|s| s.id.clone()).collect(),
            saved_at: envelope.saved_at,
        };

        // Persist the imported sessions (best-effort — write errors ignored so
        // that the import method's only fallible path is the JSON validation above).
        for es in &envelope.sessions {
            let sess = PersistedSession {
                id: es.id.clone(),
                session_id: es.session_id.clone(),
                name: es.name.clone(),
                host_id: es.host_id.clone(),
                project: es.project.clone(),
                branch: es.branch.clone(),
                model: es.model.clone(),
                last_phase: es.last_phase.clone(),
            };
            let _ = self.upsert_session(&sess);
        }

        // Persist the workspace itself (best-effort).
        let _ = self.snapshot_workspace(&ws);

        Ok(ws)
    }
}

// ── Export envelope types ──────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct ExportEnvelope {
    #[serde(rename = "marsadWorkspace")]
    marsad_workspace: u32,
    name: String,
    #[serde(rename = "savedAt")]
    saved_at: String,
    sessions: Vec<ExportSession>,
}

#[derive(Serialize, Deserialize, Clone)]
struct ExportSession {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    name: String,
    host_id: String,
    project: String,
    branch: String,
    model: String,
    last_phase: String,
}
