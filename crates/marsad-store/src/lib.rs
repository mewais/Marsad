//! marsad-store — persistence of Marsad-level data, owned by the hub daemon.
//!
//! # Design
//!
//! * **One writer.** [`Store::open`] takes an exclusive lock on `<dir>/store.lock` and spawns
//!   one task that owns all state. Every read and write is a message to that task (bounded
//!   mailbox, reply on a oneshot), so there are no locks and no lost updates, and a write is
//!   durable when its call returns.
//! * **Small JSON documents, written atomically.** Each kind of data is one file
//!   (`hosts.json`, `settings.json`, …) of the form `{"version": 1, "data": …}`, written to
//!   `<file>.tmp`, fsynced, then renamed over the old file (rename replaces atomically on every
//!   platform we support), so a crash leaves either the old or the new document, never a torn
//!   one. The data is small (tens of hosts, a few hundred markers), human-inspectable, and
//!   needs no native dependency — which is why this is not sqlite. Only the document a write
//!   touches is rewritten.
//! * **No migration.** A document with another `version` is refused with
//!   [`StoreError::Version`] (formats may break between releases; nothing is migrated).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use marsad_proto::agentconfig::{LayerScope, OverrideLayer};
use marsad_proto::hosts::HostEntry;
use marsad_proto::store::{
    NotificationPrefs, RecentProject, SeenMark, Settings, StoredLayer, WorkstreamDoc, WorkstreamPut,
};
use marsad_proto::{HostId, MarsadError, SessionId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};
use wasit_control::InstanceLock;

/// Format version of every document.
pub const VERSION: u32 = 1;
/// Most recent projects kept.
pub const MAX_RECENT: usize = 50;
const MAILBOX: usize = 256;
/// Bound on waiting for the writer (mailbox room + reply).
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A failed store operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("another process owns the store at {0}")]
    Locked(String),
    #[error("{path}: {message}")]
    Io { path: String, message: String },
    #[error("{path}: unsupported document version {found} (expected {VERSION})")]
    Version { path: String, found: u32 },
    #[error("{path}: {message}")]
    Corrupt { path: String, message: String },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("the store is closed")]
    Closed,
    #[error("the store did not answer in time")]
    Timeout,
}

impl From<StoreError> for MarsadError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(what) => MarsadError::NotFound { what },
            StoreError::Invalid(m) | StoreError::Conflict(m) => MarsadError::invalid(m),
            other => MarsadError::Store {
                message: other.to_string(),
            },
        }
    }
}

/// Which document a change touched (see [`Store::changes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Doc {
    Hosts,
    Settings,
    Notifications,
    Layers,
    Seen,
    Recent,
    Workstreams,
}

impl Doc {
    pub const ALL: [Doc; 7] = [
        Doc::Hosts,
        Doc::Settings,
        Doc::Notifications,
        Doc::Layers,
        Doc::Seen,
        Doc::Recent,
        Doc::Workstreams,
    ];

    pub fn file_name(self) -> &'static str {
        match self {
            Doc::Hosts => "hosts.json",
            Doc::Settings => "settings.json",
            Doc::Notifications => "notifications.json",
            Doc::Layers => "layers.json",
            Doc::Seen => "seen.json",
            Doc::Recent => "recent.json",
            Doc::Workstreams => "workstreams.json",
        }
    }
}

/// Everything the store holds.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub hosts: Vec<HostEntry>,
    pub settings: Settings,
    pub notifications: NotificationPrefs,
    pub layers: Vec<StoredLayer>,
    pub seen: BTreeMap<(HostId, SessionId), SeenMark>,
    pub recent: Vec<RecentProject>,
    pub workstreams: BTreeMap<String, WorkstreamDoc>,
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    version: u32,
    data: T,
}

type ReadFn = Box<dyn FnOnce(&State) + Send>;
type WriteFn = Box<dyn FnOnce(&mut State) -> Result<(), StoreError> + Send>;

enum Op {
    Read(ReadFn),
    Write {
        doc: Doc,
        apply: WriteFn,
        done: oneshot::Sender<Result<(), StoreError>>,
    },
    Close(oneshot::Sender<()>),
}

