//! Workstreams (hub): the actor that owns every [`Workstream`] document, derives the Wasl
//! topology from it and keeps every involved node in step, and aggregates the `workstreams`
//! topic.
//!
//! * **One owner.** Documents, the session snapshots of every host (from the fleet), the
//!   outbound tasks of every host's node (`wasl.tasks`) and the push state live in one task;
//!   everybody else sends it messages (bounded mailbox). Slow work (session launches,
//!   topology pushes) runs in spawned tasks that report back.
//! * **Topology.** Every committed change re-derives the topology; if it differs, the
//!   document's `topology_version` is bumped (persisted first) and the topology is pushed to
//!   every host in `topology_hosts` (idempotent, versioned). It is pushed again to a host
//!   whenever its link connects, and to all hosts when the hub starts.
//! * **Spawn outcomes.** Every host node publishes the spawn requests of its sessions that
//!   reached their final state (`wasl.spawns`, a projection of the node's own journal record:
//!   Wasl owns the outcome and its codes). The actor keeps the latest few per asking agent and
//!   lists them in each workstream's view; nothing here is stored: after a hub restart the
//!   feeds replay the nodes' journals.
//! * **Decisions.** `wasl.edges` events with cause `decision` ("always allow" chosen in a
//!   handoff gate) for the topology version in force are written into the document. Events
//!   for older versions (e.g. replayed after a hub restart, or overtaken by a later edit)
//!   are ignored.
//! * **Talking to nodes.** All node calls and subscriptions go through an in-process
//!   supervised connection to the hub's own routing service, so local and remote hosts are
//!   addressed the same way (`host` on each request) and subscriptions resume from their
//!   cursors across link drops.
//!
//! Extension point: sessions spawned by agents (`spawn_agent`, later) arrive as new nodes
//! through [`Workstreams::edit`] with their spawner connected.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

use futures::StreamExt;
use marsad_proto::fleet::{FleetEvent, FleetLink};
use marsad_proto::workstream::{
    EdgeView, HandoffGateView, NodeLaunch, NodeSession, NodeView, SpawnGateView, SpawnOutcomeView,
    TopologySync, Workstream, WorkstreamCost, WorkstreamState, WorkstreamView, WorkstreamsEvent,
    RECENT_SPAWNS,
};
use marsad_proto::{HostId, LinkState, MarsadError, SessionSnapshot};
use marsad_store::Store;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use wasit_control::{
    CallError, CloseReason, Cursor, EventStream, RequestOpts, RpcError, Supervised,
};
use wasit_core::GateRequest;
use wasl_proto::{
    methods as wasl_methods, spawn_request_id, topics as wasl_topics, AgentId, Edge,
    EdgeChangeCause, EdgeEvent, EdgePolicy, PeerCard, PeerStatus, Side, SpawnEnded, TaskEvent,
    TaskId, TaskState, TaskView, Topology, TopologyApplied,
};

use super::fleet::{Fleet, Follow, Resubscribe, Step};
use super::ring::{self, Joined, Ring};
use crate::hub_events;
use crate::util::now_ms;

const MAILBOX: usize = 1024;
/// Bound on one topology push request.
const PUSH_DEADLINE: Duration = Duration::from_secs(20);
/// How long one topology push request waits for the link to be connected (it was, when the push
/// started; a link that dropped meanwhile fails the request as retryable).
const PUSH_WAIT: Duration = Duration::from_secs(5);

/// A change to one document, applied by the actor to a copy (nothing changes on `Err`).
pub type EditFn = Box<dyn FnOnce(&mut Workstream) -> Result<(), MarsadError> + Send>;

type Reply<T> = oneshot::Sender<Result<T, MarsadError>>;

/// The result of one node launch, reported back to the actor.
pub type LaunchResult = (String, Result<NodeSession, MarsadError>);

enum Input {
    List(oneshot::Sender<Vec<Workstream>>),
    Get(String, Reply<Workstream>),
    Create(Box<Workstream>, Reply<Workstream>),
    Edit(String, Option<u64>, EditFn, Reply<Workstream>),
    Delete(String, Reply<()>),
    BeginLaunch(String, Reply<(Workstream, Vec<(String, NodeLaunch)>)>),
    EndLaunch(String, Vec<LaunchResult>, Reply<Workstream>),
    Snapshot(NodeSession, oneshot::Sender<Option<SessionSnapshot>>),
    Subscribe(Option<String>, oneshot::Sender<Joined<WorkstreamsEvent>>),
    Fleet(FleetEvent),
    Task(TaskEvent),
    Edge(EdgeEvent),
    Spawn(SpawnEnded),
    Pushed {
        ws: String,
        host: HostId,
        version: u64,
        result: Result<(), String>,
    },
}

/// Handle to the actor. Cheap to clone.
#[derive(Clone)]
pub struct Workstreams {
    tx: mpsc::Sender<Input>,
}

/// Whether a host's link is up (the local host's always is).
fn link_is_up(link: &FleetLink) -> bool {
    match link {
        FleetLink::Local => true,
        FleetLink::Remote { state } => state.is_connected(),
    }
}

/// The Wasl topology a document stands for (`version` included).
pub fn topology(ws: &Workstream) -> Topology {
    let mut t = Topology {
        workstream: ws.id.clone(),
        version: ws.topology_version,
        peers: Vec::new(),
        edges: Vec::new(),
        limits: ws.limits.clone(),
        paused: false,
    };
    if matches!(ws.state, WorkstreamState::Draft | WorkstreamState::Archived) {
        return t;
    }
    t.paused = ws.state == WorkstreamState::Paused;
    let agents: HashMap<&str, AgentId> = ws
        .nodes
        .iter()
        .filter_map(|n| {
            n.session.as_ref().map(|s| {
                (
                    n.id.as_str(),
                    AgentId::new(s.host.as_str(), s.session.as_str()),
                )
            })
        })
        .collect();
    for n in &ws.nodes {
        let Some(agent) = agents.get(n.id.as_str()) else {
            continue;
        };
        t.peers.push(PeerCard {
            agent: agent.clone(),
            name: n.name.clone(),
            driver: n
                .launch
                .as_ref()
                .map(|l| l.driver.to_string())
                .unwrap_or_default(),
            role: n.role.clone(),
            description: n.description.clone(),
            status: PeerStatus::Unknown,
        });
    }
    for e in &ws.edges {
        if let (Some(f), Some(to)) = (agents.get(e.from.as_str()), agents.get(e.to.as_str())) {
            t.edges.push(Edge {
                from: f.clone(),
                to: to.clone(),
                policy: e.policy,
            });
        }
    }
    t
}

