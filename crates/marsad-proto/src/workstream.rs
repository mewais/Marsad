//! `workstreams.*` and the `workstreams` topic (hub): sessions wired into a directed graph
//! across machines (ARCHITECTURE §5, Layer 2).
//!
//! The hub is the source of truth. A [`Workstream`] document is stored by the hub; whenever
//! its graph changes (nodes launched or added, connections, policies, limits, pause) the hub
//! derives the Wasl [`Topology`](wasl_proto::Topology) from it, bumps
//! [`Workstream::topology_version`] and pushes it (`wasl.topology.set`, idempotent and
//! versioned) to every involved host's node. It pushes again whenever a host link
//! (re)connects and when the hub starts. "Always allow" chosen in a handoff gate comes back
//! from the nodes as an edge event and is written into the document.
//!
//! A node is a session `(host, session id)` once launched or added; before that it is a
//! [`NodeLaunch`] spec (driver, directory, model, first prompt, and its own config override
//! layer, stored as the `workstream_node` layer of the effective-config merge).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wasit_control::HostId;
use wasit_core::{DriverId, GateRequest};
use wasit_runtime::wire::{ResumeRequest, SessionId, SessionSnapshot};
use wasl_proto::{EdgePolicy, Limits, TaskView};

use crate::agentconfig::OverrideLayer;

/// Schema version of [`Workstream`] documents (no migration: other versions are refused).
pub const SCHEMA: u32 = 1;

/// Lifecycle of a workstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkstreamState {
    /// Being designed; nothing is pushed to the nodes.
    #[default]
    Draft,
    /// Launched; the topology is in force on every involved host.
    Running,
    /// In force, but new delegations are rejected (tasks already created carry on).
    Paused,
    /// Kept for reference; the topology is removed from the nodes.
    Archived,
}

/// How to launch a node that has no session yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NodeLaunch {
    /// `None` = the hub's own host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostId>,
    pub driver: DriverId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Sent once the topology is in force (so the agent already sees its peers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Start fresh (default), continue the CLI's most recent conversation in `cwd`, or resume
    /// one conversation by its native id (as `sessions.launch`).
    #[serde(default, skip_serializing_if = "is_new")]
    pub resume: ResumeRequest,
    /// This node's config override layer (between the host and the session layer).
    #[serde(default, skip_serializing_if = "OverrideLayer::is_empty")]
    pub overrides: OverrideLayer,
    /// The session id to launch it under (chosen in advance for spawned agents, so a launch
    /// repeated after a hub restart finds the session instead of starting a second one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
}

impl NodeLaunch {
    pub fn new(driver: impl Into<DriverId>) -> Self {
        NodeLaunch {
            host: None,
            driver: driver.into(),
            cwd: None,
            model: None,
            mode: None,
            first_prompt: None,
            labels: BTreeMap::new(),
            resume: ResumeRequest::New,
            overrides: OverrideLayer::default(),
            session_id: None,
        }
    }
}

fn is_new(r: &ResumeRequest) -> bool {
    *r == ResumeRequest::New
}

/// A session a node is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NodeSession {
    pub host: HostId,
    pub session: SessionId,
}

/// One agent of the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamNode {
    /// Unique within the workstream (`[a-z0-9_-]`, chosen by the client or derived from the
    /// name).
    pub id: String,
    /// Display name, unique within the workstream; agents may address peers by it.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The session this node is (set once launched or added from a running session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<NodeSession>,
    /// How to launch it (absent for a node added from an existing session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<NodeLaunch>,
    /// The node whose agent spawned this one (`spawn_agent`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<String>,
    /// The spawn request that created it (idempotency key of the launch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_request: Option<String>,
}

impl WorkstreamNode {
    /// A node launched from `launch`.
    pub fn launched(id: impl Into<String>, name: impl Into<String>, launch: NodeLaunch) -> Self {
        WorkstreamNode {
            id: id.into(),
            name: name.into(),
            role: String::new(),
            description: String::new(),
            session: None,
            launch: Some(launch),
            spawned_by: None,
            spawn_request: None,
        }
    }
}

/// Whether agents in a workstream may start workers (`spawn_agent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SpawnPolicy {
    /// Every spawn raises a gate on the asking session.
    #[default]
    Ask,
    /// Spawns within the limits go through without a gate.
    AlwaysAllow,
    /// Agents are told spawning is not allowed.
    Never,
}

/// Spawning rules of a workstream (enforced by the hub).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SpawnSettings {
    #[serde(default)]
    pub policy: SpawnPolicy,
    /// "Always allow" also covers handoffs to spawned agents: their connections are created
    /// as `always_allow` instead of `ask`. Only with `policy: always_allow`.
    #[serde(default)]
    pub handoffs_always_allow: bool,
    /// Most spawned agents the workstream may have (default 8).
    #[serde(default = "default_max_spawned")]
    pub max_agents: u32,
    /// Most generations: an agent spawned by a spawned agent is depth 2 (default 2).
    #[serde(default = "default_spawn_depth")]
    pub max_depth: u32,
    /// Hosts spawned agents may run on besides the asking agent's own host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<HostId>,
    /// Drivers that may be spawned (empty = any installed one).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_drivers: Vec<DriverId>,
}