/// Handle to the store. Cheap to clone; the writer task ends when the last handle is dropped.
#[derive(Clone)]
pub struct Store {
    tx: mpsc::Sender<Op>,
    changes: broadcast::Sender<Doc>,
    dir: PathBuf,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn io_err(path: &Path, e: impl std::fmt::Display) -> StoreError {
    StoreError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

async fn load<T: DeserializeOwned + Default>(path: &Path) -> Result<T, StoreError> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(io_err(path, e)),
    };
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| StoreError::Corrupt {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let found = v.get("version").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    if found != VERSION {
        return Err(StoreError::Version {
            path: path.display().to_string(),
            found,
        });
    }
    let env: Envelope<T> = serde_json::from_value(v).map_err(|e| StoreError::Corrupt {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    Ok(env.data)
}

/// Write `data` to `path` atomically (tmp file, fsync, rename).
async fn save<T: Serialize>(path: &Path, data: &T) -> Result<(), StoreError> {
    use tokio::io::AsyncWriteExt;
    let bytes = serde_json::to_vec_pretty(&Envelope {
        version: VERSION,
        data,
    })
    .map_err(|e| io_err(path, e))?;
    let tmp = path.with_extension("json.tmp");
    let mut f = tokio::fs::File::create(&tmp)
        .await
        .map_err(|e| io_err(&tmp, e))?;
    f.write_all(&bytes).await.map_err(|e| io_err(&tmp, e))?;
    f.sync_all().await.map_err(|e| io_err(&tmp, e))?;
    drop(f);
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| io_err(path, e))?;
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        // Make the rename itself durable. Failing to fsync a directory only weakens
        // durability across power loss, not consistency.
        if let Ok(d) = tokio::fs::File::open(dir).await {
            if let Err(e) = d.sync_all().await {
                tracing::debug!(error = %e, "fsync of the store directory failed");
            }
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Default)]
struct SeenDoc {
    markers: Vec<SeenMark>,
}

async fn persist(dir: &Path, doc: Doc, st: &State) -> Result<(), StoreError> {
    let path = dir.join(doc.file_name());
    match doc {
        Doc::Hosts => save(&path, &st.hosts).await,
        Doc::Settings => save(&path, &st.settings).await,
        Doc::Notifications => save(&path, &st.notifications).await,
        Doc::Layers => save(&path, &st.layers).await,
        Doc::Seen => {
            save(
                &path,
                &SeenDoc {
                    markers: st.seen.values().cloned().collect(),
                },
            )
            .await
        }
        Doc::Recent => save(&path, &st.recent).await,
        Doc::Workstreams => {
            let v: Vec<&WorkstreamDoc> = st.workstreams.values().collect();
            save(&path, &v).await
        }
    }
}

impl Store {
    /// Open (creating) the store in `dir`. Fails with [`StoreError::Locked`] if another
    /// process has it open.
    pub async fn open(dir: impl Into<PathBuf>) -> Result<Store, StoreError> {
        let dir = dir.into();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| io_err(&dir, e))?;
        let lock = InstanceLock::try_acquire(dir.join("store.lock"))
            .await
            .map_err(|e| match e {
                wasit_control::LockError::Held(p) => StoreError::Locked(p.display().to_string()),
                other => io_err(&dir, other),
            })?;
        let f = |d: Doc| dir.join(d.file_name());
        let seen: SeenDoc = load(&f(Doc::Seen)).await?;
        let workstreams: Vec<WorkstreamDoc> = load(&f(Doc::Workstreams)).await?;
        let state = State {
            hosts: load(&f(Doc::Hosts)).await?,
            settings: load(&f(Doc::Settings)).await?,
            notifications: load(&f(Doc::Notifications)).await?,
            layers: load(&f(Doc::Layers)).await?,
            seen: seen
                .markers
                .into_iter()
                .map(|m| ((m.host.clone(), m.session.clone()), m))
                .collect(),
            recent: load(&f(Doc::Recent)).await?,
            workstreams: workstreams.into_iter().map(|w| (w.id.clone(), w)).collect(),
        };
        let (tx, rx) = mpsc::channel(MAILBOX);
        let changes = broadcast::channel(64).0;
        tokio::spawn(writer(dir.clone(), state, rx, changes.clone(), lock));
        Ok(Store { tx, changes, dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every committed write, by document. A lagging receiver should re-read.
    pub fn changes(&self) -> broadcast::Receiver<Doc> {
        self.changes.subscribe()
    }

    async fn send(&self, op: Op) -> Result<(), StoreError> {
        match tokio::time::timeout(CALL_TIMEOUT, self.tx.send(op)).await {
            Err(_) => Err(StoreError::Timeout),
            Ok(Err(_)) => Err(StoreError::Closed),
            Ok(Ok(())) => Ok(()),
        }
    }

    /// Read a value out of the state.
    pub async fn read<R: Send + 'static>(
        &self,
        f: impl FnOnce(&State) -> R + Send + 'static,
    ) -> Result<R, StoreError> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::Read(Box::new(move |s| {
            // The caller may have given up waiting.
            let _ = tx.send(f(s));
        })))
        .await?;
        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Err(_) => Err(StoreError::Timeout),
            Ok(Err(_)) => Err(StoreError::Closed),
            Ok(Ok(r)) => Ok(r),
        }
    }

