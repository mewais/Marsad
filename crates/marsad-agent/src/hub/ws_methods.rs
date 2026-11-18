//! The `workstreams.*` methods of [`HubServices`] (the document logic lives in the
//! [`super::workstreams`] actor; launches and node calls run here, outside it).

use std::collections::BTreeMap;

use marsad_proto::agentconfig::{LaunchRequest, WorkstreamNodeRef};
use marsad_proto::workstream::{
    AddNode, Connect, EdgeHistoryPage, EdgeHistoryRequest, EdgeRef, FromTemplate,
    LaunchWorkstreamReply, NodeLaunch, NodeLaunchOutcome, NodeRef, NodeSession, SaveTemplate,
    Workstream, WorkstreamDraft, WorkstreamEdge, WorkstreamState, WorkstreamTemplate,
    WorkstreamUpdate,
};
use marsad_proto::{Command, MarsadError};
use wasit_control::CallContext;
use wasit_runtime::wire::{methods as rt_methods, CommandReply, CommandRequest};
use wasl_proto::{methods as wasl_methods, AgentId, EdgePolicy, TaskList, TaskView};

use super::workstreams::{self, LaunchResult};
use super::HubServices;
use crate::util::now_ms;

/// Labels put on every session a workstream launches.
pub const LABEL_WORKSTREAM: &str = "marsad.workstream";
pub const LABEL_NODE: &str = "marsad.node";

fn not_in(ws: &str, what: &str) -> MarsadError {
    MarsadError::NotFound {
        what: format!("{what} in workstream {ws}"),
    }
}

impl HubServices {
    pub(super) async fn ws_create(&self, d: WorkstreamDraft) -> Result<Workstream, MarsadError> {
        let mut ws = Workstream::new(d.id.unwrap_or_default(), d.name);
        ws.description = d.description;
        ws.nodes = d.nodes;
        ws.edges = d.edges;
        ws.limits = d.limits;
        ws.layout = d.layout;
        // Sessions are recorded by launch / add_node only.
        for n in &mut ws.nodes {
            n.session = None;
        }
        self.workstreams.create(ws).await
    }

    pub(super) async fn ws_update(&self, u: WorkstreamUpdate) -> Result<Workstream, MarsadError> {
        let new = u.workstream;
        let id = new.id.clone();
        self.workstreams
            .edit(
                &id,
                u.expected_revision,
                Box::new(move |w| {
                    let sessions: BTreeMap<String, Option<NodeSession>> = w
                        .nodes
                        .iter()
                        .map(|n| (n.id.clone(), n.session.clone()))
                        .collect();
                    w.name = new.name;
                    w.description = new.description;
                    w.edges = new.edges;
                    w.limits = new.limits;
                    w.layout = new.layout;
                    w.nodes = new.nodes;
                    for n in &mut w.nodes {
                        n.session = sessions.get(&n.id).cloned().flatten();
                    }
                    Ok(())
                }),
            )
            .await
    }

    pub(super) async fn ws_set_state(
        &self,
        id: &str,
        to: WorkstreamState,
    ) -> Result<Workstream, MarsadError> {
        self.workstreams
            .edit(
                id,
                None,
                Box::new(move |w| {
                    let ok = matches!(
                        (w.state, to),
                        (WorkstreamState::Running, WorkstreamState::Paused)
                            | (WorkstreamState::Paused, WorkstreamState::Running)
                            | (_, WorkstreamState::Archived)
                    ) || w.state == to;
                    if !ok {
                        return Err(MarsadError::invalid(format!(
                            "workstream {} is {:?}; cannot become {to:?}",
                            w.id, w.state
                        )));
                    }
                    w.state = to;
                    Ok(())
                }),
            )
            .await
    }

    pub(super) async fn ws_connect(
        &self,
        c: Connect,
        upsert: bool,
    ) -> Result<Workstream, MarsadError> {
        let ws = c.workstream.clone();
        self.workstreams
            .edit(
                &ws,
                None,
                Box::new(move |w| {
                    match w
                        .edges
                        .iter_mut()
                        .find(|e| e.from == c.from && e.to == c.to)
                    {
                        Some(e) => e.policy = c.policy,
                        None if upsert => w.edges.push(WorkstreamEdge {
                            from: c.from,
                            to: c.to,
                            policy: c.policy,
                        }),
                        None => {
                            return Err(not_in(&w.id, &format!("connection {} → {}", c.from, c.to)))
                        }
                    }
                    Ok(())
                }),
            )
            .await
    }

