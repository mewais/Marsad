# B14 — `marsad-workstream` — spec (authoritative)

**Tier:** L0 (pure, stateless). **Deps:** `serde`, `marsad-events` (for `Phase`). No IO, no `unsafe`.

The domain layer for the Workstreams upgrade. Everything here is plain data + pure functions over
slices; the crate holds **no state** and performs **no IO**. It is the single source of truth for the
graph model (profiles → workstreams → nodes + connections) and the derivations the UI/view-model need.

Design source: `docs/design/Marsad Workstreams.dc.html` (seed data in the `Component` logic:
`hosts`, workstreams `w1..w5`, session/node list, connection list) and `docs/design/DESIGN_HANDOFF.md`
(§ State Management → Domain data model, and § Head-state).

---

## 1. Types (all `#[derive(Debug, Clone, PartialEq, Eq)]`; serde where noted)

### 1.1 `Driver`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver { Claude, Codex, Gemini }
```
- serde: serializes to `"claude" | "codex" | "gemini"` (matches design `driver:'claude'` etc.).
- `Driver::as_str(&self) -> &'static str` → `"claude" | "codex" | "gemini"`.
- `Driver::color(&self) -> &'static str` → design driver hex: Claude `#d97757`, Codex `#3fb6a8`,
  Gemini `#9b8cf0`.

### 1.2 `Profile` (renamed from the old "workspace" — a grouping lens)
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile { pub id: String, pub name: String }
```

### 1.3 `Workstream`
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workstream {
    pub id: String,
    pub name: String,
    pub goal: String,
    /// Profile id this workstream is grouped under; `None` = ungrouped.
    #[serde(default)]
    pub profile: Option<String>,
}
```

### 1.4 `Node` (one agent session within a workstream)
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub ws: String,            // workstream id (foreign key)
    pub name: String,
    pub driver: Driver,
    pub host: String,          // host id
    pub state: marsad_events::Phase,
    /// True if this node is the workstream lead (design `role:'lead'`).
    #[serde(default)]
    pub lead: bool,
    pub branch: String,
    pub context_pct: u8,       // 0..=100
    pub status: String,
}
```
Re-export: `pub use marsad_events::Phase;` (node state is the session Phase — do not redefine it).

### 1.5 `ConnKind` + `Connection`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnKind { Data, Command, Both }   // "data" | "command" | "both"

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff { pub payload: String, pub risk: String }  // HITL descriptor on a gated wire

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub ws: String,            // workstream id
    pub from: String,          // source node id
    pub to: String,            // target node id
    pub kind: ConnKind,
    /// Two-way / consulting edge (design `dir:'bi'`).
    #[serde(default)]
    pub bi: bool,
    /// Some(_) => a HITL gate sits on this connection (cross-host handoff).
    #[serde(default)]
    pub gate: Option<Handoff>,
}
```

### 1.6 `HeadState` + `StateCounts`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadState { Needs, Error, Working, Done }  // priority order: Needs > Error > Working > Done

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StateCounts { pub working: usize, pub needs: usize, pub error: usize, pub done: usize }
```

### 1.7 `ConnError`
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnError {
    SelfLoop,            // from == to
    UnknownEndpoint,     // from or to not present in the node set
    CrossWorkstream,     // from/to/conn.ws are not all the same workstream
    Duplicate,           // an identical (from,to) already exists in the same ws
    Cycle,               // adding this edge makes the directed graph cyclic (see §2.7)
}
```
Implements `std::fmt::Display` (human strings) and `std::error::Error`.

---

## 2. Functions (all pure, free functions)

### 2.1 `head_state(nodes: &[Node]) -> Option<HeadState>`
The most-urgent state among the given nodes, priority **Needs > Error > Working > Done**.
- Severity map: `Needs`→4, `Error`→3, `Working`→2, `Done`→1. `Starting` counts as `Working` (2).
- `Stale` and `Detached` are dormant: they contribute **no** severity.
- Return the `HeadState` for the highest severity present. If `nodes` is empty, or every node is
  `Stale`/`Detached`, return `None`.

### 2.2 `state_counts(nodes: &[Node]) -> StateCounts`
Tally by bucket: `working` = count of `Working`+`Starting`; `needs` = `Needs`; `error` = `Error`;
`done` = `Done`. `Stale`/`Detached` are not counted in any bucket.

### 2.3 `driver_dots(nodes: &[Node]) -> Vec<Driver>`
Distinct drivers present, in **first-appearance order** of `nodes` (stable, deduplicated).

### 2.4 `host_chips(nodes: &[Node]) -> Vec<String>`
Distinct host ids present, in first-appearance order (stable, deduplicated).

### 2.5 `nodes_of<'a>(ws_id: &str, nodes: &'a [Node]) -> Vec<&'a Node>`
All nodes whose `ws == ws_id`, preserving input order.

### 2.6 `conns_of<'a>(ws_id: &str, conns: &'a [Connection]) -> Vec<&'a Connection>`
All connections whose `ws == ws_id`, preserving input order.

### 2.7 `is_single_node(ws_id: &str, nodes: &[Node]) -> bool`
`true` iff exactly one node has `ws == ws_id` (the fast-lane detector — a single-node workstream opens
straight to its session, skipping the graph). `false` for zero or ≥2.

