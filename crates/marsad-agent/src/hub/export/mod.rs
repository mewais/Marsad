//! The diagnostics export (design §6.5–§6.7): `diagnostics.export.start` and the topic
//! `diagnostics.exports` of the hub.
//!
//! * [`job`]: the exporter, one run from the fleet's hosts to a finished file.
//! * [`facts`]: what the manifest says about each host.
//! * [`source`]: where logs are read from (a host's `DiagService`, or files of this machine).
//! * [`gzip`]: the file's writer (gzip members on a thread of its own, the size cap).
//! * this module: [`Exports`], the owner of the exports' state. One actor holds the status of
//!   the running export and of the last [`EXPORTS_KEPT`] ones and publishes every change on the
//!   topic (replayable like `fleet`, see [`super::ring`]); the job is a task of its own that
//!   reports to the actor. `diagnostics.export.start` only registers the export and starts the
//!   task: it answers at once, and nothing of the work (remote reads, compression, the file)
//!   happens in a request handler.
//!
//! **One export at a time.** A second `diagnostics.export.start` while one runs is refused with
//! the `conflict` error naming the running export (its id): the caller follows that one on the
//! topic, or starts another when it is done. Nothing is queued and nothing is silently ignored.
//!
//! A job that fails (no file could be written) or whose task panics ends as a `failed` status
//! with its reason, and as an `export.done` record with the error.

pub mod facts;
pub mod gzip;
pub mod job;
pub mod source;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use marsad_proto::diag::MarsadEvent;
use marsad_proto::export::{
    ExportRequest, ExportSource, ExportSourceState, ExportStarted, ExportState, ExportStatus,
    ExportsEvent, DEFAULT_WINDOW_MS, EXPORTS_KEPT, EXPORT_DIR, MAX_WINDOW_MS,
};
use marsad_proto::{HostId, MarsadError};
use tokio::sync::{mpsc, oneshot};
use wasit_control::{Cursor, EventStream, RpcError, Supervised};
use wasit_diag::{Bytes, Ident, Ms};
use wasit_runtime::RuntimeClient;

use super::fleet::Fleet;
use super::ring::{self, Joined, Ring};
use crate::diag;
use crate::util::now_ms;
use facts::HostSpec;
use job::{HubFacts, Job, Limits, Mode, Progress, Update};
use source::FileLogs;

/// What the exporter needs of the hub.
pub struct ExportEnv {
    pub fleet: Fleet,
    /// The hub's own routing, so any host can be asked (see `serve::loopback`).
    pub link: Supervised,
    /// `<data>/diag`.
    pub diag_dir: PathBuf,
    pub hub: HubFacts,
    pub limits: Limits,
}

enum Change {
    Sources(Vec<ExportSource>),
    Source(ExportSource),
    Done {
        file: String,
        bytes: u64,
        complete: bool,
    },
    Failed(String),
}

enum Input {
    Start(
        ExportRequest,
        oneshot::Sender<Result<ExportStarted, MarsadError>>,
    ),
    Change(String, Change),
    Subscribe(Option<String>, oneshot::Sender<Joined<ExportsEvent>>),
}

/// Handle to the exports' owner. Cheap to clone.
#[derive(Clone)]
pub struct Exports {
    tx: mpsc::UnboundedSender<Input>,
}

struct State {
    exports: VecDeque<ExportStatus>,
    running: Option<String>,
    ring: Ring<ExportsEvent>,
}

impl State {
    fn upsert(&mut self, id: &str) {
        if let Some(s) = self.exports.iter().find(|s| s.id == id) {
            self.ring.emit(ExportsEvent::Upsert { export: s.clone() });
        }
    }

    fn change(&mut self, id: &str, change: Change) {
        let Some(s) = self.exports.iter_mut().find(|s| s.id == id) else {
            // An export older than the ones kept.
            return;
        };
        match change {
            Change::Sources(v) => s.sources = v,
            Change::Source(one) => match s.sources.iter_mut().find(|x| x.source == one.source) {
                Some(x) => *x = one,
                None => s.sources.push(one),
            },
            Change::Done {
                file,
                bytes,
                complete,
            } => {
                s.state = ExportState::Done;
                s.file = Some(file);
                s.bytes = Some(bytes);
                s.complete = Some(complete);
                if self.running.as_deref() == Some(id) {
                    self.running = None;
                }
            }
            Change::Failed(error) => {
                s.state = ExportState::Failed;
                s.error = Some(error);
                // Sources still reading will not be read.
                for x in &mut s.sources {
                    if matches!(
                        x.state,
                        ExportSourceState::Waiting | ExportSourceState::Reading
                    ) {
                        x.state = ExportSourceState::Failed;
                        x.reason = Some("the export failed".into());
                    }
                }
                if self.running.as_deref() == Some(id) {
                    self.running = None;
                }
            }
        }
        self.upsert(id);
    }
}

impl Exports {
    /// Starts the owner of the exports' state.
    pub fn spawn(env: ExportEnv) -> Exports {
        let (tx, rx) = mpsc::unbounded_channel();
        let me = Exports { tx };
        tokio::spawn(actor(Arc::new(env), me.clone(), rx));
        me
    }

