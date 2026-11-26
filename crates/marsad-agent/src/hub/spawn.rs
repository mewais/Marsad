//! The spawn authority (ARCHITECTURE §5 `spawn_agent`): agents start worker agents on demand.
//!
//! Wasl's node asks (see `wasl_proto::spawn`); the hub decides and launches. The hub's own
//! node gets a [`HubSpawner`], which turns the authority calls into the hub methods
//! `workstreams.spawn.*` over an in-process connection; other hosts' nodes forward them to the
//! hub's node. The hub
//!
//! * **plans** — finds the asking session's workstream (a running one it is a node of; none ⇒
//!   approving creates "<session name> + workers"), checks the workstream's [`SpawnSettings`]
//!   (policy `never`, max spawned agents, max depth, allowed hosts and drivers, budget),
//!   resolves host / driver / directory (defaults: the asking agent's) and says whether the
//!   human must approve (policy `ask`, or no workstream yet);
//! * **launches** — after the gate: applies "always allow" to the policy, adds the node
//!   (`spawned_by`, `spawn_request`, a session id chosen in advance) with a connection from
//!   the asking node (`always_allow` only if the human extended "always allow" to handoffs,
//!   else `ask`), then launches it through the normal workstream launch (effective config,
//!   automatic Wasl / ask_user items, topology pushed). Repeating a launch with the same request
//!   id returns the same agent: the node is found by its request id and a launch under its
//!   chosen session id finds the session if it already exists;
//! * **dismisses** — stops a worker for the agent that spawned it (no gate);
//! * reports what an agent may spawn (`list_peers` → `spawnable`).

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use marsad_proto::agentconfig::LaunchRequest;
use marsad_proto::workstream::{
    NodeLaunch, NodeSession, SpawnPolicy, SpawnSettings, Workstream, WorkstreamEdge,
    WorkstreamNode, WorkstreamState,
};
use marsad_proto::{methods, HostId, MarsadError, SessionId, SessionSnapshot, SpawnError};
use serde_json::json;
use wasit_control::{CallContext, RequestOpts, RpcError, Supervised};
use wasit_runtime::wire::{
    methods as rt_methods, topics as rt_topics, DriverInventory, InventoryReply, InventoryRequest,
    SessionRef,
};
use wasl_node::SpawnAuthority;
use wasl_proto::{
    AgentId, DismissAsk, DismissReply, EdgePolicy, SpawnApproval, SpawnAsk, SpawnDecision,
    SpawnDriver, SpawnHost, SpawnLaunch, SpawnLaunched, SpawnPlan, SpawnSpec, Spawnable,
    SpawnableAsk,
};

use super::workstreams;
use super::HubServices;
use crate::hub_events;

/// Bound on one authority call through the in-process connection (a launch includes the CLI's
/// start on the target host).
const CALL: Duration = Duration::from_secs(150);
/// Follows the host's inventory events until no driver is still being probed (or the stream
/// ends) and returns the latest reply. There is no timer: a host whose probe is slow keeps the
/// wait open, the same way the launch path waits for `agentconfig.base` to settle.
async fn settle_inventory<S, T, E>(mut reply: InventoryReply, mut events: S) -> InventoryReply
where
    S: futures::Stream<Item = Result<(T, serde_json::Value), E>> + Unpin,
{
    while let Some(Ok((_, v))) = events.next().await {
        let Ok(inv) = serde_json::from_value::<InventoryReply>(v) else {
            continue;
        };
        let settled = !inv.drivers.iter().any(unresolved);
        reply = inv;
        if settled {
            break;
        }
    }
    reply
}

/// A driver whose binary the host has not looked for yet (its first probe is still running).
fn unresolved(d: &DriverInventory) -> bool {
    !d.installed && d.error.is_none()
}

/// Models listed per driver in `spawnable`.
const MODELS: usize = 12;

/// The hub node's [`SpawnAuthority`]: the hub methods over an in-process connection.
pub struct HubSpawner {
    link: Supervised,
}

impl HubSpawner {
    pub fn new(link: Supervised) -> Self {
        HubSpawner { link }
    }

    async fn call<P: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        p: P,
    ) -> Result<T, RpcError> {
        self.link
            .request(method, p, RequestOpts::new(CALL), Duration::from_secs(10))
            .await
            .map_err(|e| e.into_rpc())
    }
}

