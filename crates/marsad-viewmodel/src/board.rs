//! Workstreams board view (Level 1) - host links, board cards, board_view().
//!
//! Extracted from lib.rs verbatim; `use super::*` brings the crate-root
//! types and imports (marsad_workstream helpers, Host/HostStatus, serde).

use super::*;

// ---------------------------------------------------------------------------
// HostLink / LinkStatus — new input types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkStatus {
    Ok,
    Degraded,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLink {
    pub a: String,
    pub b: String,
    pub status: LinkStatus,
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------
// Board (Level 1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardView {
    pub summary: BoardSummary,
    pub host_chips: Vec<HostChip>,
    pub link_chips: Vec<LinkChip>,
    pub attention: Vec<WorkstreamCard>,
    pub all: Vec<WorkstreamCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardSummary {
    pub workstreams: usize,
    pub sessions: usize,
    pub hosts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostChip {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkChip {
    pub a: String,
    pub b: String,
    pub status: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkstreamCard {
    pub id: String,
    pub name: String,
    pub goal: String,
    pub head_state: Option<HeadState>,
    pub driver_dots: Vec<Driver>,
    pub host_chips: Vec<String>,
    pub session_count: usize,
    /// The session ids in this card, in node order. Lets the dashboard look up
    /// each session's canonical `SessionView` (from `state.sessions`) and derive
    /// per-session status — badge / connection hint / resume-pause — from the SAME
    /// source of truth as the session-detail view, instead of a divergent
    /// workstream-aggregate projection. See apps/desktop bridge/sessionStatus.ts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_ids: Vec<String>,
    pub counts: StateCounts,
    pub needs_attention: bool,
    pub is_single_node: bool,
    /// Attention-card blocker headline — the pending gate's summary when a node
    /// needs the human. `None` → the UI falls back to `goal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
    /// Attention-card CTA label. `None` → the UI derives one from head_state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cta_label: Option<String>,
    /// A noteworthy end reason to surface on the card (a real error's reason, or a
    /// user Cancel). Aggregated across the workstream's nodes: a real error wins
    /// over a cancel. `None` when nothing noteworthy. Drives a small reason chip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<marsad_events::EndReason>,
}

/// Lowercase status string for a host, matching `marsad_hosts::HostStatus`.
fn host_status_str(status: &HostStatus) -> &'static str {
    match status {
        HostStatus::Connected => "connected",
        HostStatus::Connecting => "connecting",
        HostStatus::Reconnecting => "reconnecting",
        HostStatus::Unreachable => "unreachable",
    }
}

/// `"local"` if `host.is_local`, else `"wsl"` if the target starts with `"wsl:"`,
/// else `"remote"`.
fn host_kind_str(host: &Host) -> &'static str {
    if host.is_local {
        "local"
    } else if host.target.starts_with("wsl:") {
        "wsl"
    } else {
        "remote"
    }
}

/// Lowercase status string for a non-`Ok` `LinkStatus`. `Ok` links are filtered
/// out by the caller before a `LinkChip` is ever built, so this only covers the
/// two statuses that can actually reach a chip.
fn link_status_str(status: &LinkStatus) -> &'static str {
    if *status == LinkStatus::Down {
        "down"
    } else {
        "degraded"
    }
}

/// Build a `WorkstreamCard` for one workstream, deriving over its nodes/connections.
fn workstream_card(ws: &Workstream, nodes: &[Node], connections: &[Connection]) -> WorkstreamCard {
    let ws_nodes = nodes_of(&ws.id, nodes);
    let owned_nodes: Vec<Node> = ws_nodes.iter().map(|n| (*n).clone()).collect();
    let hs = head_state(&owned_nodes);
    let ws_conns = conns_of(&ws.id, connections);

    let needs_attention = matches!(hs, Some(HeadState::Needs) | Some(HeadState::Error))
        || ws_conns.iter().any(|c| c.gate.is_some());

    // For a single-node workstream with no explicit goal, fall back to the
    // node's name so the card headline is never blank.
    let goal = if ws.goal.is_empty() && owned_nodes.len() == 1 {
        owned_nodes[0].name.clone()
    } else {
        ws.goal.clone()
    };

    // When a node needs the human, surface its gate summary (carried on the
    // node's `status` by core::node_of) as the card's headline + an Answer CTA
    // so the attention card shows WHAT is asked, not just the workstream name.
    let (headline, cta_label) = if matches!(hs, Some(HeadState::Needs)) {
        match owned_nodes
            .iter()
            .find(|n| matches!(n.state, Phase::Needs) && !n.status.is_empty())
        {
            Some(n) => (Some(n.status.clone()), Some("Answer".to_string())),
            None => (None, None),
        }
    } else {
        (None, None)
    };

    // Aggregate a noteworthy end reason for the card chip: a real error's reason
    // (MaxTurns / ExecutionError) wins over a user Cancel, so a genuine failure is
    // never masked by a cancel elsewhere in the workstream.
    let end_reason = owned_nodes
        .iter()
        .filter_map(|n| n.end_reason)
        .max_by_key(|r| match r {
            marsad_events::EndReason::MaxTurns | marsad_events::EndReason::ExecutionError => 1u8,
            marsad_events::EndReason::Cancelled => 0u8,
        });

    WorkstreamCard {
        id: ws.id.clone(),
        name: ws.name.clone(),
        goal,
        head_state: hs,
        driver_dots: driver_dots(&owned_nodes),
        host_chips: host_chips(&owned_nodes),
        session_count: owned_nodes.len(),
        node_ids: owned_nodes.iter().map(|n| n.id.clone()).collect(),
        counts: state_counts(&owned_nodes),
        needs_attention,
        is_single_node: owned_nodes.len() == 1,
        headline,
        cta_label,
        end_reason,
    }
}

/// Build the workstreams board: summary, host/link chips, and per-workstream cards.
pub fn board_view(
    workstreams: &[Workstream],
    nodes: &[Node],
    connections: &[Connection],
    hosts: &[Host],
    links: &[HostLink],
    active_profile: Option<&str>,
) -> BoardView {
    let filtered: Vec<&Workstream> = workstreams
        .iter()
        .filter(|w| match active_profile {
            Some(p) => w.profile.as_deref() == Some(p),
            None => true,
        })
        .collect();

    let all: Vec<WorkstreamCard> = filtered
        .iter()
        .map(|ws| workstream_card(ws, nodes, connections))
        .collect();

    let attention: Vec<WorkstreamCard> =
        all.iter().filter(|c| c.needs_attention).cloned().collect();

    let sessions: usize = filtered
        .iter()
        .map(|ws| nodes_of(&ws.id, nodes).len())
        .sum();

    let host_chips: Vec<HostChip> = hosts
        .iter()
        .map(|h| HostChip {
            id: h.id.clone(),
            label: h.label.clone(),
            kind: host_kind_str(h).to_string(),
            status: host_status_str(&h.status).to_string(),
        })
        .collect();

    let link_chips: Vec<LinkChip> = links
        .iter()
        .filter(|l| l.status != LinkStatus::Ok)
        .map(|l| LinkChip {
            a: l.a.clone(),
            b: l.b.clone(),
            status: link_status_str(&l.status).to_string(),
            note: l.note.clone(),
        })
        .collect();

    BoardView {
        summary: BoardSummary {
            workstreams: filtered.len(),
            sessions,
            hosts: hosts.len(),
        },
        host_chips,
        link_chips,
        attention,
        all,
    }
}

