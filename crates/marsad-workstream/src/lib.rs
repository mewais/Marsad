//! marsad-workstream — workstream/graph domain (B14).
//!
//! Pure, stateless domain for the Workstreams upgrade: profiles, workstreams,
//! nodes (agent sessions), typed connections, and the graph derivations the
//! board and graph views need (head-state, driver dots, host chips, dependency
//! depth, connection validation).
//!
//! See `docs/specs/workstream.md` for the authoritative contract.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

pub use marsad_events::Phase;

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Which coding-agent CLI drives a node's session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    Claude,
    Codex,
    Gemini,
}

impl Driver {
    /// Wire/display string for this driver.
    pub fn as_str(&self) -> &'static str {
        match self {
            Driver::Claude => "claude",
            Driver::Codex => "codex",
            Driver::Gemini => "gemini",
        }
    }

    /// Design hex color for this driver's dot/badge.
    pub fn color(&self) -> &'static str {
        match self {
            Driver::Claude => "#d97757",
            Driver::Codex => "#3fb6a8",
            Driver::Gemini => "#9b8cf0",
        }
    }
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// A grouping lens over workstreams (renamed from the old "workspace").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Workstream
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workstream {
    pub id: String,
    pub name: String,
    pub goal: String,
    /// Profile id this workstream is grouped under; `None` = ungrouped.
    #[serde(default)]
    pub profile: Option<String>,
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// One agent session within a workstream.
///
/// Note: does not derive `Eq` — `Phase` (re-exported from `marsad-events`) only
/// implements `PartialEq`, so `Eq` cannot be derived transitively here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    /// Workstream id (foreign key).
    pub ws: String,
    pub name: String,
    pub driver: Driver,
    /// Host id.
    pub host: String,
    pub state: Phase,
    /// Lifecycle liveness (two-axis model): `true` = Resumed (process alive on
    /// its host), `false` = Paused (not running, for any reason). Authoritative
    /// from the agent, not inferred from `state`. Drives board grouping: a
    /// not-live node is dormant (→ Paused) regardless of `state`, except a
    /// `Done` node which is always surfaced in Done (finished its job).
    #[serde(default = "default_node_live")]
    pub live: bool,
    /// Why this node's last turn ended, when noteworthy (user Cancel / real
    /// error). Surfaced as a reason chip on the board card + graph node. `None`
    /// for a normal turn.
    #[serde(default)]
    pub end_reason: Option<marsad_events::EndReason>,
    /// True if this node is the workstream lead (design `role:'lead'`).
    #[serde(default)]
    pub lead: bool,
    pub branch: String,
    /// 0..=100
    pub context_pct: u8,
    pub status: String,
}

fn default_node_live() -> bool {
    true
}

// ---------------------------------------------------------------------------
// ConnKind / Handoff / Connection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnKind {
    Data,
    Command,
    Both,
}

/// HITL descriptor on a gated wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub payload: String,
    pub risk: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    /// Workstream id.
    pub ws: String,
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    pub kind: ConnKind,
    /// Two-way / consulting edge (design `dir:'bi'`).
    #[serde(default)]
    pub bi: bool,
    /// Some(_) => a HITL gate sits on this connection (cross-host handoff).
    #[serde(default)]
    pub gate: Option<Handoff>,
}

// ---------------------------------------------------------------------------
// HeadState / StateCounts
// ---------------------------------------------------------------------------

/// Most-urgent aggregate state across a set of nodes.
/// Priority order: Needs > Error > Working > Done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadState {
    Needs,
    Error,
    Working,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StateCounts {
    pub working: usize,
    pub needs: usize,
    pub error: usize,
    pub done: usize,
}

// ---------------------------------------------------------------------------
// ConnError
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnError {
    /// `from == to`.
    SelfLoop,
    /// `from` or `to` not present in the node set.
    UnknownEndpoint,
    /// `from`/`to`/`conn.ws` are not all the same workstream.
    CrossWorkstream,
    /// An identical `(from, to)` already exists in the same ws.
    Duplicate,
    /// Adding this edge makes the directed graph cyclic.
    Cycle,
}

impl fmt::Display for ConnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            ConnError::SelfLoop => "connection endpoints must not be the same node",
            ConnError::UnknownEndpoint => "connection endpoint is not a known node",
            ConnError::CrossWorkstream => "connection endpoints span more than one workstream",
            ConnError::Duplicate => "an identical connection already exists",
            ConnError::Cycle => "connection would create a dependency cycle",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for ConnError {}

