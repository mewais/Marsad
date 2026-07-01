# U-INT-1 — Desktop bridge integration (command surface + mock backend) — spec (`apps/desktop`)

**The anti-gap foundation.** Wires every workstream backend operation into the TS bridge, and provides a
**mock backend** that simulates `marsad-core` so the wired app can be driven end-to-end in Vitest without
Tauri. Additive: extend existing files + add a mock module. Do NOT break existing behavior/tests.

Backend contract = `marsad-core` (docs/specs/core-ext.md). Command names/args mirror it exactly.

## Files
- EXTEND `src/bridge/types.ts` — add the two new `VmUpdateEvent` variants (see §1).
- EXTEND `src/bridge/api.ts` — add the command wrappers + data fetchers (see §2), each with the standard
  `isTauri()` split: real `invoke(...)` in Tauri, delegate to the mock backend otherwise.
- NEW `src/bridge/mockWorkstreams.ts` — the mock backend (see §3).
- NEW `tests/bridgeWorkstreams.test.ts` — Vitest suite (see §Test plan).
Import the view types from `src/bridge/workstreamTypes.ts` (already exists).

## 1. types.ts — VmUpdateEvent additions (append to the union)
```ts
  | { kind: "workstreams"; workstreams: Workstream[] }
  | { kind: "board"; board: BoardView };
```
Add `Workstream` (id/name/goal/profile) to types.ts (or import from workstreamTypes.ts) and import
`BoardView` from workstreamTypes.ts.

## 2. api.ts — new commands (exact invoke command name in parens; args snake_case to match core)
Each: `if (!isTauri()) { return mockWorkstreams.<op>(...); } const invoke = await getInvoke(); return invoke("<cmd>", {...}) as Promise<...>;`
- `createWorkstream(id, name, goal, profile?)` → `create_workstream` `{id,name,goal,profile}` → `Promise<void>`
- `deleteWorkstream(id)` → `delete_workstream` `{id}`
- `addConnection(id, ws, from, to, kind, bi)` → `add_connection` `{id,ws,from,to,kind,bi}`
- `removeConnection(id)` → `remove_connection` `{id}`
- `createProfile(id, name)` → `create_profile` `{id,name}`
- `deleteProfile(id)` → `delete_profile` `{id}`
- `assignSession(sessionId, wsId, lead)` → `assign_session` `{session_id,ws_id,lead}`
- `setActiveProfile(profile?)` → `set_active_profile` `{profile}`
- `fetchBoard()` → `board` `{}` → `Promise<BoardView>`
- `fetchGraph(wsId)` → `graph` `{ws_id}` → `Promise<GraphView | null>`
- `addConnection`'s `kind` is `"data"|"command"|"both"`.
Mutating commands return `Promise<void>`. `fetchBoard`/`fetchGraph` return data. All also exported for the
mock path so tests can call them without Tauri.

## 3. mockWorkstreams.ts — the simulated backend
A module holding in-memory workstream state and implementing every op so the app behaves realistically in
browser/test mode. Faithful enough to prove wiring; not a full re-derivation of the Rust viewmodel.
```ts
export interface MockState {
  workstreams: Workstream[];
  profiles: Profile[];
  connections: Connection[];   // marsad_workstream::Connection shape
  nodes: GraphNode[];          // per-session graph nodes (seed a few)
  membership: Record<string, string>;   // session_id -> ws_id
  activeProfile: string | null;
}
export const mockWorkstreams: {
  reset(): void;                       // reset to the seed (tests call in beforeEach)
  state(): MockState;                  // inspect for assertions
  createWorkstream(id,name,goal,profile?): void;   // push; upsert by id
  deleteWorkstream(id): void;          // remove ws + its connections + membership entries
  addConnection(id,ws,from,to,kind,bi): void;
  removeConnection(id): void;
  createProfile(id,name): void;
  deleteProfile(id): void;             // remove; unset it on member workstreams; clear activeProfile if matched
  assignSession(sessionId,wsId,lead): void;   // set membership; the node's ws
  setActiveProfile(profile): void;
  board(): BoardView;                  // derive from state: cards for workstreams (filtered by activeProfile),
                                       //   summary counts, host chips from nodes, attention = needs/error/gated
  graph(wsId): GraphView | null;       // nodes-of-ws + connections-of-ws + inspector; null if ws unknown
};
```
Seed `reset()` with ≥2 workstreams (one single-node, one multi-node with a connection), ≥1 profile, a few
nodes across ≥2 hosts, so `board()`/`graph()` return realistic data immediately.

`board()`/`graph()` derivations (pragmatic, must be internally consistent so tests can assert changes):
- `board().all` = one `WorkstreamCard` per workstream passing the `activeProfile` filter; `summary` counts
  reflect state; `attention` = cards whose head-state is needs/error OR that have a gated connection.
- `graph(wsId)` = the ws's nodes (membership) mapped to graph nodes + its connections mapped to edges
  (status: gated→needs, bi→consulting, else normal), inspector working/needs counts + first gated handoff.

## Test plan (`tests/bridgeWorkstreams.test.ts`, Vitest; NO Tauri — pure mock path). Every `it(...)` real,
no `.skip`/`.todo`/`.only`. `beforeEach(() => mockWorkstreams.reset())`.
- BI1  seed: `fetchBoard()` returns the seeded workstreams; `fetchGraph(seededMultiWsId)` returns its nodes+edges;
       `fetchGraph("nope")` → null.
- BI2  `createWorkstream(...)` then `fetchBoard().all` contains the new card (name/goal/profile).
- BI3  `deleteWorkstream(id)` removes it from `fetchBoard().all` AND its connections vanish from its graph
       (fetchGraph → null or empty), and membership entries are gone.
- BI4  `addConnection(...)` then `fetchGraph(ws).edges` contains it (from/to/kind/status), and a gated
       connection yields `status:"needs"` and a non-null inspector `pending_handoff`.
- BI5  `removeConnection(id)` removes it from the graph.
- BI6  `assignSession(s, ws, true)` → the node appears in `fetchGraph(ws).nodes` with `lead:true`.
- BI7  `createProfile` + `setActiveProfile(p)` filters `fetchBoard().all` to that profile's workstreams;
       `setActiveProfile(null)` shows all. `deleteProfile(p)` unsets it on members + clears activeProfile.
- BI8  `attention` reflects a needs/error/gated workstream (construct one, assert it's in `board().attention`).
- BI9  api.ts wrappers in mock mode delegate to mockWorkstreams (call `api.createWorkstream(...)` then assert
       `mockWorkstreams.state()` changed) — proves the api layer routes to the backend, not a dead stub.
- BI10 `reset()` restores the seed (state deterministic across tests).

(The real Tauri `invoke(...)` path for these commands is covered by the Rust Tauri-commands block + the
Playwright e2e; this block proves the TS command surface + the mock that the app-wiring integration tests
will drive.)