    pub(super) async fn ws_disconnect(&self, r: EdgeRef) -> Result<Workstream, MarsadError> {
        self.workstreams
            .edit(
                &r.workstream.clone(),
                None,
                Box::new(move |w| {
                    let before = w.edges.len();
                    w.edges.retain(|e| !(e.from == r.from && e.to == r.to));
                    if w.edges.len() == before {
                        return Err(not_in(&w.id, &format!("connection {} → {}", r.from, r.to)));
                    }
                    Ok(())
                }),
            )
            .await
    }

    pub(super) async fn ws_add_node(
        &self,
        ctx: &CallContext,
        a: AddNode,
    ) -> Result<Workstream, MarsadError> {
        let mut node = a.node;
        if node.id.is_empty() {
            node.id = workstreams::slug(&node.name);
        }
        if let Some(s) = &node.session {
            // A running session joins as it is; it must exist.
            if self.workstreams.snapshot(s.clone()).await.is_none() {
                self.host_of(Some(s.host.clone()))?;
                let _: marsad_proto::SessionSnapshot = self
                    .on_host(
                        ctx,
                        &s.host,
                        rt_methods::SESSION_SNAPSHOT,
                        wasit_runtime::wire::SessionRef {
                            id: s.session.clone(),
                        },
                    )
                    .await?;
            }
        }
        let launch_now = node.session.is_none();
        let id = a.workstream.clone();
        let position = a.position;
        let ws = self
            .workstreams
            .edit(
                &id,
                None,
                Box::new(move |w| {
                    if let Some(p) = position {
                        w.layout.insert(node.id.clone(), p);
                    }
                    w.nodes.push(node);
                    Ok(())
                }),
            )
            .await?;
        if launch_now && matches!(ws.state, WorkstreamState::Running | WorkstreamState::Paused) {
            return Ok(self.ws_launch(ctx, &id).await?.workstream);
        }
        Ok(ws)
    }

    pub(super) async fn ws_remove_node(&self, r: NodeRef) -> Result<Workstream, MarsadError> {
        self.workstreams
            .edit(
                &r.workstream.clone(),
                None,
                Box::new(move |w| {
                    if w.node(&r.node).is_none() {
                        return Err(not_in(&w.id, &format!("node {}", r.node)));
                    }
                    w.nodes.retain(|n| n.id != r.node);
                    w.edges.retain(|e| e.from != r.node && e.to != r.node);
                    w.layout.remove(&r.node);
                    Ok(())
                }),
            )
            .await
    }