// ---------------------------------------------------------------------------
// Severity helper (internal)
// ---------------------------------------------------------------------------

/// Severity of a node for head-state / bucketing purposes, in the two-axis
/// model. `None` means the node is dormant (→ the board's Paused group).
///
/// - `Done` is activity-terminal: a finished session is ALWAYS surfaced in Done
///   (the user wants completed work visible) whether or not its process is still
///   alive.
/// - Every other phase only contributes while the session is actually alive
///   (`live`). A not-live node — user-paused, or host-killed mid-work
///   (reboot / OOM / scheduler / crash) — is dormant, so it lands in Paused
///   instead of lying with Working/Needs/Error.
fn severity(phase: &Phase, live: bool) -> Option<HeadState> {
    match phase {
        // Attention / terminal states ALWAYS surface, regardless of liveness — a
        // pending gate (Needs), a failure (Error), or finished work (Done) must
        // never be hidden behind `live` (a blocked gate means claude is alive and
        // waiting for the human; hiding it would strand an unanswerable gate).
        Phase::Needs => Some(HeadState::Needs),
        Phase::Error => Some(HeadState::Error),
        Phase::Done => Some(HeadState::Done),
        // Everything else only surfaces while actually live; a not-live session is
        // dormant (→ the board's Paused group).
        _ if !live => None,
        Phase::Working => Some(HeadState::Working),
        // Starting folds into Done: a launched-but-idle session is idle/ready, the
        // same resting bucket as a finished one.
        Phase::Starting => Some(HeadState::Done),
        Phase::Stale | Phase::Detached => None,
    }
}

fn severity_rank(state: HeadState) -> u8 {
    match state {
        HeadState::Needs => 4,
        HeadState::Error => 3,
        HeadState::Working => 2,
        HeadState::Done => 1,
    }
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// The most-urgent state among the given nodes, priority Needs > Error > Working > Done.
/// `Stale`/`Detached` are dormant and contribute no severity. Empty or all-dormant → `None`.
pub fn head_state(nodes: &[Node]) -> Option<HeadState> {
    nodes
        .iter()
        .filter_map(|n| severity(&n.state, n.live))
        .max_by_key(|s| severity_rank(*s))
}

/// Tally nodes by bucket: `working` = Working+Starting, `needs` = Needs, `error` = Error,
/// `done` = Done. `Stale`/`Detached` are not counted in any bucket.
pub fn state_counts(nodes: &[Node]) -> StateCounts {
    let mut counts = StateCounts::default();
    for n in nodes {
        match severity(&n.state, n.live) {
            Some(HeadState::Needs) => counts.needs += 1,
            Some(HeadState::Error) => counts.error += 1,
            Some(HeadState::Working) => counts.working += 1,
            Some(HeadState::Done) => counts.done += 1,
            None => {}
        }
    }
    counts
}

/// Distinct drivers present, in first-appearance order (stable, deduplicated).
pub fn driver_dots(nodes: &[Node]) -> Vec<Driver> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for n in nodes {
        if seen.insert(n.driver) {
            out.push(n.driver);
        }
    }
    out
}

/// Distinct host ids present, in first-appearance order (stable, deduplicated).
pub fn host_chips(nodes: &[Node]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for n in nodes {
        if seen.insert(n.host.clone()) {
            out.push(n.host.clone());
        }
    }
    out
}

/// All nodes whose `ws == ws_id`, preserving input order.
pub fn nodes_of<'a>(ws_id: &str, nodes: &'a [Node]) -> Vec<&'a Node> {
    nodes.iter().filter(|n| n.ws == ws_id).collect()
}

/// All connections whose `ws == ws_id`, preserving input order.
pub fn conns_of<'a>(ws_id: &str, conns: &'a [Connection]) -> Vec<&'a Connection> {
    conns.iter().filter(|c| c.ws == ws_id).collect()
}

/// `true` iff exactly one node has `ws == ws_id` (the fast-lane detector).
pub fn is_single_node(ws_id: &str, nodes: &[Node]) -> bool {
    nodes.iter().filter(|n| n.ws == ws_id).count() == 1
}

/// Build the directed adjacency (from -> [to]) for a workstream's connections, ignoring `bi`.
fn directed_adjacency<'a>(
    ws_id: &str,
    conns: impl Iterator<Item = &'a Connection>,
) -> HashMap<&'a str, Vec<&'a str>> {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in conns.filter(|c| c.ws == ws_id) {
        adj.entry(c.from.as_str()).or_default().push(c.to.as_str());
    }
    adj
}