#[async_trait]
impl SpawnAuthority for HubSpawner {
    async fn plan(&self, ask: SpawnAsk) -> Result<SpawnPlan, RpcError> {
        self.call(methods::WS_SPAWN_PLAN, ask).await
    }
    async fn launch(&self, launch: SpawnLaunch) -> Result<SpawnLaunched, RpcError> {
        self.call(methods::WS_SPAWN_LAUNCH, launch).await
    }
    async fn dismiss(&self, ask: DismissAsk) -> Result<DismissReply, RpcError> {
        self.call(methods::WS_SPAWN_DISMISS, ask).await
    }
    async fn spawnable(&self, ask: SpawnableAsk) -> Result<Spawnable, RpcError> {
        self.call(methods::WS_SPAWNABLE, ask).await
    }
}

fn refused(message: impl Into<String>) -> MarsadError {
    MarsadError::Spawn {
        code: SpawnError::Refused,
        message: message.into(),
    }
}

fn limit(message: impl Into<String>) -> MarsadError {
    MarsadError::Spawn {
        code: SpawnError::Limit,
        message: message.into(),
    }
}

/// The asking session and where it stands.
struct Parent {
    agent: AgentId,
    snap: SessionSnapshot,
    /// The running workstream it is a node of.
    ws: Option<Workstream>,
    node: Option<String>,
    /// Its name as people and peers see it.
    name: String,
}

impl Parent {
    fn settings(&self) -> SpawnSettings {
        self.ws
            .as_ref()
            .map(|w| w.spawn.clone())
            .unwrap_or_default()
    }

    fn ws_name(&self) -> String {
        match &self.ws {
            Some(w) => w.name.clone(),
            None => format!("{} + workers", self.name),
        }
    }
}

/// A request checked and resolved.
struct Resolved {
    parent: Parent,
    host: HostId,
    host_name: String,
    driver: DriverInventory,
    cwd: String,
    depth: u32,
}

/// What a session is called when it has no name of its own.
fn session_name(s: &SessionSnapshot) -> String {
    s.display_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        // The first prompt, shortened.
        .or_else(|| {
            s.title
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(|t| match t.char_indices().nth(40) {
                    Some((i, _)) => format!("{}…", &t[..i]),
                    None => t.to_string(),
                })
        })
        .or_else(|| {
            s.cwd
                .as_deref()
                .and_then(|c| std::path::Path::new(c).file_name())
                .map(|d| format!("{} in {}", s.driver, d.to_string_lossy()))
        })
        .unwrap_or_else(|| format!("{} session", s.driver))
}

/// A node id not used in `ws`, derived from `name`.
fn free_id(ws: &Workstream, name: &str) -> String {
    let base = workstreams::slug(name);
    if ws.node(&base).is_none() {
        return base;
    }
    (2..)
        .map(|i| format!("{base}-{i}"))
        .find(|id| ws.node(id).is_none())
        .unwrap_or(base)
}