    /// Change one document; durable when this returns `Ok`. If `f` fails nothing changes.
    pub async fn write<R: Send + 'static>(
        &self,
        doc: Doc,
        f: impl FnOnce(&mut State) -> Result<R, StoreError> + Send + 'static,
    ) -> Result<R, StoreError> {
        let (rtx, rrx) = oneshot::channel();
        let (dtx, drx) = oneshot::channel();
        self.send(Op::Write {
            doc,
            apply: Box::new(move |s| {
                let r = f(s)?;
                // The caller may have given up waiting.
                let _ = rtx.send(r);
                Ok(())
            }),
            done: dtx,
        })
        .await?;
        match tokio::time::timeout(CALL_TIMEOUT, drx).await {
            Err(_) => return Err(StoreError::Timeout),
            Ok(Err(_)) => return Err(StoreError::Closed),
            Ok(Ok(r)) => r?,
        }
        rrx.await.map_err(|_| StoreError::Closed)
    }

    /// Stop the writer after the writes already queued and release the lock; resolves once
    /// another process can open the store. Other handles get [`StoreError::Closed`].
    pub async fn close(self) -> Result<(), StoreError> {
        let (tx, rx) = oneshot::channel();
        self.send(Op::Close(tx)).await?;
        tokio::time::timeout(CALL_TIMEOUT, rx)
            .await
            .map_err(|_| StoreError::Timeout)?
            .map_err(|_| StoreError::Closed)
    }

    // ── hosts ───────────────────────────────────────────────────────────────

    pub async fn hosts(&self) -> Result<Vec<HostEntry>, StoreError> {
        self.read(|s| s.hosts.clone()).await
    }

    /// Insert or replace (by id).
    pub async fn put_host(&self, host: HostEntry) -> Result<(), StoreError> {
        self.write(Doc::Hosts, move |s| {
            match s.hosts.iter_mut().find(|h| h.id == host.id) {
                Some(h) => *h = host,
                None => s.hosts.push(host),
            }
            Ok(())
        })
        .await
    }

    /// Remove a host, its host layer, seen-markers and recent projects.
    pub async fn remove_host(&self, id: &HostId) -> Result<(), StoreError> {
        let id = id.clone();
        let id2 = id.clone();
        self.write(Doc::Hosts, move |s| {
            let before = s.hosts.len();
            s.hosts.retain(|h| h.id != id);
            if s.hosts.len() == before {
                return Err(StoreError::NotFound(format!("host {id}")));
            }
            Ok(())
        })
        .await?;
        let id3 = id2.clone();
        self.write(Doc::Layers, move |s| {
            s.layers.retain(|l| match &l.scope {
                LayerScope::Host { host } | LayerScope::Session { host, .. } => *host != id2,
                _ => true,
            });
            Ok(())
        })
        .await?;
        let id4 = id3.clone();
        self.write(Doc::Seen, move |s| {
            s.seen.retain(|(h, _), _| *h != id3);
            Ok(())
        })
        .await?;
        self.write(Doc::Recent, move |s| {
            s.recent.retain(|r| r.host != id4);
            Ok(())
        })
        .await
    }

    // ── settings / notifications ────────────────────────────────────────────

    pub async fn settings(&self) -> Result<Settings, StoreError> {
        self.read(|s| s.settings.clone()).await
    }

    /// Set (or with `null`, remove) one setting; returns all settings.
    pub async fn set_setting(
        &self,
        key: String,
        value: serde_json::Value,
    ) -> Result<Settings, StoreError> {
        if key.is_empty() {
            return Err(StoreError::Invalid("empty setting key".into()));
        }
        self.write(Doc::Settings, move |s| {
            if value.is_null() {
                s.settings.values.remove(&key);
            } else {
                s.settings.values.insert(key, value);
            }
            Ok(s.settings.clone())
        })
        .await
    }

    pub async fn notifications(&self) -> Result<NotificationPrefs, StoreError> {
        self.read(|s| s.notifications).await
    }

    pub async fn set_notifications(&self, p: NotificationPrefs) -> Result<(), StoreError> {
        self.write(Doc::Notifications, move |s| {
            s.notifications = p;
            Ok(())
        })
        .await
    }

    // ── layers ──────────────────────────────────────────────────────────────

    pub async fn layers(&self) -> Result<Vec<StoredLayer>, StoreError> {
        self.read(|s| s.layers.clone()).await
    }

    /// The layer of `scope` (empty when none is stored).
    pub async fn layer(&self, scope: LayerScope) -> Result<OverrideLayer, StoreError> {
        self.read(move |s| {
            s.layers
                .iter()
                .find(|l| l.scope == scope)
                .map(|l| l.layer.clone())
                .unwrap_or_default()
        })
        .await
    }

    /// Store a layer; an empty layer removes it.
    pub async fn set_layer(
        &self,
        scope: LayerScope,
        layer: OverrideLayer,
    ) -> Result<(), StoreError> {
        self.write(Doc::Layers, move |s| {
            s.layers.retain(|l| l.scope != scope);
            if !layer.is_empty() {
                s.layers.push(StoredLayer { scope, layer });
                s.layers.sort_by(|a, b| a.scope.cmp(&b.scope));
            }
            Ok(())
        })
        .await
    }

    // ── seen markers ────────────────────────────────────────────────────────

    pub async fn seen(&self) -> Result<Vec<SeenMark>, StoreError> {
        self.read(|s| s.seen.values().cloned().collect()).await
    }

    /// Record markers; a marker never moves backwards (a lower cursor is ignored).
    pub async fn mark_seen(&self, marks: Vec<SeenMark>) -> Result<Vec<SeenMark>, StoreError> {
        self.write(Doc::Seen, move |s| {
            for mut m in marks {
                if m.at_ms == 0 {
                    m.at_ms = now_ms();
                }
                let key = (m.host.clone(), m.session.clone());
                match s.seen.get(&key) {
                    Some(old) if old.cursor > m.cursor => {}
                    _ => {
                        s.seen.insert(key, m);
                    }
                }
            }
            Ok(s.seen.values().cloned().collect())
        })
        .await
    }

    /// Drop the markers of sessions that no longer exist.
    pub async fn forget_seen(&self, host: HostId, session: SessionId) -> Result<(), StoreError> {
        self.write(Doc::Seen, move |s| {
            s.seen.remove(&(host, session));
            Ok(())
        })
        .await
    }

    // ── recent projects ─────────────────────────────────────────────────────

    pub async fn recent(&self) -> Result<Vec<RecentProject>, StoreError> {
        self.read(|s| s.recent.clone()).await
    }

    /// Move (or add) a project to the front; bounded to [`MAX_RECENT`].
    pub async fn add_recent(&self, mut p: RecentProject) -> Result<Vec<RecentProject>, StoreError> {
        if p.path.is_empty() {
            return Err(StoreError::Invalid("empty project path".into()));
        }
        if p.last_used_ms == 0 {
            p.last_used_ms = now_ms();
        }
        self.write(Doc::Recent, move |s| {
            s.recent.retain(|r| !(r.host == p.host && r.path == p.path));
            s.recent.insert(0, p);
            s.recent.truncate(MAX_RECENT);
            Ok(s.recent.clone())
        })
        .await
    }

    // ── workstreams ─────────────────────────────────────────────────────────

    pub async fn workstreams(&self) -> Result<Vec<WorkstreamDoc>, StoreError> {
        self.read(|s| s.workstreams.values().cloned().collect())
            .await
    }

    pub async fn put_workstream(&self, put: WorkstreamPut) -> Result<WorkstreamDoc, StoreError> {
        if put.id.is_empty() {
            return Err(StoreError::Invalid("empty workstream id".into()));
        }
        self.write(Doc::Workstreams, move |s| {
            let current = s.workstreams.get(&put.id).map(|w| w.revision).unwrap_or(0);
            if let Some(expected) = put.expected_revision {
                if expected != current {
                    return Err(StoreError::Conflict(format!(
                        "workstream {} is at revision {current}, not {expected}",
                        put.id
                    )));
                }
            }
            let doc = WorkstreamDoc {
                id: put.id.clone(),
                schema: put.schema,
                revision: current + 1,
                updated_at_ms: now_ms(),
                doc: put.doc,
            };
            s.workstreams.insert(put.id, doc.clone());
            Ok(doc)
        })
        .await
    }

    pub async fn delete_workstream(&self, id: String) -> Result<(), StoreError> {
        self.write(Doc::Workstreams, move |s| {
            s.workstreams
                .remove(&id)
                .map(|_| ())
                .ok_or(StoreError::NotFound(format!("workstream {id}")))
        })
        .await
    }
}

async fn writer(
    dir: PathBuf,
    mut state: State,
    mut rx: mpsc::Receiver<Op>,
    changes: broadcast::Sender<Doc>,
    lock: InstanceLock,
) {
    while let Some(op) = rx.recv().await {
        match op {
            Op::Close(done) => {
                drop(lock);
                // The closer may have given up waiting.
                let _ = done.send(());
                return;
            }
            Op::Read(f) => f(&state),
            Op::Write { doc, apply, done } => {
                // Apply to a copy so a failed callback or a failed write changes nothing.
                let mut next = state.clone();
                let res = match apply(&mut next) {
                    Err(e) => Err(e),
                    Ok(()) => match persist(&dir, doc, &next).await {
                        Ok(()) => {
                            state = next;
                            // No subscribers is fine.
                            let _ = changes.send(doc);
                            Ok(())
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "store write failed");
                            Err(e)
                        }
                    },
                };
                // The caller may have given up waiting; the write stands either way.
                let _ = done.send(res);
            }
        }
    }
}