/// The hosts a document's nodes run on (sessions only).
fn involved(ws: &Workstream) -> BTreeSet<HostId> {
    ws.nodes
        .iter()
        .filter_map(|n| n.session.as_ref().map(|s| s.host.clone()))
        .collect()
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A slug for an id derived from a display name.
pub fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').chars().take(48).collect::<String>();
    if s.is_empty() {
        "ws".into()
    } else {
        s
    }
}

/// Structural checks every stored document passes.
pub fn validate(ws: &Workstream) -> Result<(), MarsadError> {
    let bad = |m: String| MarsadError::invalid(m);
    if ws.name.trim().is_empty() {
        return Err(bad("a workstream needs a name".into()));
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut sessions = HashSet::new();
    for n in &ws.nodes {
        if !valid_id(&n.id) {
            return Err(bad(format!(
                "node id {:?} must be 1-64 characters of a-z, 0-9, - and _",
                n.id
            )));
        }
        if !ids.insert(n.id.as_str()) {
            return Err(bad(format!("two nodes have the id {:?}", n.id)));
        }
        if n.name.trim().is_empty() {
            return Err(bad(format!("node {} needs a name", n.id)));
        }
        if !names.insert(n.name.to_lowercase()) {
            return Err(bad(format!("two nodes are named {:?}", n.name)));
        }
        if n.session.is_none() && n.launch.is_none() {
            return Err(bad(format!(
                "node {} has neither a session nor a launch spec",
                n.id
            )));
        }
        if let Some(s) = &n.session {
            if !sessions.insert(s.clone()) {
                return Err(bad(format!(
                    "session {}/{} is in the workstream twice",
                    s.host, s.session
                )));
            }
        }
    }
    let mut edges = HashSet::new();
    for e in &ws.edges {
        if !ids.contains(e.from.as_str()) || !ids.contains(e.to.as_str()) {
            return Err(bad(format!(
                "connection {} → {} names an unknown node",
                e.from, e.to
            )));
        }
        if e.from == e.to {
            return Err(bad(format!("node {} cannot connect to itself", e.from)));
        }
        if !edges.insert((e.from.as_str(), e.to.as_str())) {
            return Err(bad(format!(
                "connection {} → {} is listed twice",
                e.from, e.to
            )));
        }
    }
    if ws.limits.max_depth == 0 {
        return Err(bad("max depth must be at least 1".into()));
    }
    if ws.limits.default_deadline_secs == 0 {
        return Err(bad("the default task deadline must be positive".into()));
    }
    if ws.limits.budget_usd.is_some_and(|b| b.is_nan() || b < 0.0) {
        return Err(bad("the budget must be a non-negative number".into()));
    }
    Ok(())
}

fn not_found(id: &str) -> MarsadError {
    MarsadError::NotFound {
        what: format!("workstream {id}"),
    }
}

struct Actor {
    store: Store,
    link: Supervised,
    local: HostId,
    inputs: mpsc::Sender<Input>,
    docs: BTreeMap<String, Workstream>,
    sessions: HashMap<NodeSession, SessionSnapshot>,
    tasks: HashMap<TaskId, TaskView>,
    /// The latest ended spawn requests per asking agent (oldest first, at most
    /// [`RECENT_SPAWNS`] each), as the asking agent's node published them.
    ended_spawns: HashMap<AgentId, Vec<SpawnEnded>>,
    sync: HashMap<(String, HostId), TopologySync>,
    launching: HashSet<(String, String)>,
    feeds: HashMap<HostId, Vec<JoinHandle<()>>>,
    /// Whether each host's link is up (the feeds of a host follow it; the local host is always up).
    links: HashMap<HostId, watch::Sender<bool>>,
    last: HashMap<String, WorkstreamView>,
    ring: Ring<WorkstreamsEvent>,
}

impl Workstreams {
    /// Load the stored documents, start the actor and its feeds, and push every topology in
    /// force (remote hosts get it again when their link connects).
    pub async fn start(
        store: Store,
        link: Supervised,
        fleet: Fleet,
        local: HostId,
    ) -> Result<Workstreams, MarsadError> {
        let docs: BTreeMap<String, Workstream> = store
            .workstreams()
            .await?
            .into_iter()
            .map(|w| (w.id.clone(), w))
            .collect();
        let (tx, rx) = mpsc::channel(MAILBOX);
        let mut actor = Actor {
            store,
            link,
            local,
            inputs: tx.clone(),
            docs,
            sessions: HashMap::new(),
            tasks: HashMap::new(),
            ended_spawns: HashMap::new(),
            sync: HashMap::new(),
            launching: HashSet::new(),
            feeds: HashMap::new(),
            links: HashMap::new(),
            last: HashMap::new(),
            ring: Ring::new(),
        };
        let ids: Vec<String> = actor.docs.keys().cloned().collect();
        for id in &ids {
            actor.push(id, None);
        }
        tokio::spawn(actor.run(rx));
        let ws = Workstreams { tx };
        ws.feed_fleet(fleet);
        Ok(ws)
    }

    async fn send(&self, i: Input) -> Result<(), MarsadError> {
        self.tx
            .send(i)
            .await
            .map_err(|_| MarsadError::io("the workstream actor is gone"))
    }

    async fn ask<T>(&self, f: impl FnOnce(Reply<T>) -> Input) -> Result<T, MarsadError> {
        let (tx, rx) = oneshot::channel();
        self.send(f(tx)).await?;
        rx.await
            .map_err(|_| MarsadError::io("the workstream actor is gone"))?
    }

    pub async fn list(&self) -> Result<Vec<Workstream>, MarsadError> {
        let (tx, rx) = oneshot::channel();
        self.send(Input::List(tx)).await?;
        rx.await
            .map_err(|_| MarsadError::io("the workstream actor is gone"))
    }

    pub async fn get(&self, id: &str) -> Result<Workstream, MarsadError> {
        let id = id.to_string();
        self.ask(|r| Input::Get(id, r)).await
    }

    /// Store a new document (id taken from `ws.id`, or derived from the name when empty).
    pub async fn create(&self, ws: Workstream) -> Result<Workstream, MarsadError> {
        self.ask(|r| Input::Create(Box::new(ws), r)).await
    }

    /// Apply `f` to a copy of the document and commit it (see the module docs).
    pub async fn edit(
        &self,
        id: &str,
        expected_revision: Option<u64>,
        f: EditFn,
    ) -> Result<Workstream, MarsadError> {
        let id = id.to_string();
        self.ask(|r| Input::Edit(id, expected_revision, f, r)).await
    }

    pub async fn delete(&self, id: &str) -> Result<(), MarsadError> {
        let id = id.to_string();
        self.ask(|r| Input::Delete(id, r)).await
    }

    /// Mark the unlaunched nodes as launching (a draft becomes running) and return them.
    pub async fn begin_launch(
        &self,
        id: &str,
    ) -> Result<(Workstream, Vec<(String, NodeLaunch)>), MarsadError> {
        let id = id.to_string();
        self.ask(|r| Input::BeginLaunch(id, r)).await
    }

    /// Record what the launches produced (and push the new topology).
    pub async fn end_launch(
        &self,
        id: &str,
        results: Vec<LaunchResult>,
    ) -> Result<Workstream, MarsadError> {
        let id = id.to_string();
        self.ask(|r| Input::EndLaunch(id, results, r)).await
    }

    /// The latest snapshot of a session, as the fleet reported it.
    pub async fn snapshot(&self, s: NodeSession) -> Option<SessionSnapshot> {
        let (tx, rx) = oneshot::channel();
        self.send(Input::Snapshot(s, tx)).await.ok()?;
        rx.await.ok().flatten()
    }

    /// The `workstreams` topic.
    pub async fn subscribe(&self, from: Option<Cursor>) -> Result<EventStream, RpcError> {
        let (tx, rx) = oneshot::channel();
        self.send(Input::Subscribe(from.map(|c| c.0), tx))
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let joined = rx
            .await
            .map_err(|_| RpcError::internal("the workstream actor is gone"))?;
        Ok(ring::stream(joined))
    }

    /// Follow the fleet (forever; resumes from its cursor after a lag).
    fn feed_fleet(&self, fleet: Fleet) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut last: Option<Cursor> = None;
            let mut resume = Resubscribe::default();
            loop {
                let mut lagged = false;
                match fleet.subscribe(last.clone()).await {
                    Ok(mut s) => {
                        while let Some(item) = s.next().await {
                            let (c, v) = match item {
                                Ok(x) => x,
                                Err(e) => {
                                    lagged = matches!(e, CloseReason::Lagged);
                                    break;
                                }
                            };
                            resume.delivered();
                            last = Some(c);
                            match serde_json::from_value::<FleetEvent>(v) {
                                Ok(e) => {
                                    if tx.send(Input::Fleet(e)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => hub_events::fleet_event_undecodable(&e),
                            }
                        }
                    }
                    Err(e) => hub_events::fleet_follow_failed(&e),
                }
                resume.after_end(lagged).await;
            }
        });
    }
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::Receiver<Input>) {
        while let Some(i) = rx.recv().await {
            self.apply(i).await;
        }
    }

    async fn apply(&mut self, input: Input) {
        match input {
            Input::List(r) => {
                // The asker may have gone away.
                let _ = r.send(self.docs.values().cloned().collect());
            }
            Input::Get(id, r) => {
                let _ = r.send(self.docs.get(&id).cloned().ok_or_else(|| not_found(&id)));
            }
            Input::Create(ws, r) => {
                let res = self.create(*ws).await;
                let _ = r.send(res);
            }
            Input::Edit(id, expected, f, r) => {
                let res = self.edit(&id, expected, f).await;
                let _ = r.send(res);
            }
            Input::Delete(id, r) => {
                let res = self.delete(&id).await;
                let _ = r.send(res);
            }
            Input::BeginLaunch(id, r) => {
                let res = self.begin_launch(&id).await;
                let _ = r.send(res);
            }
            Input::EndLaunch(id, results, r) => {
                let res = self.end_launch(&id, results).await;
                let _ = r.send(res);
            }
            Input::Snapshot(s, r) => {
                let _ = r.send(self.sessions.get(&s).cloned());
            }
            Input::Subscribe(from, r) => {
                let views: Vec<WorkstreamView> = self.docs.values().map(|w| self.view(w)).collect();
                let joined = self.ring.join(from.as_deref(), || WorkstreamsEvent::Reset {
                    workstreams: views,
                });
                let _ = r.send(joined);
            }
            Input::Fleet(e) => self.on_fleet(e),
            Input::Task(e) => {
                if e.side == Side::Outbound {
                    let ws = e.view.task.workstream.clone();
                    self.tasks.insert(e.view.task.id.clone(), e.view);
                    self.refresh(&ws);
                }
            }
            Input::Edge(e) => self.on_edge(e).await,
            Input::Spawn(e) => self.on_spawn_ended(e),
            Input::Pushed {
                ws,
                host,
                version,
                result,
            } => {
                let entry = self
                    .sync
                    .entry((ws.clone(), host.clone()))
                    .or_insert(TopologySync {
                        host,
                        applied: 0,
                        error: None,
                    });
                match result {
                    Ok(()) => {
                        entry.applied = entry.applied.max(version);
                        entry.error = None;
                    }
                    Err(e) => entry.error = Some(e),
                }
                self.refresh(&ws);
            }
        }
    }

    // ── documents ───────────────────────────────────────────────────────────

    async fn create(&mut self, mut ws: Workstream) -> Result<Workstream, MarsadError> {
        if ws.id.is_empty() {
            let base = slug(&ws.name);
            let mut id = base.clone();
            let mut n = 2;
            while self.docs.contains_key(&id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            ws.id = id;
        } else if self.docs.contains_key(&ws.id) {
            return Err(MarsadError::invalid(format!(
                "workstream id {} is taken",
                ws.id
            )));
        } else if !valid_id(&ws.id) {
            return Err(MarsadError::invalid(format!(
                "workstream id {:?} must be 1-64 characters of a-z, 0-9, - and _",
                ws.id
            )));
        }
        ws.schema = marsad_proto::workstream::SCHEMA;
        ws.revision = 0;
        ws.topology_version = 0;
        ws.topology_hosts.clear();
        ws.created_at_ms = now_ms();
        validate(&ws)?;
        self.commit(None, ws).await
    }

    async fn edit(
        &mut self,
        id: &str,
        expected: Option<u64>,
        f: EditFn,
    ) -> Result<Workstream, MarsadError> {
        let cur = self.docs.get(id).cloned().ok_or_else(|| not_found(id))?;
        if let Some(e) = expected {
            if e != cur.revision {
                return Err(MarsadError::Conflict {
                    message: format!("workstream {id} is at revision {}, not {e}", cur.revision),
                });
            }
        }
        let mut next = cur.clone();
        f(&mut next)?;
        // What only the hub sets.
        next.id = cur.id.clone();
        next.schema = cur.schema;
        next.revision = cur.revision;
        next.topology_version = cur.topology_version;
        next.topology_hosts = cur.topology_hosts.clone();
        next.created_at_ms = cur.created_at_ms;
        validate(&next)?;
        self.commit(Some(cur), next).await
    }

    /// Persist `next` (bumping revision, and the topology version when the topology
    /// changed), sync node layers, push and publish.
    async fn commit(
        &mut self,
        old: Option<Workstream>,
        mut next: Workstream,
    ) -> Result<Workstream, MarsadError> {
        let mut key = topology(&next);
        key.version = 0;
        let changed = match &old {
            None => !key.peers.is_empty(),
            Some(o) => {
                let mut before = topology(o);
                before.version = 0;
                before != key
            }
        };
        if changed {
            next.topology_version += 1;
            for h in involved(&next) {
                if !next.topology_hosts.contains(&h) {
                    next.topology_hosts.push(h);
                }
            }
        }
        next.revision += 1;
        next.updated_at_ms = now_ms();
        self.store.put_workstream(next.clone()).await?;
        self.sync_layers(old.as_ref(), Some(&next)).await?;
        let id = next.id.clone();
        self.docs.insert(id.clone(), next.clone());
        if changed {
            self.push(&id, None);
        }
        self.refresh(&id);
        Ok(next)
    }

    /// Store each node's override layer as its `workstream_node` layer (removing those of
    /// nodes that went away).
    async fn sync_layers(
        &self,
        old: Option<&Workstream>,
        new: Option<&Workstream>,
    ) -> Result<(), MarsadError> {
        use marsad_proto::agentconfig::LayerScope;
        let layer = |w: Option<&Workstream>, node: &str| {
            w.and_then(|w| w.node(node))
                .and_then(|n| n.launch.as_ref())
                .map(|l| l.overrides.clone())
                .unwrap_or_default()
        };
        let mut nodes: BTreeSet<(String, String)> = BTreeSet::new();
        for w in old.iter().chain(new.iter()) {
            for n in &w.nodes {
                nodes.insert((w.id.clone(), n.id.clone()));
            }
        }
        for (ws, node) in nodes {
            let after = layer(new, &node);
            if layer(old, &node) != after {
                self.store
                    .set_layer(
                        LayerScope::WorkstreamNode {
                            workstream: ws,
                            node,
                        },
                        after,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn delete(&mut self, id: &str) -> Result<(), MarsadError> {
        let cur = self.docs.get(id).cloned().ok_or_else(|| not_found(id))?;
        self.store.delete_workstream(id.to_string()).await?;
        self.sync_layers(Some(&cur), None).await?;
        self.docs.remove(id);
        self.last.remove(id);
        self.launching.retain(|(w, _)| w != id);
        self.sync.retain(|(w, _), _| w != id);
        // Take the topology down everywhere it was pushed (a newer, empty version).
        if cur.topology_version > 0 {
            let removal = Topology {
                workstream: cur.id.clone(),
                version: cur.topology_version + 1,
                peers: Vec::new(),
                edges: Vec::new(),
                limits: cur.limits.clone(),
                paused: false,
            };
            for h in &cur.topology_hosts {
                self.spawn_push(removal.clone(), h.clone());
            }
        }
        self.ring
            .emit(WorkstreamsEvent::Removed { id: id.to_string() });
        Ok(())
    }

    async fn begin_launch(
        &mut self,
        id: &str,
    ) -> Result<(Workstream, Vec<(String, NodeLaunch)>), MarsadError> {
        let cur = self.docs.get(id).cloned().ok_or_else(|| not_found(id))?;
        if cur.state == WorkstreamState::Archived {
            return Err(MarsadError::invalid(format!("workstream {id} is archived")));
        }
        let todo: Vec<(String, NodeLaunch)> = cur
            .nodes
            .iter()
            .filter(|n| n.session.is_none())
            .filter(|n| !self.launching.contains(&(id.to_string(), n.id.clone())))
            .filter_map(|n| n.launch.clone().map(|l| (n.id.clone(), l)))
            .collect();
        for (n, _) in &todo {
            self.launching.insert((id.to_string(), n.clone()));
        }
        let ws = if cur.state == WorkstreamState::Draft {
            let mut next = cur.clone();
            next.state = WorkstreamState::Running;
            match self.commit(Some(cur), next).await {
                Ok(w) => w,
                Err(e) => {
                    for (n, _) in &todo {
                        self.launching.remove(&(id.to_string(), n.clone()));
                    }
                    return Err(e);
                }
            }
        } else {
            self.refresh(id);
            cur
        };
        Ok((ws, todo))
    }

    async fn end_launch(
        &mut self,
        id: &str,
        results: Vec<LaunchResult>,
    ) -> Result<Workstream, MarsadError> {
        for (n, _) in &results {
            self.launching.remove(&(id.to_string(), n.clone()));
        }
        let ok: Vec<(String, NodeSession)> = results
            .into_iter()
            .filter_map(|(n, r)| r.ok().map(|s| (n, s)))
            .collect();
        self.edit(
            id,
            None,
            Box::new(move |w| {
                for (n, s) in ok {
                    if let Some(node) = w.nodes.iter_mut().find(|x| x.id == n) {
                        if node.session.is_none() {
                            node.session = Some(s);
                        }
                    }
                }
                Ok(())
            }),
        )
        .await
    }

    // ── topology pushes ─────────────────────────────────────────────────────

    /// Push the document's topology to its hosts (or only to `only`).
    fn push(&mut self, id: &str, only: Option<&HostId>) {
        let Some(ws) = self.docs.get(id) else { return };
        if ws.topology_version == 0 {
            return;
        }
        let t = topology(ws);
        let hosts: Vec<HostId> = ws
            .topology_hosts
            .iter()
            .filter(|h| only.is_none_or(|o| o == *h))
            .cloned()
            .collect();
        for h in hosts {
            self.spawn_push(t.clone(), h);
        }
    }

    fn spawn_push(&self, t: Topology, host: HostId) {
        let link = self.link.clone();
        let tx = self.inputs.clone();
        tokio::spawn(async move {
            let result = push_topology(&link, &host, &t).await;
            if let Err(e) = &result {
                hub_events::topology_push_failed(&t.workstream, &host, e);
            }
            // The actor may be gone (shutdown).
            let _ = tx
                .send(Input::Pushed {
                    ws: t.workstream.clone(),
                    host,
                    version: t.version,
                    result,
                })
                .await;
        });
    }

    // ── feeds ───────────────────────────────────────────────────────────────

    fn on_fleet(&mut self, e: FleetEvent) {
        let mut touched_hosts: Vec<HostId> = Vec::new();
        match e {
            FleetEvent::Reset { hosts } => {
                self.sessions.clear();
                let ids: HashSet<HostId> = hosts.iter().map(|h| h.host.id.clone()).collect();
                let gone: Vec<HostId> = self
                    .feeds
                    .keys()
                    .filter(|h| !ids.contains(*h))
                    .cloned()
                    .collect();
                for h in gone {
                    self.unfeed(&h);
                }
                for h in hosts {
                    self.set_link(&h.host.id, link_is_up(&h.link));
                    self.ensure_feeds(&h.host.id);
                    for s in h.sessions {
                        self.sessions.insert(
                            NodeSession {
                                host: h.host.id.clone(),
                                session: s.id.clone(),
                            },
                            s,
                        );
                    }
                }
                let all: Vec<String> = self.docs.keys().cloned().collect();
                for id in all {
                    self.refresh(&id);
                }
                return;
            }
            FleetEvent::HostUpserted { host } => self.ensure_feeds(&host.id),
            FleetEvent::HostRemoved { id } => {
                self.unfeed(&id);
                self.links.remove(&id);
                self.sessions.retain(|k, _| k.host != id);
                touched_hosts.push(id);
            }
            FleetEvent::Link { host, state } => {
                self.set_link(&host, state.is_connected());
                if state.is_connected() {
                    let ids: Vec<String> = self
                        .docs
                        .values()
                        .filter(|w| w.topology_hosts.contains(&host))
                        .map(|w| w.id.clone())
                        .collect();
                    for id in ids {
                        self.push(&id, Some(&host));
                    }
                }
            }
            FleetEvent::Sessions { host, sessions } => {
                self.sessions.retain(|k, _| k.host != host);
                for s in sessions {
                    self.sessions.insert(
                        NodeSession {
                            host: host.clone(),
                            session: s.id.clone(),
                        },
                        s,
                    );
                }
                touched_hosts.push(host);
            }
            FleetEvent::SessionUpserted { host, session } => {
                let key = NodeSession {
                    host: host.clone(),
                    session: session.id.clone(),
                };
                self.sessions.insert(key.clone(), *session);
                self.refresh_session(&key);
                return;
            }
            FleetEvent::SessionRemoved { host, id } => {
                let key = NodeSession { host, session: id };
                self.sessions.remove(&key);
                self.refresh_session(&key);
                return;
            }
            FleetEvent::AuthPrompt { .. }
            | FleetEvent::Git { .. }
            | FleetEvent::FeedError { .. } => return,
        }
        for h in touched_hosts {
            let ids: Vec<String> = self
                .docs
                .values()
                .filter(|w| {
                    w.nodes
                        .iter()
                        .any(|n| n.session.as_ref().is_some_and(|s| s.host == h))
                })
                .map(|w| w.id.clone())
                .collect();
            for id in ids {
                self.refresh(&id);
            }
        }
    }

    fn refresh_session(&mut self, key: &NodeSession) {
        let ids: Vec<String> = self
            .docs
            .values()
            .filter(|w| w.nodes.iter().any(|n| n.session.as_ref() == Some(key)))
            .map(|w| w.id.clone())
            .collect();
        for id in ids {
            self.refresh(&id);
        }
    }

    /// Whether `host`'s link is up, as a watch its feeds follow. A host the actor has not heard of
    /// is down until its link connects (the local host is always up).
    fn link_sender(&mut self, host: &HostId) -> &watch::Sender<bool> {
        let up = *host == self.local;
        self.links
            .entry(host.clone())
            .or_insert_with(|| watch::channel(up).0)
    }

    /// Record the link state of `host`. Only a change is announced to the feeds: a repeated state
    /// must not make them subscribe again.
    fn set_link(&mut self, host: &HostId, up: bool) {
        self.link_sender(host).send_if_modified(|v| {
            let changed = *v != up;
            *v = up;
            changed
        });
    }

    fn ensure_feeds(&mut self, host: &HostId) {
        if self.feeds.contains_key(host) {
            return;
        }
        let up = self.link_sender(host).subscribe();
        let handles = vec![
            self.spawn_feed(host.clone(), wasl_topics::TASKS, up.clone()),
            self.spawn_feed(host.clone(), wasl_topics::EDGES, up.clone()),
            self.spawn_feed(host.clone(), wasl_topics::SPAWNS, up),
        ];
        self.feeds.insert(host.clone(), handles);
    }

    fn unfeed(&mut self, host: &HostId) {
        if let Some(handles) = self.feeds.remove(host) {
            for h in handles {
                h.abort();
            }
        }
    }

    /// Follow a host node's `wasl.tasks` / `wasl.edges` / `wasl.spawns` over the loopback link,
    /// for as long as the host is known. Subscribes only while the host's link is up; a refusal
    /// is logged and the feed is subscribed again when the link next connects.
    fn spawn_feed(
        &self,
        host: HostId,
        topic: &'static str,
        up: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let mut follow = Follow::new(
            self.link.clone(),
            topic,
            Some(host.clone()),
            up,
            |up: &bool| *up,
            false,
        );
        let tx = self.inputs.clone();
        tokio::spawn(async move {
            while let Some(step) = follow.next().await {
                let v = match step {
                    Step::Event(v) => v,
                    Step::Refused(e) => {
                        hub_events::node_feed_ended(&host, topic, &e);
                        continue;
                    }
                };
                let input = match topic {
                    wasl_topics::TASKS => serde_json::from_value::<TaskEvent>(v).map(Input::Task),
                    wasl_topics::SPAWNS => {
                        serde_json::from_value::<SpawnEnded>(v).map(Input::Spawn)
                    }
                    _ => serde_json::from_value::<EdgeEvent>(v).map(Input::Edge),
                };
                match input {
                    Ok(i) => {
                        if tx.send(i).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => hub_events::node_event_undecodable(&host, topic, &e),
                }
            }
        })
    }

    /// A node published an ended spawn request: keep it for the workstreams its asker is a node
    /// of. A request that is already kept (a feed that resumed) is not kept twice.
    fn on_spawn_ended(&mut self, e: SpawnEnded) {
        let asker = e.from.clone();
        let list = self.ended_spawns.entry(asker.clone()).or_default();
        if list.iter().any(|x| x.request_id == e.request_id) {
            return;
        }
        list.push(e);
        list.sort_by_key(|x| (x.ended_ms, x.request_id.clone()));
        if list.len() > RECENT_SPAWNS {
            list.remove(0);
        }
        let ids: Vec<String> = self
            .docs
            .values()
            .filter(|w| {
                w.nodes.iter().any(|n| {
                    n.session.as_ref().is_some_and(|s| {
                        s.host.as_str() == asker.host && s.session.as_str() == asker.session
                    })
                })
            })
            .map(|w| w.id.clone())
            .collect();
        for id in ids {
            self.refresh(&id);
        }
    }

    async fn on_edge(&mut self, e: EdgeEvent) {
        if e.cause != EdgeChangeCause::Decision || e.policy != EdgePolicy::AlwaysAllow {
            return;
        }
        let Some(ws) = self.docs.get(&e.workstream) else {
            return;
        };
        if ws.topology_version != e.version {
            return;
        }
        let node_of = |a: &AgentId| {
            ws.nodes
                .iter()
                .find(|n| {
                    n.session.as_ref().is_some_and(|s| {
                        s.host.as_str() == a.host && s.session.as_str() == a.session
                    })
                })
                .map(|n| n.id.clone())
        };
        let (Some(from), Some(to)) = (node_of(&e.from), node_of(&e.to)) else {
            return;
        };
        if ws
            .edge(&from, &to)
            .is_none_or(|x| x.policy == EdgePolicy::AlwaysAllow)
        {
            return;
        }
        let id = ws.id.clone();
        let res = self
            .edit(
                &id,
                None,
                Box::new(move |w| {
                    for x in w.edges.iter_mut().filter(|x| x.from == from && x.to == to) {
                        x.policy = EdgePolicy::AlwaysAllow;
                    }
                    Ok(())
                }),
            )
            .await;
        if let Err(err) = res {
            hub_events::always_allow_failed(&id, &err);
        }
    }

    // ── views ───────────────────────────────────────────────────────────────

    fn refresh(&mut self, id: &str) {
        let Some(ws) = self.docs.get(id) else { return };
        let v = self.view(ws);
        if self.last.get(id) == Some(&v) {
            return;
        }
        self.last.insert(id.to_string(), v.clone());
        self.ring.emit(WorkstreamsEvent::Upserted {
            workstream: Box::new(v),
        });
    }

    fn view(&self, ws: &Workstream) -> WorkstreamView {
        let mut nodes = Vec::new();
        let mut cost = WorkstreamCost::default();
        let mut agents: HashMap<String, (AgentId, NodeSession)> = HashMap::new();
        for n in &ws.nodes {
            let host = n
                .session
                .as_ref()
                .map(|s| s.host.clone())
                .or_else(|| n.launch.as_ref().and_then(|l| l.host.clone()))
                .unwrap_or_else(|| self.local.clone());
            let snap = n
                .session
                .as_ref()
                .and_then(|s| self.sessions.get(s))
                .cloned();
            if let Some(s) = &snap {
                cost.sessions_usd += s.usage.cost_usd;
                cost.by_node.insert(n.id.clone(), s.usage.cost_usd);
            }
            if let Some(s) = &n.session {
                agents.insert(
                    n.id.clone(),
                    (AgentId::new(s.host.as_str(), s.session.as_str()), s.clone()),
                );
            }
            nodes.push(NodeView {
                node: n.id.clone(),
                host,
                session: snap.map(Box::new),
                launching: self.launching.contains(&(ws.id.clone(), n.id.clone())),
            });
        }
        let mut edges = Vec::new();
        for e in &ws.edges {
            let mut ev = EdgeView {
                from: e.from.clone(),
                to: e.to.clone(),
                policy: e.policy,
                in_flight: Vec::new(),
                pending_gates: Vec::new(),
                completed: 0,
                unsuccessful: 0,
                cost_usd: 0.0,
            };
            if let (Some((fa, fs)), Some((ta, _))) = (agents.get(&e.from), agents.get(&e.to)) {
                let mut tasks: Vec<&TaskView> = self
                    .tasks
                    .values()
                    .filter(|t| {
                        t.task.workstream == ws.id && t.task.from == *fa && t.task.to == *ta
                    })
                    .collect();
                tasks.sort_by_key(|t| (t.task.created_ms, t.task.id.clone()));
                for t in tasks {
                    match &t.state {
                        TaskState::Completed { result } => {
                            ev.completed += 1;
                            let c = result.usage.as_ref().map(|u| u.cost_usd).unwrap_or(0.0);
                            ev.cost_usd += c;
                            cost.tasks_usd += c;
                        }
                        s if s.is_terminal() => ev.unsuccessful += 1,
                        _ => ev.in_flight.push(t.clone()),
                    }
                }
                if let Some(snap) = self.sessions.get(fs) {
                    for g in &snap.pending_gates {
                        if let GateRequest::Handoff {
                            task_id, details, ..
                        } = &g.gate
                        {
                            let to = details.get("to_agent").and_then(|v| v.as_str());
                            let w = details.get("workstream").and_then(|v| v.as_str());
                            if w == Some(ws.id.as_str()) && to == Some(ta.to_string().as_str()) {
                                ev.pending_gates.push(HandoffGateView {
                                    host: fs.host.clone(),
                                    session: fs.session.clone(),
                                    task: task_id.clone(),
                                    raised_at_ms: g.raised_at_ms,
                                    gate: g.gate.clone(),
                                });
                            }
                        }
                    }
                }
            }
            edges.push(ev);
        }
        let sync = ws
            .topology_hosts
            .iter()
            .map(|h| {
                self.sync
                    .get(&(ws.id.clone(), h.clone()))
                    .cloned()
                    .unwrap_or(TopologySync {
                        host: h.clone(),
                        applied: 0,
                        error: None,
                    })
            })
            .collect();
        let mut pending_spawns = Vec::new();
        for n in &ws.nodes {
            let Some(s) = &n.session else { continue };
            let Some(snap) = self.sessions.get(s) else {
                continue;
            };
            let asker = AgentId::new(s.host.as_str(), s.session.as_str());
            let ended = self.ended_spawns.get(&asker);
            for g in &snap.pending_gates {
                // The two feeds (the session's snapshot, the node's endings) are not in step:
                // a request its node says ended is not pending, whatever the snapshot shows yet.
                let over = ended.is_some_and(|l| {
                    l.iter()
                        .any(|e| spawn_request_id(&e.request_id) == g.gate.request_id())
                });
                if matches!(g.gate, GateRequest::Spawn { .. }) && !over {
                    pending_spawns.push(SpawnGateView {
                        node: n.id.clone(),
                        host: s.host.clone(),
                        session: s.session.clone(),
                        raised_at_ms: g.raised_at_ms,
                        gate: g.gate.clone(),
                    });
                }
            }
        }
        let mut recent_spawns = Vec::new();
        for n in &ws.nodes {
            let Some(s) = &n.session else { continue };
            let asker = AgentId::new(s.host.as_str(), s.session.as_str());
            for e in self.ended_spawns.get(&asker).into_iter().flatten() {
                recent_spawns.push(SpawnOutcomeView {
                    request_id: e.request_id.clone(),
                    node: n.id.clone(),
                    host: s.host.clone(),
                    session: s.session.clone(),
                    name: e.name.clone(),
                    ended_ms: e.ended_ms,
                    end: e.end.clone(),
                    detail: e.detail.clone(),
                });
            }
        }
        recent_spawns.sort_by(|a, b| (b.ended_ms, &b.request_id).cmp(&(a.ended_ms, &a.request_id)));
        recent_spawns.truncate(RECENT_SPAWNS);
        WorkstreamView {
            workstream: ws.clone(),
            nodes,
            edges,
            sync,
            cost,
            pending_spawns,
            recent_spawns,
        }
    }
}

/// Push `t` to `host`'s node, once the host's link is connected (see [`push_when_connected`]).
pub async fn push_topology(link: &Supervised, host: &HostId, t: &Topology) -> Result<(), String> {
    push_when_connected(
        link.state(),
        move || async move {
            let opts = RequestOpts::new(PUSH_DEADLINE).host(host.clone());
            link.request::<TopologyApplied>(wasl_methods::TOPOLOGY_SET, t, opts, PUSH_WAIT)
                .await
                .map(|_| ())
        },
        |e| hub_events::topology_push_retry(host, e),
    )
    .await
}

/// Runs `send` once the link is connected, and again after each retryable failure once it
/// connects again. What is waited for is the link's own state (a change event, no timer): the
/// link's supervisor decides when it reconnects, and a push that failed while connected waits for
/// the next change of that state rather than for a guessed delay. A failure that is not retryable
/// ends the push with its text.
async fn push_when_connected<Fut>(
    mut state: watch::Receiver<LinkState>,
    mut send: impl FnMut() -> Fut,
    mut on_retry: impl FnMut(&CallError),
) -> Result<(), String>
where
    Fut: Future<Output = Result<(), CallError>>,
{
    loop {
        if state.wait_for(|s| s.is_connected()).await.is_err() {
            return Err("the host's link is gone".into());
        }
        match send().await {
            Ok(()) => return Ok(()),
            Err(e) if e.is_retryable() => {
                on_retry(&e);
                if state.changed().await.is_err() {
                    return Err(e.to_string());
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use marsad_proto::workstream::{WorkstreamEdge, WorkstreamNode};
    use std::sync::{Arc, Mutex};

    fn node(id: &str, session: Option<(&str, &str)>) -> WorkstreamNode {
        WorkstreamNode {
            id: id.into(),
            name: id.to_uppercase(),
            role: String::new(),
            description: String::new(),
            session: session.map(|(h, s)| NodeSession {
                host: HostId::new(h),
                session: marsad_proto::SessionId::parse(s).unwrap(),
            }),
            launch: Some(NodeLaunch::new("claude")),
            spawned_by: None,
            spawn_request: None,
        }
    }

    #[test]
    fn topology_follows_state_sessions_and_policies() {
        let mut ws = Workstream::new("ws", "W");
        ws.nodes = vec![
            node("o", Some(("hub", "aaaaaaaaaaaa"))),
            node("a", Some(("box", "bbbbbbbbbbbb"))),
            node("b", None),
        ];
        ws.edges = vec![
            WorkstreamEdge {
                from: "o".into(),
                to: "a".into(),
                policy: EdgePolicy::Ask,
            },
            WorkstreamEdge {
                from: "o".into(),
                to: "b".into(),
                policy: EdgePolicy::Deny,
            },
        ];
        validate(&ws).unwrap();
        assert!(topology(&ws).peers.is_empty(), "a draft pushes nothing");
        ws.state = WorkstreamState::Running;
        let t = topology(&ws);
        assert_eq!(t.peers.len(), 2);
        assert_eq!(t.edges.len(), 1, "edges to unlaunched nodes wait");
        assert_eq!(t.edges[0].to, AgentId::new("box", "bbbbbbbbbbbb"));
        ws.state = WorkstreamState::Paused;
        assert!(topology(&ws).paused);
        assert_eq!(
            involved(&ws).into_iter().collect::<Vec<_>>(),
            vec![HostId::new("box"), HostId::new("hub")]
        );
    }

    #[test]
    fn validation_rejects_broken_graphs() {
        let mut ws = Workstream::new("ws", "W");
        ws.nodes = vec![node("o", None), node("o", None)];
        assert!(validate(&ws).is_err());
        ws.nodes = vec![node("o", None), node("x", None)];
        ws.edges = vec![WorkstreamEdge {
            from: "o".into(),
            to: "nope".into(),
            policy: EdgePolicy::Ask,
        }];
        assert!(validate(&ws).is_err());
        ws.edges[0].to = "o".into();
        assert!(validate(&ws).is_err());
        ws.edges[0].to = "x".into();
        validate(&ws).unwrap();
        ws.nodes[1].launch = None;
        assert!(validate(&ws).is_err());
        assert_eq!(slug("  My Review Team! "), "my-review-team");
    }

    fn connected() -> LinkState {
        LinkState::Connected {
            hello: wasit_control::Hello::new("test-build", "test", "test"),
        }
    }

    fn reconnecting() -> LinkState {
        LinkState::Reconnecting {
            attempt: 1,
            next_at: std::time::SystemTime::now(),
            last_error: "dropped".into(),
        }
    }

    /// The push is not sent while the host is down: the first attempt waits for the connect.
    #[tokio::test]
    async fn topology_is_pushed_on_connect() {
        let (tx, rx) = watch::channel(LinkState::Connecting);
        let sends = Arc::new(Mutex::new(0usize));
        let counted = sends.clone();
        let push = tokio::spawn(push_when_connected(
            rx,
            move || {
                *counted.lock().unwrap() += 1;
                async { Ok(()) }
            },
            |_| {},
        ));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            *sends.lock().unwrap(),
            0,
            "pushed before the host connected"
        );
        tx.send_replace(connected());
        let r = tokio::time::timeout(Duration::from_secs(5), push)
            .await
            .expect("the push ends once connected")
            .unwrap();
        assert_eq!(r, Ok(()));
        assert_eq!(*sends.lock().unwrap(), 1);
    }

    /// A retryable failure is not retried on a timer: the next attempt waits for a change of the
    /// link state, and comes after the link connects again.
    #[tokio::test]
    async fn a_retryable_failure_waits_for_the_next_connect() {
        let (tx, rx) = watch::channel(connected());
        let sends = Arc::new(Mutex::new(0usize));
        let counted = sends.clone();
        let push = tokio::spawn(push_when_connected(
            rx,
            move || {
                let mut n = counted.lock().unwrap();
                *n += 1;
                let first = *n == 1;
                async move {
                    if first {
                        Err(CallError::Timeout)
                    } else {
                        Ok(())
                    }
                }
            },
            |_| {},
        ));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            *sends.lock().unwrap(),
            1,
            "retried without a change of the link"
        );
        tx.send_replace(reconnecting());
        tx.send_replace(connected());
        let r = tokio::time::timeout(Duration::from_secs(5), push)
            .await
            .expect("the push ends after the reconnect")
            .unwrap();
        assert_eq!(r, Ok(()));
        assert_eq!(*sends.lock().unwrap(), 2);
    }
}
