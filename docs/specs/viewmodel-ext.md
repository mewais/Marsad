# B11 — `marsad-viewmodel` — Workstreams board + graph (additive spec)

**Additive only.** Do NOT change `AppSnapshot`, `SessionView`, or any existing function/struct/test. Add
NEW view structs + two NEW pure functions (`board_view`, `graph_view`) that take their inputs as
**parameters** (not read from `AppSnapshot`) — this keeps the extension non-breaking to `core`. Dep
already wired: `marsad-workstream` (re-use its `Workstream`, `Node`, `Connection`, `Driver`, `ConnKind`,
`HeadState`, `StateCounts`, `Phase`, and derivations `head_state`/`state_counts`/`driver_dots`/
`host_chips`/`nodes_of`/`conns_of`/`is_single_node`/`dependency_depth`/`is_acyclic`).

Design source (port exactly, invent nothing): `docs/design/DESIGN_HANDOFF.md` §Level 1 Board and §Level 2
Graph, and `docs/design/Marsad Workstreams.dc.html`.

Re-use the already-re-exported `Host` (`marsad_hosts::Host`) for the host strip.

## New input type (define here)
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkStatus { Ok, Degraded, Down }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLink { pub a: String, pub b: String, pub status: LinkStatus, pub note: Option<String> }
```

## Board (Level 1)
```rust
pub struct BoardView {
    pub summary: BoardSummary,
    pub host_chips: Vec<HostChip>,     // one per host, in input order
    pub link_chips: Vec<LinkChip>,     // ONLY links whose status != Ok, in input order
    pub attention: Vec<WorkstreamCard>,// subset of `all` needing you, input order preserved
    pub all: Vec<WorkstreamCard>,      // all workstreams after profile filter, input order
}
pub struct BoardSummary { pub workstreams: usize, pub sessions: usize, pub hosts: usize }
pub struct HostChip { pub id: String, pub label: String, pub kind: String, pub status: String }
pub struct LinkChip { pub a: String, pub b: String, pub status: String, pub note: Option<String> }
pub struct WorkstreamCard {
    pub id: String,
    pub name: String,
    pub goal: String,
    pub head_state: Option<HeadState>,
    pub driver_dots: Vec<Driver>,
    pub host_chips: Vec<String>,   // distinct host ids among the ws's nodes, first-appearance order
    pub session_count: usize,
    pub counts: StateCounts,
    pub needs_attention: bool,
    pub is_single_node: bool,
}