    /// Launch every unlaunched node through the normal launch path, push the topology to
    /// every involved host, then send the first prompts.
    pub(super) async fn ws_launch(
        &self,
        ctx: &CallContext,
        id: &str,
    ) -> Result<LaunchWorkstreamReply, MarsadError> {
        let (doc, todo) = self.workstreams.begin_launch(id).await?;
        let launches = todo.iter().map(|(node, spec)| {
            let req = self.node_launch_request(id, node, spec);
            let name = doc
                .nodes
                .iter()
                .find(|n| n.id == *node)
                .map(|n| n.name.trim().to_string())
                .filter(|n| !n.is_empty());
            async move {
                let r = match self.host_of(spec.host.clone()) {
                    Ok(_) => self.launch(ctx, req).await,
                    Err(e) => Err(e),
                };
                // A node's session is called what the node is called (a node often has no
                // first prompt, which would otherwise name it).
                if let (Ok(reply), Some(name)) = (&r, name) {
                    let rename = wasit_runtime::wire::RenameSession {
                        id: reply.id.clone(),
                        display_name: Some(name),
                    };
                    let done: Result<marsad_proto::Ack, _> = self
                        .on_host(ctx, &reply.host, rt_methods::SESSIONS_RENAME, rename)
                        .await;
                    if let Err(e) = done {
                        tracing::warn!(node, error = %e, "cannot name the node's session");
                    }
                }
                (node.clone(), r)
            }
        });
        let results = futures::future::join_all(launches).await;
        let mut outcomes = Vec::new();
        let mut recorded: Vec<LaunchResult> = Vec::new();
        for (node, r) in results {
            match r {
                Ok(reply) => {
                    let s = NodeSession {
                        host: reply.host,
                        session: reply.id,
                    };
                    outcomes.push(NodeLaunchOutcome {
                        node: node.clone(),
                        session: Some(s.clone()),
                        error: None,
                    });
                    recorded.push((node, Ok(s)));
                }
                Err(e) => {
                    tracing::warn!(workstream = id, node, error = %e, "node launch failed");
                    outcomes.push(NodeLaunchOutcome {
                        node: node.clone(),
                        session: None,
                        error: Some(e.clone()),
                    });
                    recorded.push((node, Err(e)));
                }
            }
        }
        let ws = self.workstreams.end_launch(id, recorded).await?;
        // The agents must see their peers before their first turn: push now and wait.
        let t = workstreams::topology(&ws);
        let pushes = ws.topology_hosts.iter().map(|h| {
            let t = t.clone();
            async move {
                let r: Result<wasl_proto::TopologyApplied, MarsadError> = self
                    .on_host(ctx, h, wasl_methods::TOPOLOGY_SET, t)
                    .await;
                if let Err(e) = r {
                    tracing::warn!(workstream = id, host = %h, error = %e, "topology push at launch failed (it is retried)");
                }
            }
        });
        futures::future::join_all(pushes).await;
        for o in &outcomes {
            let (Some(s), Some(prompt)) = (
                &o.session,
                todo.iter()
                    .find(|(n, _)| *n == o.node)
                    .and_then(|(_, l)| l.first_prompt.clone()),
            ) else {
                continue;
            };
            let r: Result<CommandReply, MarsadError> = self
                .on_host(
                    ctx,
                    &s.host,
                    rt_methods::SESSION_COMMAND,
                    CommandRequest {
                        id: s.session.clone(),
                        command: Command::Prompt { text: prompt },
                    },
                )
                .await;
            if let Err(e) = r {
                tracing::warn!(workstream = id, node = %o.node, error = %e, "first prompt failed");
            }
        }
        Ok(LaunchWorkstreamReply {
            workstream: ws,
            launched: outcomes,
        })
    }

    fn node_launch_request(&self, ws: &str, node: &str, spec: &NodeLaunch) -> LaunchRequest {
        let mut req = LaunchRequest::new(spec.driver.clone());
        req.host = spec.host.clone();
        req.cwd = spec.cwd.clone();
        req.model = spec.model.clone();
        req.mode = spec.mode.clone();
        req.labels = spec.labels.clone();
        req.resume = spec.resume.clone();
        req.labels.insert(LABEL_WORKSTREAM.into(), ws.to_string());
        req.labels.insert(LABEL_NODE.into(), node.to_string());
        // The node's overrides are its stored `workstream_node` layer.
        req.workstream = Some(WorkstreamNodeRef {
            workstream: ws.to_string(),
            node: node.to_string(),
        });
        req
    }