    /// `diagnostics.export.start`: registers the export and starts its task; answers at once.
    pub async fn start(&self, req: ExportRequest) -> Result<ExportStarted, MarsadError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Input::Start(req, reply))
            .map_err(|_| MarsadError::io("the exports' owner is gone"))?;
        rx.await
            .map_err(|_| MarsadError::io("the exports' owner is gone"))?
    }

    /// The topic `diagnostics.exports`.
    pub async fn subscribe(&self, from: Option<Cursor>) -> Result<EventStream, RpcError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Input::Subscribe(from.map(|c| c.0), reply))
            .map_err(|_| RpcError::internal("the exports' owner is gone"))?;
        let joined = rx
            .await
            .map_err(|_| RpcError::internal("the exports' owner is gone"))?;
        Ok(ring::stream(joined))
    }

    fn change(&self, id: &str, change: Change) {
        // The owner is gone only when the hub is shutting down.
        let _ = self.tx.send(Input::Change(id.to_owned(), change));
    }
}

async fn actor(env: Arc<ExportEnv>, me: Exports, mut rx: mpsc::UnboundedReceiver<Input>) {
    let mut st = State {
        exports: VecDeque::new(),
        running: None,
        ring: Ring::new(),
    };
    while let Some(input) = rx.recv().await {
        match input {
            Input::Start(req, reply) => {
                let _ = reply.send(start(&env, &me, &mut st, req));
            }
            Input::Change(id, change) => st.change(&id, change),
            Input::Subscribe(from, reply) => {
                let exports = &st.exports;
                let joined = st.ring.join(from.as_deref(), || ExportsEvent::Reset {
                    exports: exports.iter().cloned().collect(),
                });
                // The subscriber may have gone away.
                let _ = reply.send(joined);
            }
        }
    }
}

fn start(
    env: &Arc<ExportEnv>,
    me: &Exports,
    st: &mut State,
    req: ExportRequest,
) -> Result<ExportStarted, MarsadError> {
    let now = now_ms();
    let since = req
        .since_ms
        .unwrap_or_else(|| now.saturating_sub(DEFAULT_WINDOW_MS));
    if since > now {
        return Err(MarsadError::invalid("the window starts in the future"));
    }
    if now - since > MAX_WINDOW_MS {
        return Err(MarsadError::invalid("the window is longer than 31 days"));
    }
    if let Some(id) = &st.running {
        return Err(MarsadError::Conflict {
            message: format!("export {id} is already running"),
        });
    }
    let id = job::new_id(now);
    st.running = Some(id.clone());
    st.exports.push_front(ExportStatus {
        id: id.clone(),
        state: ExportState::Running,
        since_ms: since,
        started_ms: now,
        sources: Vec::new(),
        file: None,
        bytes: None,
        complete: None,
        error: None,
    });
    st.exports.truncate(EXPORTS_KEPT);
    st.upsert(&id);

    let task = tokio::spawn(run_export(
        env.clone(),
        me.clone(),
        id.clone(),
        since,
        req.include_trails,
    ));
    // A task that panics must still end as a failed export.
    let (me, watched) = (me.clone(), id.clone());
    tokio::spawn(async move {
        if let Err(e) = task.await {
            diag::emit(MarsadEvent::ExportDone {
                id: Ident::new(&watched),
                bytes: Bytes(0),
                complete: false,
                skipped: 0,
                ms: Ms(0),
                error: Some(diag::ours("task", &e)),
            });
            me.change(&watched, Change::Failed("the export task stopped".into()));
        }
    });
    Ok(ExportStarted { id })
}

/// The export's task: builds the job from the fleet as it is now and runs it.
async fn run_export(
    env: Arc<ExportEnv>,
    me: Exports,
    id: String,
    since: u64,
    include_trails: bool,
) {
    let started = Instant::now();
    let limits = env.limits.clone();
    let client = |host: &HostId| {
        RuntimeClient::new(env.link.clone())
            .with_host(host.clone())
            .with_deadlines(limits.call_deadline, limits.call_deadline)
    };
    let mut hosts: Vec<HostSpec> = env
        .fleet
        .snapshot()
        .await
        .iter()
        .map(|h| HostSpec::from_fleet(h, &client))
        .collect();
    // The hub's own computer first, then the others by id.
    hosts.sort_by(|a, b| (a.role != "hub", a.id.as_str()).cmp(&(b.role != "hub", b.id.as_str())));
    let progress: Progress = {
        let (me, id) = (me.clone(), id.clone());
        Arc::new(move |u| {
            me.change(
                &id,
                match u {
                    Update::Sources(v) => Change::Sources(v),
                    Update::Source(s) => Change::Source(s),
                },
            )
        })
    };
    let result = job::run(Job {
        id: id.clone(),
        mode: Mode::Hub,
        since_ms: since,
        include_trails,
        hub: HubFacts {
            host_id: env.hub.host_id.clone(),
            build: env.hub.build.clone(),
            target: env.hub.target.clone(),
            machine: env.hub.machine.clone(),
            pid: env.hub.pid,
        },
        hosts,
        app: Some(Arc::new(FileLogs::app(env.diag_dir.clone()))),
        local: None,
        dir: env.diag_dir.join(EXPORT_DIR),
        limits: env.limits.clone(),
        progress,
    })
    .await;
    match result {
        Ok(f) => me.change(
            &id,
            Change::Done {
                file: f.name,
                bytes: f.bytes,
                complete: f.complete,
            },
        ),
        Err(e) => {
            diag::emit(MarsadEvent::ExportDone {
                id: Ident::new(&id),
                bytes: Bytes(0),
                complete: false,
                skipped: 0,
                ms: diag::ms(started),
                error: Some(e.info),
            });
            me.change(&id, Change::Failed(e.words));
        }
    }
}