pub fn board_view(
    workstreams: &[Workstream],
    nodes: &[Node],
    connections: &[Connection],
    hosts: &[Host],
    links: &[HostLink],
    active_profile: Option<&str>,
) -> BoardView;
```
Semantics:
- **Profile filter:** if `active_profile` is `Some(p)`, include only workstreams whose `profile ==
  Some(p)`; if `None`, include all. `all` and `attention` are built from this filtered set, preserving
  `workstreams` input order.
- Per card, derive over that ws's nodes (`nodes_of(ws.id, nodes)`): `head_state`, `driver_dots`,
  `host_chips`, `session_count = count`, `counts = state_counts`, `is_single_node`.
- **`needs_attention`** = `head_state` is `Some(Needs)` or `Some(Error)`, **OR** any connection in this
  ws (`conns_of`) has `gate.is_some()`. `attention` = the filtered cards where `needs_attention`.
- **HostChip:** `label` = host.label; `status` string = lowercase of the host's status
  (`"connected"|"connecting"|"reconnecting"|"unreachable"` — map from `marsad_hosts::HostStatus`);
  `kind` = `"local"` if `host.is_local`, else `"wsl"` if the host's target starts with `"wsl:"`, else
  `"remote"`. (Read `marsad_hosts::Host` for exact field names.)
- **LinkChip:** emit only links with `status != Ok`; `status` = `"degraded"|"down"`.
- **BoardSummary:** `workstreams` = filtered workstream count; `sessions` = total nodes across the
  filtered workstreams; `hosts` = `hosts.len()`.

## Graph (Level 2)
```rust
pub struct GraphView {
    pub ws_id: String,
    pub name: String,
    pub goal: String,
    pub nodes: Vec<GraphNode>,   // nodes_of(ws), input order
    pub edges: Vec<GraphEdge>,   // conns_of(ws), input order
    pub inspector: Inspector,
}
pub struct GraphNode {
    pub id: String, pub name: String, pub driver: Driver, pub host: String,
    pub state: Phase, pub status: String, pub context_pct: u8, pub is_lead: bool,
    pub depth: usize,            // dependency_depth(id, ws, conns) — layout row
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeStatus { Normal, Consulting, Needs }
pub struct GraphEdge {
    pub id: String, pub from: String, pub to: String,
    pub kind: ConnKind, pub bi: bool, pub status: EdgeStatus, pub has_gate: bool,
}
pub struct Inspector {
    pub working: usize, pub needs: usize,
    pub pending_handoff: Option<HandoffSummary>,
}
pub struct HandoffSummary { pub conn_id: String, pub from: String, pub to: String, pub payload: String, pub risk: String }

pub fn graph_view(
    ws_id: &str,
    workstreams: &[Workstream],
    nodes: &[Node],
    connections: &[Connection],
) -> Option<GraphView>;
```
Semantics:
- Return `None` if no workstream has `id == ws_id`. Otherwise build from that ws.
- `nodes`: `nodes_of(ws_id, nodes)` mapped 1:1 (copy name/driver/host/state/status/context_pct/lead),
  `depth = dependency_depth(node.id, ws_id, connections)`.
- `edges`: `conns_of(ws_id, connections)` mapped; `has_gate = conn.gate.is_some()`. **`status` precedence:**
  `gate.is_some()` → `Needs`; else `bi` → `Consulting`; else `Normal`. (Colour = status; the `both`/data/
  command distinction is dash-only and lives in the UI, not here.)
- `inspector.working` / `inspector.needs` = `state_counts` over the ws's nodes (working incl. Starting;
  needs bucket). `pending_handoff` = the first `conns_of` connection with `gate.is_some()`, mapped to
  `HandoffSummary` (conn id, from, to, payload, risk); `None` if no gated connection.

All structs derive `Debug, Clone, PartialEq, Serialize, Deserialize` (Node-derived ones may drop `Eq`
since `Phase` is only `PartialEq` — follow B14's precedent). All functions pure, no IO, no `unsafe`.

## Test plan (test-agent: add NEW files only — `tests/board.rs`, `tests/graph.rs`. Do NOT edit existing
test files.) 100% line+region coverage of the new code.

`tests/board.rs`:
- BV1  summary counts (workstreams/sessions/hosts) for a multi-ws, multi-node, multi-host fixture.
- BV2  profile filter: `Some(p)` includes only that profile's workstreams; `None` includes all.
- BV3  a card's derivations (head_state, driver_dots dedup order, host_chips dedup order, session_count,
       counts, is_single_node) for a mixed ws.
- BV4  `needs_attention` true when head_state is Needs; true when Error; true when a connection has a
       gate even if all nodes are Working/Done; false otherwise. `attention` = exactly those cards.
- BV5  HostChip kind mapping: local (is_local), wsl (target `wsl:`…), remote (else); status string
       lowercased from HostStatus for each variant.
- BV6  LinkChip: `Ok` links omitted; `Degraded`/`Down` emitted with note passthrough and status string.
- BV7  empty inputs → empty board (no panic; empty vecs; zeroed summary).

`tests/graph.rs`:
- GV1  `graph_view` for an unknown ws_id → `None`.
- GV2  nodes mapped 1:1 with correct `depth` on a linear chain and a diamond (reuse B14 depth semantics).
- GV3  edge `status`: plain → Normal; `bi` (no gate) → Consulting; gated → Needs (gate wins over bi);
       `has_gate` matches `gate.is_some()`.
- GV4  `kind` passthrough for Data/Command/Both unaffected by status.
- GV5  inspector working/needs counts; `pending_handoff` = first gated connection mapped to HandoffSummary.
- GV6  `pending_handoff` is `None` when no connection is gated.
- GV7  a ws with a single node → nodes len 1, edges empty, inspector counts correct.

Coverage: hit every branch (each EdgeStatus arm, each HostChip kind arm, profile Some/None, attention
true/false paths, None vs Some graph). Delete any unreachable branch rather than leaving it uncovered.