impl HubServices {
    async fn parent(&self, ctx: &CallContext, agent: &AgentId) -> Result<Parent, MarsadError> {
        let host = HostId::new(agent.host.as_str());
        let session = SessionId::parse(&agent.session)
            .ok_or_else(|| MarsadError::invalid(format!("{agent} is not a session")))?;
        let snap: SessionSnapshot = self
            .on_host(
                ctx,
                &host,
                rt_methods::SESSION_SNAPSHOT,
                SessionRef {
                    id: session.clone(),
                },
            )
            .await?;
        let all = self.workstreams.list().await?;
        let ws = all.into_iter().find(|w| {
            matches!(w.state, WorkstreamState::Running | WorkstreamState::Paused)
                && w.node_of(&host, &session).is_some()
        });
        let node = ws
            .as_ref()
            .and_then(|w| w.node_of(&host, &session))
            .map(|n| (n.id.clone(), n.name.clone()));
        let name = node
            .as_ref()
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| session_name(&snap));
        Ok(Parent {
            agent: agent.clone(),
            snap,
            ws,
            node: node.map(|(id, _)| id),
            name,
        })
    }

    /// The drivers installed on `host`. A host that has not looked for a driver's binary yet
    /// (its first inventory starts that) is followed on its inventory topic until it has.
    async fn inventory(
        &self,
        ctx: &CallContext,
        host: &HostId,
    ) -> Result<Vec<DriverInventory>, MarsadError> {
        let mut r: InventoryReply = self
            .on_host(
                ctx,
                host,
                rt_methods::DRIVERS_INVENTORY,
                InventoryRequest { project: None },
            )
            .await?;
        if r.drivers.iter().any(unresolved) {
            let mut c = ctx.clone();
            c.host = Some(host.clone());
            let params = serde_json::to_value(InventoryRequest { project: None })
                .map_err(MarsadError::io)?;
            match self
                .routing
                .subscribe(c, rt_topics::DRIVERS_INVENTORY, params, None)
                .await
            {
                Ok(stream) => r = settle_inventory(r, stream).await,
                Err(e) => hub_events::inventory_unavailable(host, &e),
            }
        }
        Ok(r.drivers.into_iter().filter(|d| d.installed).collect())
    }

    fn host_connected(&self, host: &HostId) -> bool {
        *host == self.registry.local().id
            || self
                .registry
                .hub()
                .states()
                .borrow()
                .get(host)
                .is_some_and(|s| s.is_connected())
    }

    /// Check `spec` against the workstream's spawn settings and resolve its defaults.
    async fn resolve(
        &self,
        ctx: &CallContext,
        agent: &AgentId,
        spec: &SpawnSpec,
    ) -> Result<Resolved, MarsadError> {
        let parent = self.parent(ctx, agent).await?;
        let st = parent.settings();
        if let Some(w) = &parent.ws {
            if w.state == WorkstreamState::Paused {
                return Err(refused(format!(
                    "workstream {} is paused by the human; no new agents until it is resumed",
                    w.name
                )));
            }
            if st.policy == SpawnPolicy::Never {
                return Err(refused(format!(
                    "spawning agents is turned off in workstream {}; continue with the peers you have",
                    w.name
                )));
            }
            if w.spawned_count() >= st.max_agents {
                return Err(limit(format!(
                    "workstream {} already has {} spawned agents, the most it allows; dismiss one first or reuse a peer",
                    w.name,
                    w.spawned_count()
                )));
            }
            if let Some(b) = w.limits.budget_usd {
                let mut spent = 0.0;
                for n in w.nodes.iter().filter_map(|n| n.session.clone()) {
                    if let Some(s) = self.workstreams.snapshot(n).await {
                        spent += s.usage.cost_usd;
                    }
                }
                if spent >= b {
                    return Err(limit(format!(
                        "the workstream budget of ${b:.2} is used up (${spent:.2} spent)"
                    )));
                }
            }
        }
        let depth = match (&parent.ws, &parent.node) {
            (Some(w), Some(n)) => w.spawn_depth(n) + 1,
            _ => 1,
        };
        if depth > st.max_depth {
            return Err(limit(format!(
                "you were spawned yourself; agents may only spawn {} generation{} deep here",
                st.max_depth,
                if st.max_depth == 1 { "" } else { "s" }
            )));
        }
        let own_host = HostId::new(agent.host.as_str());
        let host = match spec.host.as_deref().map(str::trim) {
            None | Some("") => own_host.clone(),
            Some(h) => self
                .registry
                .list()
                .into_iter()
                .find(|e| e.id.as_str() == h || e.name.eq_ignore_ascii_case(h))
                .map(|e| e.id)
                .ok_or_else(|| refused(format!("there is no host {h:?}")))?,
        };
        if host != own_host && !st.allowed_hosts.contains(&host) {
            let mut ok: Vec<String> = vec![self.host_name(&own_host)];
            ok.extend(st.allowed_hosts.iter().map(|h| self.host_name(h)));
            return Err(refused(format!(
                "spawning on {} is not allowed in this workstream; allowed hosts: {}",
                self.host_name(&host),
                ok.join(", ")
            )));
        }
        if !self.host_connected(&host) {
            return Err(refused(format!(
                "host {} is not connected right now",
                self.host_name(&host)
            )));
        }
        let want = spec
            .driver
            .clone()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| parent.snap.driver.to_string());
        let installed = self.inventory(ctx, &host).await?;
        let Some(driver) = installed
            .iter()
            .find(|d| {
                d.driver.as_str() == want || d.display_name.eq_ignore_ascii_case(want.as_str())
            })
            .cloned()
        else {
            let names: Vec<&str> = installed.iter().map(|d| d.driver.as_str()).collect();
            return Err(refused(format!(
                "driver {want:?} is not installed on {}; installed: {}",
                self.host_name(&host),
                names.join(", ")
            )));
        };
        if !st.allowed_drivers.is_empty() && !st.allowed_drivers.contains(&driver.driver) {
            let names: Vec<&str> = st.allowed_drivers.iter().map(|d| d.as_str()).collect();
            return Err(refused(format!(
                "spawning {} agents is not allowed in this workstream; allowed: {}",
                driver.display_name,
                names.join(", ")
            )));
        }
        let cwd = match spec.cwd.clone().filter(|c| !c.trim().is_empty()) {
            Some(c) => c,
            None if host == own_host => parent.snap.cwd.clone().ok_or_else(|| {
                refused("your session has no working directory; give cwd explicitly")
            })?,
            None => {
                return Err(refused(format!(
                    "give cwd: on another host ({}) the working directory must be explicit",
                    self.host_name(&host)
                )))
            }
        };
        if let Some(w) = &parent.ws {
            if w.nodes
                .iter()
                .any(|n| n.name.eq_ignore_ascii_case(spec.name.trim()))
            {
                return Err(refused(format!(
                    "an agent named {:?} already exists in workstream {}; pick another name",
                    spec.name.trim(),
                    w.name
                )));
            }
        }
        let host_name = self.host_name(&host);
        Ok(Resolved {
            parent,
            host,
            host_name,
            driver,
            cwd,
            depth,
        })
    }

    fn host_name(&self, h: &HostId) -> String {
        self.registry
            .get(h)
            .map(|e| e.name)
            .unwrap_or_else(|| h.to_string())
    }

    /// `workstreams.spawn.plan`.
    pub(super) async fn spawn_plan(
        &self,
        ctx: &CallContext,
        ask: SpawnAsk,
    ) -> Result<SpawnPlan, MarsadError> {
        let r = self.resolve(ctx, &ask.parent, &ask.spec).await?;
        let st = r.parent.settings();
        let approval = match &r.parent.ws {
            Some(_) if st.policy == SpawnPolicy::AlwaysAllow => SpawnApproval::Allowed,
            _ => SpawnApproval::Ask,
        };
        Ok(SpawnPlan {
            approval,
            from: r.parent.name.clone(),
            name: ask.spec.name.trim().to_string(),
            role: ask.spec.role.trim().to_string(),
            host: r.host_name.clone(),
            driver: r.driver.display_name.clone(),
            model: ask.spec.model.clone(),
            cwd: r.cwd.clone(),
            mode: ask.spec.mode.clone(),
            workstream: r.parent.ws_name(),
            new_workstream: r.parent.ws.is_none(),
            details: json!({
                "parent": r.parent.agent,
                "host_id": r.host,
                "driver_id": r.driver.driver,
                "workstream_id": r.parent.ws.as_ref().map(|w| w.id.clone()),
                "depth": r.depth,
                "max_depth": st.max_depth,
                "spawned": r.parent.ws.as_ref().map(|w| w.spawned_count()).unwrap_or(0),
                "max_agents": st.max_agents,
            }),
        })
    }

    /// The node created by `request_id`, if any: (workstream, node).
    async fn spawned_node(
        &self,
        request_id: &str,
    ) -> Result<Option<(Workstream, WorkstreamNode)>, MarsadError> {
        Ok(self.workstreams.list().await?.into_iter().find_map(|w| {
            let n = w
                .nodes
                .iter()
                .find(|n| n.spawn_request.as_deref() == Some(request_id))
                .cloned();
            n.map(|n| (w, n))
        }))
    }

    fn launched(ws: &Workstream, node: &WorkstreamNode) -> Option<SpawnLaunched> {
        let s = node.session.as_ref()?;
        Some(SpawnLaunched {
            agent: AgentId::new(s.host.as_str(), s.session.as_str()),
            name: node.name.clone(),
            workstream: ws.id.clone(),
            topology: workstreams::topology(ws),
        })
    }

    /// `workstreams.spawn.launch` (idempotent by request id).
    pub(super) async fn spawn_launch(
        &self,
        ctx: &CallContext,
        l: SpawnLaunch,
    ) -> Result<SpawnLaunched, MarsadError> {
        if matches!(l.decision, SpawnDecision::Deny { .. }) {
            return Err(refused("the human denied this spawn"));
        }
        // One launch at a time: a retry racing the first attempt must find its node.
        let _guard = self.spawn_lock.lock().await;
        let ws_id = match self.spawned_node(&l.request_id).await? {
            Some((ws, node)) => {
                if let Some(done) = Self::launched(&ws, &node) {
                    return Ok(done);
                }
                // Recorded but not launched yet (the hub stopped in between): launch it now.
                ws.id
            }
            None => self.add_spawned_node(ctx, &l).await?,
        };
        let reply = self.ws_launch(ctx, &ws_id).await?;
        let node = reply
            .workstream
            .nodes
            .iter()
            .find(|n| n.spawn_request.as_deref() == Some(l.request_id.as_str()))
            .cloned()
            .ok_or_else(|| MarsadError::io("the spawned node disappeared"))?;
        if let Some(done) = Self::launched(&reply.workstream, &node) {
            return Ok(done);
        }
        let why = reply
            .launched
            .iter()
            .find(|o| o.node == node.id)
            .and_then(|o| o.error.clone())
            .map(|e| e.to_string())
            .unwrap_or_else(|| "it did not start".into());
        Err(refused(format!(
            "{} could not be launched: {why}",
            node.name
        )))
    }

    /// Record the new node (and the workstream, for a standalone parent); returns its id.
    async fn add_spawned_node(
        &self,
        ctx: &CallContext,
        l: &SpawnLaunch,
    ) -> Result<String, MarsadError> {
        let r = self.resolve(ctx, &l.parent, &l.spec).await?;
        let mut launch = NodeLaunch::new(r.driver.driver.clone());
        launch.host = Some(r.host.clone());
        launch.cwd = Some(r.cwd.clone());
        launch.model = l.spec.model.clone();
        launch.mode = l.spec.mode.clone();
        launch.session_id = Some(SessionId::random());
        let decision = l.decision.clone();
        let request = l.request_id.clone();
        let spec = l.spec.clone();
        let apply = move |w: &mut Workstream, parent_node: &str| {
            match decision {
                SpawnDecision::AlwaysAllow => w.spawn.policy = SpawnPolicy::AlwaysAllow,
                SpawnDecision::AlwaysAllowWithHandoffs => {
                    w.spawn.policy = SpawnPolicy::AlwaysAllow;
                    w.spawn.handoffs_always_allow = true;
                }
                _ => {}
            }
            let id = free_id(w, &spec.name);
            let mut node = WorkstreamNode::launched(id.clone(), spec.name.trim(), launch);
            node.role = spec.role.trim().to_string();
            node.description = spec.role.trim().to_string();
            node.spawned_by = Some(parent_node.to_string());
            node.spawn_request = Some(request);
            w.nodes.push(node);
            let policy =
                if w.spawn.policy == SpawnPolicy::AlwaysAllow && w.spawn.handoffs_always_allow {
                    EdgePolicy::AlwaysAllow
                } else {
                    EdgePolicy::Ask
                };
            w.edges.push(WorkstreamEdge {
                from: parent_node.to_string(),
                to: id,
                policy,
            });
        };
        match (&r.parent.ws, &r.parent.node) {
            (Some(w), Some(parent_node)) => {
                let pn = parent_node.clone();
                self.workstreams
                    .edit(
                        &w.id,
                        None,
                        Box::new(move |w| {
                            apply(w, &pn);
                            Ok(())
                        }),
                    )
                    .await?;
                Ok(w.id.clone())
            }
            _ => {
                // A standalone session: approving created its workstream.
                let mut w = Workstream::new("", r.parent.ws_name());
                w.description = format!("Created when {} spawned a worker.", r.parent.name);
                let pn = workstreams::slug(&r.parent.name);
                w.nodes.push(WorkstreamNode {
                    id: pn.clone(),
                    name: r.parent.name.clone(),
                    role: String::new(),
                    description: String::new(),
                    session: Some(NodeSession {
                        host: HostId::new(l.parent.host.as_str()),
                        session: r.parent.snap.id.clone(),
                    }),
                    launch: None,
                    spawned_by: None,
                    spawn_request: None,
                });
                apply(&mut w, &pn);
                Ok(self.workstreams.create(w).await?.id)
            }
        }
    }

    /// `workstreams.spawn.dismiss`: stop a worker the caller spawned.
    pub(super) async fn spawn_dismiss(
        &self,
        ctx: &CallContext,
        d: DismissAsk,
    ) -> Result<DismissReply, MarsadError> {
        let parent = self.parent(ctx, &d.parent).await?;
        let (Some(ws), Some(pn)) = (&parent.ws, &parent.node) else {
            return Err(MarsadError::Spawn {
                code: SpawnError::NotSpawnedByCaller,
                message: "you have not spawned any agents".into(),
            });
        };
        let peer = d.peer.trim();
        let node = ws
            .nodes
            .iter()
            .find(|n| {
                n.name.eq_ignore_ascii_case(peer)
                    || n.id == peer
                    || n.session
                        .as_ref()
                        .is_some_and(|s| format!("{}/{}", s.host, s.session) == peer)
            })
            .ok_or_else(|| MarsadError::NotFound {
                what: format!("agent {peer:?} in workstream {}", ws.name),
            })?;
        if node.spawned_by.as_deref() != Some(pn.as_str()) {
            return Err(MarsadError::Spawn {
                code: SpawnError::NotSpawnedByCaller,
                message: format!(
                    "{} was not spawned by you; you can only dismiss your own workers",
                    node.name
                ),
            });
        }
        let s = node
            .session
            .clone()
            .ok_or_else(|| refused(format!("{} has not started", node.name)))?;
        self.stop_session(ctx, &s).await;
        Ok(DismissReply {
            agent: AgentId::new(s.host.as_str(), s.session.as_str()),
            name: node.name.clone(),
        })
    }

    /// Stop a session; one that already ended (or is gone) counts as stopped.
    pub(super) async fn stop_session(&self, ctx: &CallContext, s: &NodeSession) {
        let r: Result<marsad_proto::Ack, MarsadError> = self
            .on_host(
                ctx,
                &s.host,
                rt_methods::SESSIONS_STOP,
                SessionRef {
                    id: s.session.clone(),
                },
            )
            .await;
        if let Err(e) = r {
            hub_events::spawned_session_stop_failed(&s.host, &s.session, &e);
        }
    }

    /// `workstreams.spawn.spawnable`.
    pub(super) async fn spawnable(
        &self,
        ctx: &CallContext,
        a: SpawnableAsk,
    ) -> Result<Spawnable, MarsadError> {
        let parent = self.parent(ctx, &a.parent).await?;
        let st = parent.settings();
        let mut note = match &parent.ws {
            None => "a human approves each spawn; approving the first one creates a workstream for you and your workers".to_string(),
            Some(w) if w.state == WorkstreamState::Paused => {
                return Ok(Spawnable::unavailable(format!("workstream {} is paused", w.name)))
            }
            Some(w) if st.policy == SpawnPolicy::Never => {
                return Ok(Spawnable::unavailable(format!(
                    "spawning agents is turned off in workstream {}",
                    w.name
                )))
            }
            Some(_) if st.policy == SpawnPolicy::AlwaysAllow => {
                "spawning is allowed without asking (within the limits)".to_string()
            }
            Some(_) => "a human approves each spawn".to_string(),
        };
        let depth = match (&parent.ws, &parent.node) {
            (Some(w), Some(n)) => w.spawn_depth(n) + 1,
            _ => 1,
        };
        if depth > st.max_depth {
            return Ok(Spawnable::unavailable(format!(
                "agents may only spawn {} generation(s) deep here, and you were spawned yourself",
                st.max_depth
            )));
        }
        let remaining = st
            .max_agents
            .saturating_sub(parent.ws.as_ref().map(|w| w.spawned_count()).unwrap_or(0));
        if remaining == 0 {
            return Ok(Spawnable::unavailable(
                "the workstream has as many spawned agents as it allows",
            ));
        }
        let own = HostId::new(a.parent.host.as_str());
        let mut hosts_ids = vec![own.clone()];
        hosts_ids.extend(st.allowed_hosts.iter().filter(|h| **h != own).cloned());
        let futs = hosts_ids
            .into_iter()
            .filter(|h| self.host_connected(h))
            // No timer here: a host's inventory is settled by its events (see `settle_inventory`).
            // A slow probe makes the answer wait; only a host whose inventory fails is left out.
            .map(|h| async move {
                let inv = self.inventory(ctx, &h).await;
                (h, inv)
            });
        let mut hosts = Vec::new();
        for (h, inv) in futures::future::join_all(futs).await {
            let Ok(inv) = inv else { continue };
            let drivers: Vec<SpawnDriver> = inv
                .into_iter()
                .filter(|d| st.allowed_drivers.is_empty() || st.allowed_drivers.contains(&d.driver))
                .map(|d| SpawnDriver {
                    id: d.driver.to_string(),
                    name: d.display_name,
                    models: d.models.into_iter().take(MODELS).map(|m| m.id).collect(),
                })
                .collect();
            if drivers.is_empty() {
                continue;
            }
            hosts.push(SpawnHost {
                id: h.to_string(),
                name: self.host_name(&h),
                drivers,
            });
        }
        if hosts.len() == 1 {
            note.push_str("; only on your own host");
        }
        Ok(Spawnable {
            available: !hosts.is_empty(),
            note,
            hosts,
            remaining: Some(remaining),
        })
    }

    /// Archiving a workstream stops the agents spawned in it (they are kept, not deleted).
    pub(super) async fn stop_spawned(&self, ctx: &CallContext, ws: &Workstream) {
        for n in ws.nodes.iter().filter(|n| n.spawned_by.is_some()) {
            if let Some(s) = &n.session {
                self.stop_session(ctx, s).await;
            }
        }
    }

    /// The pre-chosen session of a launch, if it exists already (an idempotent re-launch).
    pub(super) async fn existing_launch(
        &self,
        ctx: &CallContext,
        host: &HostId,
        r: &LaunchRequest,
    ) -> Option<SessionSnapshot> {
        let id = r.id.clone()?;
        self.on_host::<_, SessionSnapshot>(
            ctx,
            host,
            rt_methods::SESSION_SNAPSHOT,
            SessionRef { id },
        )
        .await
        .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use serde_json::json;

    /// One driver's inventory entry (its binary found or not yet looked for).
    fn reply(installed: bool) -> InventoryReply {
        serde_json::from_value(json!({
            "drivers": [{
                "driver": "claude",
                "display_name": "Claude",
                "capabilities": {
                    "interrupt": false, "set_model": false, "set_mode": false,
                    "slash_commands": false, "permission_gates": false, "questions": false,
                    "plan_gate": false, "usage": false, "cost": false, "resume": false,
                    "continue_latest": false, "streaming_output": false,
                    "prompt_while_busy": "queued_by_cli", "earlier_history": "none",
                    "fork": false, "compact": false, "run_shell": false, "subagents": {},
                },
                "installed": installed,
                "refreshing": !installed,
                "refreshed_at_ms": 0,
            }]
        }))
        .expect("inventory reply")
    }

    fn event(r: &InventoryReply) -> Result<((), serde_json::Value), ()> {
        Ok(((), serde_json::to_value(r).expect("json")))
    }

    /// A host still probing its driver's binary keeps the wait open however long the probe takes:
    /// the probe here settles after 12 seconds, past where a 10-second bound would have given up.
    #[tokio::test(start_paused = true)]
    async fn a_slow_probe_is_waited_for_not_cut_off() {
        let first = stream::iter([event(&reply(false))]);
        let later = stream::once(async {
            tokio::time::sleep(Duration::from_secs(12)).await;
            event(&reply(true))
        });
        let events = first.chain(later).boxed();
        let settled = settle_inventory(reply(false), events).await;
        assert!(
            settled.drivers.iter().all(|d| d.installed),
            "the wait ended before the probe finished"
        );
    }

    /// When the event stream ends first, whatever was last known is the answer.
    #[tokio::test]
    async fn a_closed_stream_returns_the_last_known_inventory() {
        let events = stream::iter([event(&reply(false))]).boxed();
        let settled = settle_inventory(reply(false), events).await;
        assert!(settled.drivers.iter().all(|d| !d.installed));
    }
}
