# U-INT-2 — Desktop useVm workstream integration — spec (`apps/desktop`)

**Additive extension of `src/bridge/useVm.tsx`** (the app's state layer). Adds board/graph navigation,
workstream/graph state, handling of the new backend events, and **command helpers** on the context that call
`api.ts` and update state — so screens dispatch real backend operations, not dead handlers. Do NOT break
existing state/actions/selectors/tests. Uses the `api.ts` wrappers + `mockWorkstreams` from the bridge block.

## New state (add to `VmState`)
```ts
board: BoardView | null;        // latest board (from a "board" VmUpdate + initial fetch)
wsId: string | null;            // current workstream (Level 2/3)
graph: GraphView | null;        // graph for wsId (fetched on open)
nodeSel: string | null;         // selected node id in the graph
activeProfile: string | null;   // board profile filter
```
`Screen` gains `"board"` and `"graph"` (keep existing values).

## New actions (add to the `Action` union + reducer; existing actions unchanged)
- `{ type: "SET_BOARD"; board: BoardView }` — reducer: `board = action.board`.
- `{ type: "SET_GRAPH"; wsId: string; graph: GraphView | null }` — `wsId, graph`.
- `{ type: "OPEN_BOARD" }` — `screen = "board"`, clear `wsId/graph/nodeSel`.
- `{ type: "OPEN_GRAPH"; wsId: string }` — `screen = "graph"`, `wsId`, clear `nodeSel`.
- `{ type: "SELECT_NODE"; id: string | null }` — `nodeSel = id`.
- `{ type: "SET_ACTIVE_PROFILE"; profile: string | null }` — `activeProfile`.
- `VM_UPDATE` handling: `{kind:"board"}` → set `board`; `{kind:"workstreams"}` → (no direct state field
  needed beyond board; may be ignored or trigger a board refresh — see helpers). Do NOT regress the
  existing VM_UPDATE cases.

## Command helpers (add to the context value returned by `useVm()`)
Each: call the `api.ts` wrapper, then refresh (`fetchBoard()` → `SET_BOARD`; if a `wsId` is open,
`fetchGraph(wsId)` → `SET_GRAPH`). This keeps the UI in sync in mock mode; in Tauri mode the core's
Board/Workstreams events also arrive.
- `openBoard(): void` — dispatch OPEN_BOARD + `fetchBoard()`→SET_BOARD.
- `openWorkstream(id): Promise<void>` — **nav rule:** look up the card in `state.board?.all`; if
  `card.is_single_node` is true, open its session directly (find the single node via `fetchGraph(id)`,
  `SELECT_SESSION` that node id → screen "session"); else `fetchGraph(id)`→SET_GRAPH + OPEN_GRAPH(id).
- `selectNode(id): void` — SELECT_NODE (and, in graph, may open the session on a second concern — keep it
  to SELECT_NODE here; screen opens session via a separate `openNode`).
- `openNode(sessionId): void` — SELECT_SESSION(sessionId) (→ screen "session").
- `createWorkstream(id,name,goal,profile?): Promise<void>` — `api.createWorkstream(...)` then refresh board.
- `deleteWorkstream(id)`, `addConnection(...)`, `removeConnection(id)`, `createProfile(id,name)`,
  `deleteProfile(id)`, `assignSession(sessionId,wsId,lead)` — each: call the api wrapper then refresh
  board (+ graph if `wsId` open).
- `setActiveProfile(profile): Promise<void>` — `api.setActiveProfile(profile)` +
  `SET_ACTIVE_PROFILE` + refresh board.

## VmProvider wiring
On mount, after the existing snapshot load, also `fetchBoard()` → `SET_BOARD` so the board has data. Keep
the existing `listenVmUpdate` handling; ensure the new `board`/`workstreams` events dispatch VM_UPDATE.

## Test plan (`tests/useVmWorkstreams.test.tsx`, Vitest + testing-library; mirror the render/dispatch style
of the existing useVm-driven tests). Real `it(...)`s, NO `.skip`/`.todo`/`.only`. Use a small harness that
renders a `VmProvider` and a probe component calling `useVm()`, OR test the reducer + helpers directly.
Reset `mockWorkstreams` in `beforeEach`.
- V1  reducer: SET_BOARD sets `board`; OPEN_GRAPH sets screen "graph" + wsId + clears nodeSel; OPEN_BOARD
      clears wsId/graph/nodeSel + screen "board"; SELECT_NODE sets nodeSel; SET_ACTIVE_PROFILE sets it.
- V2  reducer: existing actions (SET_SCREEN, SELECT_SESSION, VM_UPDATE session/hosts/etc.) still behave
      (regression guard) — a couple of assertions that pre-existing behavior is intact.
- V3  VM_UPDATE `{kind:"board", board}` updates `state.board`; a `{kind:"session"}` event still updates a
      session (no regression).
- V4  `createWorkstream` helper calls `api.createWorkstream` (spy) AND, after, `state.board.all` contains
      the new workstream (board refreshed). (Proves the helper is wired end-to-end via the mock.)
- V5  `addConnection` helper → after, the open graph reflects the new edge (open a ws first, then add).
- V6  `openWorkstream(multiNodeWsId)` → screen "graph", wsId set, `graph` populated. 
- V7  `openWorkstream(singleNodeWsId)` → screen "session" (nav rule: single-node opens its session), not
      "graph".
- V8  `setActiveProfile(p)` → `state.activeProfile == p` AND `state.board.all` reflects the profile filter
      (board refreshed via the mock).
- V9  `deleteWorkstream` helper → after, board no longer lists it.
- V10 `assignSession` / `createProfile` / `deleteProfile` helpers each call their api wrapper (spy) and
      refresh board (assert the state change).

Every command helper MUST be proven to (a) call the api layer and (b) result in updated state — that is the
"UI hooked to functionality" guarantee. A helper that dispatches but never calls api, or calls api but never
refreshes, must fail a test.