### 2.8 `validate_connection(conn: &Connection, nodes: &[Node], existing: &[Connection]) -> Result<(), ConnError>`
Validate a proposed connection against a node set and the already-present connections. Checks, in this
order (first failure wins):
1. `conn.from == conn.to` → `SelfLoop`.
2. `from` and `to` must both exist as node ids in `nodes`, and both those nodes' `ws` must equal
   `conn.ws` → else `UnknownEndpoint` (endpoint missing) / `CrossWorkstream` (endpoint in another ws or
   `conn.ws` mismatch). (Missing endpoint takes precedence over cross-workstream.)
3. An existing connection in `existing` within the same `ws` with the same `(from, to)` → `Duplicate`.
   (Direction-sensitive: `(a,b)` and `(b,a)` are not duplicates.)
4. If adding `conn` to `existing` (restricted to `conn.ws`, **directed**, ignoring `bi`) would create a
   directed cycle → `Cycle`. Note: a `bi` edge is still treated as a single directed `from→to` edge for
   cycle-detection purposes (the reverse arrow is a UI affordance, not a dependency). A 2-node mutual pair
   (`a→b` plus a separate `b→a`) is therefore a directed cycle and is **rejected** — model a two-way
   relationship as a single `bi` connection, never as two opposing edges.
On all checks passing → `Ok(())`.

### 2.9 `is_acyclic(ws_id: &str, conns: &[Connection]) -> bool`
`true` iff the directed graph formed by the `ws_id` connections (each `from→to`, `bi` ignored) has no
cycle. Empty edge set → `true`.

### 2.10 `dependency_depth(node_id: &str, ws_id: &str, conns: &[Connection]) -> usize`
Longest directed path (edge count) from any source (a node with no inbound edge in this ws) to
`node_id`, over the `ws_id` connections (`from→to`, `bi` ignored). A node with no inbound edges has
depth `0`. Used by the by-host layout to pick a node's row. If `node_id` participates in a cycle or is
absent, return `0` (callers validate acyclicity first via §2.9).

---

## 3. Test plan (test-agent: write these; 100% line+region coverage required)

Split across `tests/happy.rs`, `tests/branches.rs`, `tests/errors.rs` (mirror sibling crates). Every
case below must be a real, running, asserting test — none `#[ignore]`d or stubbed.

**Types / serde (`happy.rs`)**
- T1  `Driver` serde round-trips to `"claude"|"codex"|"gemini"`; `as_str` + `color` for all three.
- T2  `ConnKind` serde ↔ `"data"|"command"|"both"`; `HeadState` serde ↔ lowercase.
- T3  `Workstream` with `profile: None` omits/defaults correctly; `Some` round-trips.
- T4  `Node` round-trips incl. `lead` default false, a `Phase` value, `context_pct` boundaries 0 and 100.
- T5  `Connection` round-trips: `bi` default false, `gate: None` default, and a `Some(Handoff{..})`.

**Derivations — happy (`happy.rs`)**
- T6  `head_state`: mixed set with a Needs → `Some(Needs)`; Error+Working+Done → `Some(Error)`;
      Working+Done → `Some(Working)`; only Done → `Some(Done)`.
- T7  `head_state`: `Starting` alone → `Some(Working)`; `Starting`+`Done` → `Some(Working)`.
- T8  `state_counts`: a set covering all buckets incl. `Starting` folded into `working`.
- T9  `driver_dots`: dedup + first-appearance order (e.g. gemini,claude,gemini,codex → [gemini,claude,codex]).
- T10 `host_chips`: dedup + first-appearance order.
- T11 `nodes_of` / `conns_of`: filter by ws, order preserved; foreign ws excluded.
- T12 `is_single_node`: 1 node → true; 2 → false; 0 → false.
- T13 `dependency_depth`: linear chain a→b→c gives depths 0,1,2; a diamond a→b, a→c, b→d, c→d gives d=2.

**Branches / edges (`branches.rs`)**
- B1  `head_state`: empty slice → `None`.
- B2  `head_state`: all `Stale`/`Detached` → `None`; a `Needs` among `Stale` → `Some(Needs)`.
- B3  `state_counts`: `Stale`/`Detached` counted in no bucket (all-zero for such a set).
- B4  `driver_dots`/`host_chips`: empty slice → empty vec.
- B5  `is_acyclic`: empty → true; DAG → true; a 3-cycle a→b→c→a → false.
- B6  `dependency_depth`: node with no inbound → 0; absent node → 0; node in a cycle → 0.
- B7  `validate_connection`: a valid new edge in a 2-node ws → `Ok(())`.
- B8  `bi` edge is treated as single directed edge for cycle detection (a→b bi, then b→a proposed →
      `Cycle` since a→b already directed and b→a closes it) — assert this precise behavior.

**Errors (`errors.rs`)**
- E1  `validate_connection` self-loop (from==to) → `SelfLoop` (even if the node is unknown — self-loop first).
- E2  unknown `from` or `to` → `UnknownEndpoint`.
- E3  endpoint exists but belongs to another ws, or `conn.ws` disagrees with endpoints → `CrossWorkstream`.
- E4  missing-endpoint precedence: when an endpoint is both missing AND cross-ws-ish, `UnknownEndpoint` wins.
- E5  duplicate `(from,to)` in same ws → `Duplicate`. Reversed `(to,from)` is **not** a `Duplicate`
      (direction-sensitive) — but with `a→b` already present it closes the 2-cycle `a→b→a`, so it returns
      `Cycle`. (A valid `Ok` reverse is impossible while the forward edge exists; that's the intended rule.)
- E6  adding an edge that closes a directed cycle → `Cycle`.
- E7  `ConnError` `Display` produces a non-empty distinct string for each variant; `Error` impl present.

Coverage note: no unreachable branches. If a match arm/guard cannot be hit through the public API,
remove it rather than leaving it uncovered.
