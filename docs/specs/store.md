# B06 — `marsad-store`

**Tier:** L1 (logic behind a mockable trait). **Mode:** sync API. **Coverage:** 100% line+region
(via the in-memory `Backend` fake; the fs `Backend` impl is exercised by `i04` integration).

## Purpose

Local-first persistence: sessions, transcripts, and workspaces; continuous snapshotting; restore on
relaunch; and `.marsad.json` workspace export/import. All storage goes through a `Backend` trait so
the crate is 100%-coverable with an in-memory fake; a filesystem impl backs production.

## Public API (authoritative)

```rust
pub trait Backend {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), StoreError>;
    fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError>;
    fn delete(&self, key: &str) -> Result<(), StoreError>;
}

pub struct MemoryBackend { /* ... */ }   // in-crate fake for tests
impl MemoryBackend { pub fn new() -> Self; pub fn corrupt(&self, key: &str); }  // corrupt() to test recovery
pub struct FsBackend { /* root dir */ }  // production
impl FsBackend { pub fn new(root: impl Into<std::path::PathBuf>) -> Self; }

pub struct Store { /* ... */ }
impl Store {
    pub fn new(backend: Box<dyn Backend>) -> Self;
    pub fn upsert_session(&mut self, s: &PersistedSession) -> Result<(), StoreError>;
    pub fn append_transcript(&mut self, session_id: &str, entry: &TranscriptEntry) -> Result<(), StoreError>;
    pub fn snapshot_workspace(&mut self, ws: &Workspace) -> Result<(), StoreError>;
    pub fn restore(&self) -> Result<Restored, StoreError>;
    pub fn export_workspace(&self, id: &str) -> Result<String, StoreError>;   // .marsad.json text
    pub fn import_workspace(&mut self, json: &str) -> Result<Workspace, StoreError>;
}

pub struct PersistedSession { pub id: String, pub session_id: Option<String>, pub name: String, pub host_id: String, pub project: String, pub branch: String, pub model: String, pub last_phase: String }
pub struct TranscriptEntry { /* type + payload mirroring proto/events */ }
pub struct Workspace { pub id: String, pub name: String, pub session_ids: Vec<String>, pub saved_at: String }
pub struct Restored { pub sessions: Vec<PersistedSession>, pub workspaces: Vec<Workspace>, pub active_ws: Option<String>, pub transcripts: std::collections::HashMap<String, Vec<TranscriptEntry>> }

pub enum StoreError { Backend(String), Corrupt { key: String }, VersionMismatch { found: u32, expected: u32 }, BadImport(String), NotFound(String) }
```

Derive `Debug, Clone, PartialEq` (+ serde) on the data types.

## Behavior notes

- Each persisted blob carries a schema `version`; reading an older/newer one → `VersionMismatch`
  (caller decides migrate vs reset).
- `restore` reads everything under the data root and reconstructs `Restored`. A session whose
  `session_id` is present but whose runtime is gone is **not** the store's concern — lifecycle (B09)
  classifies Stale/Detached; the store just returns what was persisted.
- A blob that fails to parse → `Corrupt{key}` (use `MemoryBackend::corrupt` to test).
- `export_workspace` serializes `{ marsadWorkspace:1, name, savedAt, sessions:[...] }` (mirror the
  `.dc.html` exporter). `import_workspace` validates the envelope; bad JSON/shape → `BadImport`.
- Round-trip: export then import yields an equivalent `Workspace` (+ its sessions) — `i08` asserts
  fidelity.

## Test buckets (`tests/`)

- `happy.rs` — upsert/append/snapshot/restore round-trip on `MemoryBackend`; export→import.
- `errors.rs` — `Corrupt`, `VersionMismatch`, `BadImport`, `NotFound`, backend write/read failure.
- `branches.rs` — empty restore; multiple workspaces + active selection; transcript accumulation;
  list-by-prefix.
- `fakes.rs` — `Backend` contract exercised against `MemoryBackend` (read/write/list/delete, overwrite,
  delete-missing).

## Edit scope

Code-agent: write `crates/marsad-store/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
