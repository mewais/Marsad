# B12 — `marsad-core` — Workstream layer (additive spec)

**Additive only.** Do NOT change existing methods, fields' meanings, or existing tests (mechanical
struct-literal field additions are permitted where a new Core field forces them — assertions unchanged).
Existing behavior (sessions, workspaces, hosts, gates, snapshot) stays intact. Deps already available:
`marsad-workstream`, `marsad-store` (has upsert/delete workstream/connection/profile + `set_membership`
+ `profiles_from_workspaces` + `Restored.{workstreams,connections,profiles,membership}`),
`marsad-viewmodel` (has `board_view`/`graph_view` + view structs).

Core owns workstream organization; the detached agent stays workstream-agnostic. IDs for user-created
entities are **caller-provided** (deterministic); migration derives ids.

## New private `Core` state (add fields; update `with_deps`/constructors + any existing `Core { … }` literal
in tests mechanically with empty defaults)
```rust
workstreams: Vec<marsad_workstream::Workstream>,
connections: Vec<marsad_workstream::Connection>,
profiles:    Vec<marsad_workstream::Profile>,
membership:  std::collections::HashMap<String, String>, // session_id -> ws_id
ws_leads:    std::collections::HashMap<String, String>, // ws_id -> lead session_id
active_profile: Option<String>,
```

## New `CoreError` variant
```rust
#[error("connection error: {0}")]
Connection(String),   // from marsad_workstream::validate_connection failures
```

## New `VmUpdate` variants
```rust
Workstreams(Vec<marsad_workstream::Workstream>),   // the workstream list changed
Board(marsad_viewmodel::BoardView),                // the board recomputed
```

## Node projection (private helper)
`fn node_of(&self, id, record) -> marsad_workstream::Node` — mirror `make_session_view`:
- `id`; `ws = membership[id]` (skip sessions with no membership when building the node list);
- `name` = record.name or `session_display_name(project, id)`; `host = record.host_id`;
- `driver = Driver::Claude` (Phase 1 is Claude-only); `state = record.state.phase.clone()`;
- `lead = ws_leads.get(ws) == Some(id)`; `branch = String::new()` (same as SessionView today);
- `context_pct = record.state.context_pct.round().clamp(0.0,100.0) as u8`; `status = String::new()`.

`pub fn nodes(&self) -> Vec<marsad_workstream::Node>` — one node per session that HAS a membership entry,
in `session_order`. (Sessions with no membership are not yet placed in a workstream.)

## New public methods (all additive)
Each mutation persists via the store and returns the VmUpdates the UI needs (at least `Workstreams(...)`
and a recomputed `Board(self.board())`), following the existing workspace-method style.

- `pub fn load_workstreams(&mut self) -> Result<Vec<VmUpdate>, CoreError>`
  Restore `workstreams/connections/profiles/membership` from `store.restore()` into Core state, then run
  **migration** (idempotent):
  1. `profiles`: if empty, seed from `profiles_from_workspaces(&self.workspaces)` and persist each.
  2. For every session id in `session_order` with **no** membership entry: create a single-node workstream
     with id `format!("ws-{session_id}")`, name = the session's display name, `goal = String::new()`,
     `profile = None`; set `membership[session_id] = that ws id`; mark it the ws lead; persist the
     workstream + membership. (So every existing session becomes a fast-lane single-node workstream.)
  Returns `[Workstreams(...), Board(...)]`.

- `pub fn create_workstream(&mut self, id, name, goal, profile: Option<String>) -> Result<Vec<VmUpdate>, CoreError>`
  Reject a duplicate id with `CoreError::Connection`? No — duplicates overwrite (upsert semantics), matching
  store. Persist; return `[Workstreams, Board]`.

- `pub fn delete_workstream(&mut self, id) -> Result<Vec<VmUpdate>, CoreError>`
  Remove the workstream, its connections (and their store rows), its membership entries and lead, and
  persist all removals. Return `[Workstreams, Board]`. Deleting an absent id is a no-op `Ok`.

- `pub fn add_connection(&mut self, id, ws, from, to, kind, bi) -> Result<Vec<VmUpdate>, CoreError>`
  Build the `Connection`, validate with `marsad_workstream::validate_connection(&conn, &self.nodes(),
  &self.connections)`; on `Err(e)` return `CoreError::Connection(e.to_string())` and persist nothing.
  On Ok: push, persist, return `[Board]`.