    pub(super) async fn ws_save_template(
        &self,
        s: SaveTemplate,
    ) -> Result<WorkstreamTemplate, MarsadError> {
        let ws = self.workstreams.get(&s.workstream).await?;
        let mut nodes = Vec::new();
        for n in &ws.nodes {
            let mut n = n.clone();
            if n.launch.is_none() {
                // A node added from a running session: launch the same CLI the same way.
                let snap = match &n.session {
                    Some(sess) => self.workstreams.snapshot(sess.clone()).await,
                    None => None,
                };
                let Some(snap) = snap else {
                    return Err(MarsadError::invalid(format!(
                        "node {} has no launch spec and its session is not known; cannot template it",
                        n.id
                    )));
                };
                let mut l = NodeLaunch::new(snap.driver.clone());
                l.host = n.session.as_ref().map(|s| s.host.clone());
                l.cwd = snap.cwd.clone();
                l.model = snap.model.clone();
                n.launch = Some(l);
            }
            // A template starts fresh conversations: continuing or resuming one belongs to
            // the workstream it was taken from.
            if let Some(l) = n.launch.as_mut() {
                l.resume = marsad_proto::runtime::ResumeRequest::New;
            }
            n.session = None;
            nodes.push(n);
        }
        let mut edges = ws.edges.clone();
        // Decisions taken while running were about those sessions, not the template.
        for e in &mut edges {
            if e.policy == EdgePolicy::AlwaysAllow {
                e.policy = EdgePolicy::Ask;
            }
        }
        let name = s.name.unwrap_or_else(|| ws.name.clone());
        let existing: Vec<String> = self
            .store
            .templates()
            .await?
            .into_iter()
            .map(|t| t.id)
            .collect();
        let base = workstreams::slug(&name);
        let mut id = base.clone();
        let mut k = 2;
        while existing.contains(&id) {
            id = format!("{base}-{k}");
            k += 1;
        }
        let t = WorkstreamTemplate {
            id,
            schema: marsad_proto::workstream::SCHEMA,
            name,
            description: s.description.unwrap_or_else(|| ws.description.clone()),
            nodes,
            edges,
            limits: ws.limits.clone(),
            layout: ws.layout.clone(),
            created_at_ms: now_ms(),
        };
        self.store.put_template(t.clone()).await?;
        Ok(t)
    }

    pub(super) async fn ws_from_template(
        &self,
        f: FromTemplate,
    ) -> Result<Workstream, MarsadError> {
        let t = self.store.template(f.template.clone()).await?;
        let mut ws = Workstream::new("", f.name.unwrap_or_else(|| t.name.clone()));
        ws.description = t.description;
        ws.edges = t.edges;
        ws.limits = t.limits;
        ws.layout = t.layout;
        ws.template = Some(t.id);
        ws.nodes = t.nodes;
        let local = self.registry.local().id.clone();
        for n in &mut ws.nodes {
            n.session = None;
            if let Some(l) = &mut n.launch {
                let from = l.host.clone().unwrap_or_else(|| local.clone());
                if let Some(to) = f.hosts.get(&from) {
                    l.host = Some(to.clone());
                }
            }
        }
        self.workstreams.create(ws).await
    }

    pub(super) async fn ws_history(
        &self,
        ctx: &CallContext,
        r: EdgeHistoryRequest,
    ) -> Result<EdgeHistoryPage, MarsadError> {
        let ws = self.workstreams.get(&r.workstream).await?;
        let agent = |id: &str| -> Result<(AgentId, NodeSession), MarsadError> {
            let n = ws
                .node(id)
                .ok_or_else(|| not_in(&ws.id, &format!("node {id}")))?;
            let s = n
                .session
                .clone()
                .ok_or_else(|| MarsadError::invalid(format!("node {id} is not launched")))?;
            Ok((AgentId::new(s.host.as_str(), s.session.as_str()), s))
        };
        let (fa, fs) = agent(&r.from)?;
        let (ta, _) = agent(&r.to)?;
        let list: TaskList = self
            .on_host(ctx, &fs.host, wasl_methods::TASKS_LIST, ())
            .await?;
        let mut tasks: Vec<TaskView> = list
            .outbound
            .into_iter()
            .filter(|t| t.task.workstream == ws.id && t.task.from == fa && t.task.to == ta)
            .filter(|t| r.before_ms.is_none_or(|b| t.task.created_ms < b))
            .collect();
        tasks.sort_by(|a, b| {
            b.task
                .created_ms
                .cmp(&a.task.created_ms)
                .then_with(|| b.task.id.cmp(&a.task.id))
        });
        let limit = r.limit.unwrap_or(50).clamp(1, 500) as usize;
        let more = tasks.len() > limit;
        tasks.truncate(limit);
        let next_before = if more {
            tasks.last().map(|t| t.task.created_ms)
        } else {
            None
        };
        Ok(EdgeHistoryPage { tasks, next_before })
    }
}
