//! Workstream dependency graph view (Level 2) - graph_view() + node/edge types.
//!
//! Extracted from lib.rs verbatim; `use super::*` brings the crate-root
//! types and imports (marsad_workstream helpers, Host/HostStatus, serde).

use super::*;

// ---------------------------------------------------------------------------
// Graph (Level 2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphView {
    pub ws_id: String,
    pub name: String,
    pub goal: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub inspector: Inspector,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub driver: Driver,
    pub host: String,
    pub state: Phase,
    pub status: String,
    pub context_pct: u8,
    pub is_lead: bool,
    pub depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeStatus {
    Normal,
    Consulting,
    Needs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    /// `bi=false` → Directed (single arrowhead), `bi=true` → Consult (two heads).
    /// Direction is the only edge axis (no data/command/both kinds).
    pub bi: bool,
    pub status: EdgeStatus,
    pub has_gate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inspector {
    pub working: usize,
    pub needs: usize,
    pub pending_handoff: Option<HandoffSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffSummary {
    pub conn_id: String,
    pub from: String,
    pub to: String,
    pub payload: String,
    pub risk: String,
}

/// Build the Level-2 dependency graph for one workstream, or `None` if `ws_id`
/// does not match any workstream.
pub fn graph_view(
    ws_id: &str,
    workstreams: &[Workstream],
    nodes: &[Node],
    connections: &[Connection],
) -> Option<GraphView> {
    let ws = workstreams.iter().find(|w| w.id == ws_id)?;

    let ws_nodes = nodes_of(ws_id, nodes);
    let graph_nodes: Vec<GraphNode> = ws_nodes
        .iter()
        .map(|n| GraphNode {
            id: n.id.clone(),
            name: n.name.clone(),
            driver: n.driver,
            host: n.host.clone(),
            state: n.state.clone(),
            status: n.status.clone(),
            context_pct: n.context_pct,
            is_lead: n.lead,
            depth: dependency_depth(&n.id, ws_id, connections),
        })
        .collect();

    let ws_conns = conns_of(ws_id, connections);
    let graph_edges: Vec<GraphEdge> = ws_conns
        .iter()
        .map(|c| {
            let has_gate = c.gate.is_some();
            let status = if has_gate {
                EdgeStatus::Needs
            } else if c.bi {
                EdgeStatus::Consulting
            } else {
                EdgeStatus::Normal
            };
            GraphEdge {
                id: c.id.clone(),
                from: c.from.clone(),
                to: c.to.clone(),
                bi: c.bi,
                status,
                has_gate,
            }
        })
        .collect();

    let owned_nodes: Vec<Node> = ws_nodes.iter().map(|n| (*n).clone()).collect();
    let counts = state_counts(&owned_nodes);

    let pending_handoff = ws_conns.iter().find(|c| c.gate.is_some()).map(|c| {
        let gate = c.gate.as_ref().expect("filtered by gate.is_some()");
        HandoffSummary {
            conn_id: c.id.clone(),
            from: c.from.clone(),
            to: c.to.clone(),
            payload: gate.payload.clone(),
            risk: gate.risk.clone(),
        }
    });

    Some(GraphView {
        ws_id: ws.id.clone(),
        name: ws.name.clone(),
        goal: ws.goal.clone(),
        nodes: graph_nodes,
        edges: graph_edges,
        inspector: Inspector {
            working: counts.working,
            needs: counts.needs,
            pending_handoff,
        },
    })
}

// ---------------------------------------------------------------------------
// SLICE G — Workstream cost breakdown (Usage tab: "By host" / "By node")
// ---------------------------------------------------------------------------