fn default_max_spawned() -> u32 {
    8
}

fn default_spawn_depth() -> u32 {
    2
}

impl Default for SpawnSettings {
    fn default() -> Self {
        SpawnSettings {
            policy: SpawnPolicy::Ask,
            handoffs_always_allow: false,
            max_agents: default_max_spawned(),
            max_depth: default_spawn_depth(),
            allowed_hosts: Vec::new(),
            allowed_drivers: Vec::new(),
        }
    }
}

/// A directed connection: `from` may delegate to `to` (node ids).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamEdge {
    pub from: String,
    pub to: String,
    /// `ask` (a handoff gate on every delegation), `always_allow`, or `deny` (blocked).
    #[serde(default)]
    pub policy: EdgePolicy,
}

/// Where a node sits in the graph view.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NodePosition {
    pub x: f64,
    pub y: f64,
}

/// A workstream document (stored by the hub).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Workstream {
    pub id: String,
    /// Always [`SCHEMA`].
    pub schema: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub state: WorkstreamState,
    pub nodes: Vec<WorkstreamNode>,
    pub edges: Vec<WorkstreamEdge>,
    /// Max depth, default task deadline, optional budget.
    pub limits: Limits,
    /// Whether and how its agents may spawn workers.
    #[serde(default)]
    pub spawn: SpawnSettings,
    /// Graph positions by node id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub layout: BTreeMap<String, NodePosition>,
    /// Version of the Wasl topology derived from this document (bumped by the hub whenever
    /// the derived topology changes).
    pub topology_version: u64,
    /// Every host the topology was ever pushed to (removal is pushed there too).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topology_hosts: Vec<HostId>,
    /// The template this workstream was created from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Incremented by the hub on every write.
    pub revision: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl Workstream {
    /// An empty draft (timestamps 0; the hub fills them).
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Workstream {
            id: id.into(),
            schema: SCHEMA,
            name: name.into(),
            description: String::new(),
            state: WorkstreamState::Draft,
            nodes: Vec::new(),
            edges: Vec::new(),
            limits: Limits::default(),
            spawn: SpawnSettings::default(),
            layout: BTreeMap::new(),
            topology_version: 0,
            topology_hosts: Vec::new(),
            template: None,
            revision: 0,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    pub fn node(&self, id: &str) -> Option<&WorkstreamNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn edge(&self, from: &str, to: &str) -> Option<&WorkstreamEdge> {
        self.edges.iter().find(|e| e.from == from && e.to == to)
    }

    /// The node that is session `s`.
    pub fn node_of(&self, host: &HostId, session: &SessionId) -> Option<&WorkstreamNode> {
        self.nodes.iter().find(|n| {
            n.session
                .as_ref()
                .is_some_and(|x| x.host == *host && x.session == *session)
        })
    }

    /// Generation of a node: 0 unless spawned, else its spawner's + 1.
    pub fn spawn_depth(&self, node: &str) -> u32 {
        let mut depth = 0;
        let mut cur = self.node(node);
        while let Some(parent) = cur.and_then(|n| n.spawned_by.as_deref()) {
            depth += 1;
            if depth > 64 {
                break;
            }
            cur = self.node(parent);
        }
        depth
    }

    /// Number of spawned nodes.
    pub fn spawned_count(&self) -> u32 {
        self.nodes.iter().filter(|n| n.spawned_by.is_some()).count() as u32
    }
}

/// A reusable graph: nodes as launch specs (no sessions), connections, limits, layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamTemplate {
    pub id: String,
    pub schema: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Every node has a `launch` spec and no `session`.
    pub nodes: Vec<WorkstreamNode>,
    pub edges: Vec<WorkstreamEdge>,
    pub limits: Limits,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub layout: BTreeMap<String, NodePosition>,
    pub created_at_ms: u64,
}

// ── requests ────────────────────────────────────────────────────────────────

/// `workstreams.create` (starts as a draft).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamDraft {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Requested id (`None` = derived from the name). Must be unique.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default)]
    pub nodes: Vec<WorkstreamNode>,
    #[serde(default)]
    pub edges: Vec<WorkstreamEdge>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub spawn: SpawnSettings,
    #[serde(default)]
    pub layout: BTreeMap<String, NodePosition>,
}

/// `workstreams.update`: replace the editable parts (name, description, nodes, edges,
/// limits, spawn settings, layout). State, versions and node sessions already recorded by the hub are kept
/// as the hub knows them (a node's `session` can only be set by launch / `add_node`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamUpdate {
    pub workstream: Workstream,
    /// Fail unless the stored revision matches (optimistic concurrency between clients).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

/// Names one workstream (`get`, `delete`, `launch`, `pause`, `resume`, `archive`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamRef {
    pub id: String,
}

/// Result of `workstreams.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamList {
    pub workstreams: Vec<Workstream>,
}