- `pub fn remove_connection(&mut self, id) -> Result<Vec<VmUpdate>, CoreError>` — remove + persist-delete;
  absent id → no-op `Ok`. Return `[Board]`.

- `pub fn create_profile(&mut self, id, name) -> Result<Vec<VmUpdate>, CoreError>` — upsert + persist; `[Board]`.
- `pub fn delete_profile(&mut self, id) -> Result<Vec<VmUpdate>, CoreError>` — remove profile; any workstream
  whose `profile == Some(id)` is set to `None` and re-persisted; if `active_profile == Some(id)` clear it.
  Return `[Workstreams, Board]`. Absent id → no-op `Ok`.

- `pub fn assign_session(&mut self, session_id, ws_id, lead: bool) -> Result<Vec<VmUpdate>, CoreError>`
  Require `session_id` to be a known session and `ws_id` a known workstream, else `CoreError::UnknownSession`
  / `CoreError::Connection`. Set `membership[session_id] = ws_id`; if `lead`, set `ws_leads[ws_id] =
  session_id`. Persist membership. Return `[Workstreams, Board]`.

- `pub fn set_active_profile(&mut self, profile: Option<String>) -> Vec<VmUpdate>` — set the board filter;
  return `[Board]`. (No persistence required in Phase 1.)

- `pub fn board(&self) -> marsad_viewmodel::BoardView` — delegate to `marsad_viewmodel::board_view(
  &self.workstreams, &self.nodes(), &self.connections, &self.hosts.list(), &[], self.active_profile.as_deref())`.
  (Host-links are `&[]` in Phase 1.)
- `pub fn graph(&self, ws_id: &str) -> Option<marsad_viewmodel::GraphView>` — delegate to
  `marsad_viewmodel::graph_view(ws_id, &self.workstreams, &self.nodes(), &self.connections)`.

(Use whatever `self.hosts` accessor returns the `Vec<Host>` the existing `snapshot()` uses.)

## Test plan (test-agent: add a NEW file `tests/workstreams.rs`; you MAY mechanically add the new Core
fields to any existing `Core { … }` construction the compiler flags, with empty defaults — no assertion
changes. Drive Core via the existing test harness/`LoopbackFake` + `with_deps` used by the other core
tests.) 100% line+region coverage of the new code.

- CW1  `create_workstream` then `board()` shows the workstream; `Workstreams` VmUpdate carries it.
- CW2  `delete_workstream` removes it + cascades: its connections gone, membership + lead cleared; absent id → Ok no-op.
- CW3  `add_connection` valid edge → appears in `graph(ws)`; persisted (survives a reload via a fresh Core `load_workstreams`).
- CW4  `add_connection` invalid (self-loop / unknown endpoint / cross-ws / duplicate / cycle) → `CoreError::Connection`, nothing persisted.
- CW5  `remove_connection` removes from `graph`; absent id → Ok no-op.
- CW6  `create_profile` + `delete_profile`: delete un-sets it on member workstreams and clears `active_profile` if it pointed at it.
- CW7  `assign_session`: unknown session → `UnknownSession`; unknown ws → `Connection`; valid → membership set, node appears in `nodes()`/`graph`; `lead:true` marks the node `lead`.
- CW8  `set_active_profile` filters `board()` to that profile; `None` shows all.
- CW9  `nodes()`: one node per session-with-membership, in `session_order`, driver=Claude, correct state/host/lead; sessions without membership excluded.
- CW10 migration in `load_workstreams`: a Core with N sessions and no persisted workstreams →
       N single-node workstreams (`ws-<id>`), each session a lead of its own; profiles seeded from
       legacy workspaces; running it again is idempotent (no duplicates).
- CW11 persistence round-trip: create workstreams/connections/profiles + assign membership, then a NEW
       Core over the SAME backend `load_workstreams()` restores them all identically.
- CW12 `board()`/`graph()` delegate correctly (spot-check a known board card's head_state and a graph edge's status).

Coverage: exercise every new branch (each error arm, cascade path, migration idempotency, the `lead`
true/false paths). Delete unreachable branches rather than leaving them uncovered.
