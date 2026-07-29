//! Workstream cost breakdown (Usage tab) - workstream_cost_breakdown() + rows.
//!
//! Extracted from lib.rs verbatim; `use super::*` brings the crate-root
//! types and imports (marsad_workstream helpers, Host/HostStatus, serde).

use super::*;

/// One row of the workstream cost breakdown (Usage tab's "By host" / "By
/// node" tables): a group's total cost, the distinct drivers contributing to
/// it, its share of the workstream total, and whether it is backed only by
/// estimated (litellm/hand-table backfill) cost rather than any live,
/// agent-reported cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostRow {
    /// Group identity: a host id (`by_host`) or a node/session id (`by_node`).
    pub key: String,
    /// Display label: the host id (`by_host`) or the node's display name
    /// (`by_node`).
    pub label: String,
    /// Distinct drivers contributing to this group, first-appearance order
    /// (mirrors `marsad_workstream::driver_dots`; a `by_node` row is always a
    /// single-element vec — the one node's driver).
    pub driver_dots: Vec<Driver>,
    /// Summed `cost_usd` of every contributing session (a node with no
    /// matching `SessionView` contributes `0.0`).
    pub cost_usd: f64,
    /// This group's share of the workstream total, `0.0..=100.0`. `0.0` for
    /// every row when the workstream total is `0.0` (avoids a NaN divide).
    pub pct: f32,
    /// U4 cost truthfulness (mirrors `SessionView.cost_estimated` one level
    /// up): `true` only when this group has at least one contributing
    /// session AND every one of them is estimated-only. A group with ANY
    /// live, agent-reported cost — or with no contributing sessions at all —
    /// is never labelled estimated; agent-reported cost is truth and must
    /// never be clobbered by the litellm/hand backfill table.
    pub estimated: bool,
}

/// The full cost breakdown for one workstream's sessions, grouped by host and
/// by node/session, for the Session Usage tab's "Cost breakdown" box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkstreamCostBreakdown {
    /// Sum of `cost_usd` across every session in the workstream.
    pub total_cost_usd: f64,
    /// `true` only when every session in the workstream is estimated-only
    /// (see `CostRow::estimated`); `false` when the workstream has no
    /// sessions at all (no data is not "estimated", it's just empty).
    pub total_estimated: bool,
    /// Grouped by host id, sorted by cost descending.
    pub by_host: Vec<CostRow>,
    /// One row per node/session, sorted by cost descending.
    pub by_node: Vec<CostRow>,
}

/// `cost / total * 100`, or `0.0` when `total == 0.0` (avoids a NaN divide).
fn cost_pct(cost: f64, total: f64) -> f32 {
    if total == 0.0 {
        0.0
    } else {
        (cost / total * 100.0) as f32
    }
}

/// Sort rows by `cost_usd` descending; ties broken by `key` ascending for
/// determinism (grouping is otherwise order-dependent on a `HashMap`).
fn sort_cost_rows_desc(rows: &mut [CostRow]) {
    rows.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key.cmp(&b.key))
    });
}

/// Build the workstream-scoped cost breakdown (Usage tab's "By host" / "By
/// node" tables) for one workstream's sessions, or `None` if `ws_id` does not
/// match any workstream.
///
/// `nodes` and `sessions` are the FULL lists (across all workstreams), like
/// `graph_view`'s `nodes` parameter — this function filters to `ws_id`'s own
/// nodes internally (`nodes_of`) and joins each to the `SessionView` sharing
/// its `id` (a node with no matching session view contributes `0.0`/
/// not-estimated, mirroring `make_session_view`'s rule that no cost data at
/// all is not "estimated", it's just empty).
pub fn workstream_cost_breakdown(
    ws_id: &str,
    workstreams: &[Workstream],
    nodes: &[Node],
    sessions: &[SessionView],
) -> Option<WorkstreamCostBreakdown> {
    workstreams.iter().find(|w| w.id == ws_id)?;

    let rows: Vec<(&Node, f64, bool)> = nodes_of(ws_id, nodes)
        .into_iter()
        .map(|n| {
            let sv = sessions.iter().find(|s| s.id == n.id);
            let cost = sv.map(|s| s.cost_usd).unwrap_or(0.0);
            let estimated = sv.map(|s| s.cost_estimated).unwrap_or(false);
            (n, cost, estimated)
        })
        .collect();

    let total_cost_usd: f64 = rows.iter().map(|(_, cost, _)| cost).sum();
    let total_estimated = !rows.is_empty() && rows.iter().all(|(_, _, estimated)| *estimated);

    // by_node: one row per node, no aggregation.
    let mut by_node: Vec<CostRow> = rows
        .iter()
        .map(|(n, cost, estimated)| CostRow {
            key: n.id.clone(),
            label: n.name.clone(),
            driver_dots: vec![n.driver],
            cost_usd: *cost,
            pct: cost_pct(*cost, total_cost_usd),
            estimated: *estimated,
        })
        .collect();
    sort_cost_rows_desc(&mut by_node);

    // by_host: group rows by host id, first-appearance order.
    let mut host_order: Vec<String> = Vec::new();
    let mut host_cost: HashMap<String, f64> = HashMap::new();
    let mut host_all_estimated: HashMap<String, bool> = HashMap::new();
    let mut host_nodes: HashMap<String, Vec<Node>> = HashMap::new();
    for (n, cost, estimated) in &rows {
        if !host_cost.contains_key(&n.host) {
            host_order.push(n.host.clone());
        }
        *host_cost.entry(n.host.clone()).or_insert(0.0) += cost;
        let all_estimated_so_far = host_all_estimated.entry(n.host.clone()).or_insert(true);
        *all_estimated_so_far = *all_estimated_so_far && *estimated;
        host_nodes
            .entry(n.host.clone())
            .or_default()
            .push((*n).clone());
    }

    let mut by_host: Vec<CostRow> = host_order
        .into_iter()
        .map(|host| {
            let cost = host_cost[&host];
            CostRow {
                estimated: host_all_estimated[&host],
                driver_dots: driver_dots(&host_nodes[&host]),
                pct: cost_pct(cost, total_cost_usd),
                cost_usd: cost,
                label: host.clone(),
                key: host,
            }
        })
        .collect();
    sort_cost_rows_desc(&mut by_host);

    Some(WorkstreamCostBreakdown {
        total_cost_usd,
        total_estimated,
        by_host,
        by_node,
    })
}