/// DFS-based cycle check over a directed adjacency map.
fn has_cycle(adj: &HashMap<&str, Vec<&str>>) -> bool {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        Visiting,
        Done,
    }

    let mut marks: HashMap<&str, Mark> = HashMap::new();

    fn visit<'a>(
        node: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        marks: &mut HashMap<&'a str, Mark>,
    ) -> bool {
        match marks.get(node) {
            Some(Mark::Visiting) => return true,
            Some(Mark::Done) => return false,
            None => {}
        }
        marks.insert(node, Mark::Visiting);
        if let Some(children) = adj.get(node) {
            for &child in children {
                if visit(child, adj, marks) {
                    return true;
                }
            }
        }
        marks.insert(node, Mark::Done);
        false
    }

    let nodes: Vec<&str> = adj.keys().copied().collect();
    for n in nodes {
        if visit(n, adj, &mut marks) {
            return true;
        }
    }
    false
}

/// Validate a proposed connection against a node set and already-present connections.
/// Checks, in order (first failure wins): SelfLoop, UnknownEndpoint/CrossWorkstream,
/// Duplicate, Cycle.
pub fn validate_connection(
    conn: &Connection,
    nodes: &[Node],
    existing: &[Connection],
) -> Result<(), ConnError> {
    if conn.from == conn.to {
        return Err(ConnError::SelfLoop);
    }

    let from_node = nodes.iter().find(|n| n.id == conn.from);
    let to_node = nodes.iter().find(|n| n.id == conn.to);

    let (from_node, to_node) = match (from_node, to_node) {
        (Some(f), Some(t)) => (f, t),
        _ => return Err(ConnError::UnknownEndpoint),
    };

    if from_node.ws != conn.ws || to_node.ws != conn.ws {
        return Err(ConnError::CrossWorkstream);
    }

    let is_duplicate = existing
        .iter()
        .any(|c| c.ws == conn.ws && c.from == conn.from && c.to == conn.to);
    if is_duplicate {
        return Err(ConnError::Duplicate);
    }

    let mut adj = directed_adjacency(&conn.ws, existing.iter());
    adj.entry(conn.from.as_str())
        .or_default()
        .push(conn.to.as_str());
    if has_cycle(&adj) {
        return Err(ConnError::Cycle);
    }

    Ok(())
}

/// `true` iff the directed graph formed by the `ws_id` connections (`from -> to`, `bi`
/// ignored) has no cycle. Empty edge set → `true`.
pub fn is_acyclic(ws_id: &str, conns: &[Connection]) -> bool {
    let adj = directed_adjacency(ws_id, conns.iter());
    !has_cycle(&adj)
}

/// Longest directed path (edge count) from any source (no inbound edge in this ws) to
/// `node_id`, over the `ws_id` connections. A node with no inbound edges has depth 0.
/// If `node_id` participates in a cycle or is absent, return 0.
pub fn dependency_depth(node_id: &str, ws_id: &str, conns: &[Connection]) -> usize {
    let relevant: Vec<&Connection> = conns.iter().filter(|c| c.ws == ws_id).collect();

    // Absent node: not an endpoint of any relevant connection.
    let present = relevant
        .iter()
        .any(|c| c.from == node_id || c.to == node_id);
    if !present {
        return 0;
    }

    let adj = directed_adjacency(ws_id, relevant.iter().copied());
    if has_cycle(&adj) {
        return 0;
    }

    // Build reverse adjacency: for each node, its predecessors.
    let mut preds: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut all_nodes: HashSet<&str> = HashSet::new();
    for c in &relevant {
        preds
            .entry(c.to.as_str())
            .or_default()
            .push(c.from.as_str());
        all_nodes.insert(c.from.as_str());
        all_nodes.insert(c.to.as_str());
    }

    // Memoized longest-path-to-node_id via predecessors (graph is acyclic, so this
    // recursion terminates).
    fn depth_of<'a>(
        node: &'a str,
        preds: &HashMap<&'a str, Vec<&'a str>>,
        memo: &mut HashMap<&'a str, usize>,
    ) -> usize {
        if let Some(&d) = memo.get(node) {
            return d;
        }
        let d = match preds.get(node) {
            None => 0,
            Some(ps) => ps
                .iter()
                .map(|p| depth_of(p, preds, memo) + 1)
                .max()
                .unwrap_or(0),
        };
        memo.insert(node, d);
        d
    }

    let mut memo = HashMap::new();
    depth_of(node_id, &preds, &mut memo)
}