/// One node of a `workstreams.launch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NodeLaunchOutcome {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<NodeSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::MarsadError>,
}

/// Result of `workstreams.launch`: every unlaunched node was launched (or why not), the
/// topology was pushed, then first prompts were sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LaunchWorkstreamReply {
    pub workstream: Workstream,
    pub launched: Vec<NodeLaunchOutcome>,
}

/// `workstreams.add_node`: add a node. With `node.session` set it is a running session
/// joining live (no restart: every Marsad session already has the Wasl tools); with only a
/// `launch` spec it is launched at once when the workstream is running.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AddNode {
    pub workstream: String,
    pub node: WorkstreamNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<NodePosition>,
}

/// `workstreams.remove_node` (its connections go too; the session keeps running).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NodeRef {
    pub workstream: String,
    pub node: String,
}

/// `workstreams.connect` (adds or updates) and `workstreams.set_policy` (the connection must
/// exist).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Connect {
    pub workstream: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub policy: EdgePolicy,
}

/// `workstreams.disconnect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EdgeRef {
    pub workstream: String,
    pub from: String,
    pub to: String,
}

/// `workstreams.save_template`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SaveTemplate {
    pub workstream: String,
    /// Default: the workstream's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `workstreams.from_template`: a new draft from a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FromTemplate {
    pub template: String,
    /// Default: the template's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Move nodes to other hosts (template host → host).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hosts: BTreeMap<HostId, HostId>,
}

/// Names one template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TemplateRef {
    pub id: String,
}

/// Result of `workstreams.templates.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TemplateList {
    pub templates: Vec<WorkstreamTemplate>,
}

/// `workstreams.history`: the tasks of one connection, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EdgeHistoryRequest {
    pub workstream: String,
    pub from: String,
    pub to: String,
    /// Page cursor: only tasks created before this time (`next_before` of the last page).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_ms: Option<u64>,
    /// Default 50, at most 500.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Result of `workstreams.history`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EdgeHistoryPage {
    pub tasks: Vec<TaskView>,
    /// Pass as `before_ms` for the next (older) page; absent on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_before: Option<u64>,
}

// ── the `workstreams` topic ─────────────────────────────────────────────────

/// A node with its live session snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NodeView {
    pub node: String,
    /// The host the node runs (or will run) on.
    pub host: HostId,
    /// The session's latest snapshot (absent before launch, or while its host has not
    /// reported it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Box<SessionSnapshot>>,
    /// A launch of this node is in progress.
    #[serde(default)]
    pub launching: bool,
}

/// A handoff gate waiting for the human (answer it on the sending session with
/// `session.command` / `answer_gate` like any other gate).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HandoffGateView {
    pub host: HostId,
    /// The sending session.
    pub session: SessionId,
    pub task: String,
    pub raised_at_ms: u64,
    /// Always a `GateRequest::Handoff`.
    pub gate: GateRequest,
}

/// A spawn gate waiting for the human on one of the workstream's sessions (answer it with
/// `session.command` / `answer_gate`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SpawnGateView {
    /// The asking node.
    pub node: String,
    pub host: HostId,
    pub session: SessionId,
    pub raised_at_ms: u64,
    /// Always a `GateRequest::Spawn`.
    pub gate: GateRequest,
}

/// One connection with what is moving over it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EdgeView {
    pub from: String,
    pub to: String,
    pub policy: EdgePolicy,
    /// Tasks not yet terminal (pending approval, queued, running with progress).
    pub in_flight: Vec<TaskView>,
    pub pending_gates: Vec<HandoffGateView>,
    pub completed: u32,
    /// Failed, denied, cancelled or expired.
    pub unsuccessful: u32,
    /// Cost reported by the results of this connection's tasks.
    pub cost_usd: f64,
}

/// How far the topology push got on one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TopologySync {
    pub host: HostId,
    /// Version the host's node acknowledged (0 = none yet in this hub run).
    pub applied: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Cost roll-up of a workstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamCost {
    /// Sum of the node sessions' cumulative cost.
    pub sessions_usd: f64,
    /// Sum of the cost reported by completed tasks' results.
    pub tasks_usd: f64,
    /// Per node id (session cost).
    pub by_node: BTreeMap<String, f64>,
}

/// Everything a client needs to draw one workstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamView {
    pub workstream: Workstream,
    pub nodes: Vec<NodeView>,
    pub edges: Vec<EdgeView>,
    pub sync: Vec<TopologySync>,
    pub cost: WorkstreamCost,
    /// Spawn gates waiting on its sessions.
    #[serde(default)]
    pub pending_spawns: Vec<SpawnGateView>,
}

/// Payload of the `workstreams` topic. Apply in order; `reset` replaces everything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkstreamsEvent {
    Reset {
        workstreams: Vec<WorkstreamView>,
    },
    /// A workstream or anything shown for it changed (the full view of that workstream).
    Upserted {
        workstream: Box<WorkstreamView>,
    },
    Removed {
        id: String,
    },
}
