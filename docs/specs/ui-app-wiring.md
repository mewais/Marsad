# U-WIRE — Desktop app-wiring (Board/Graph connected + router + full-flow tests) — spec (`apps/desktop`)

**The gap-catcher.** This block connects the presentational `Board` and `GraphScreen` to the app's
state layer (`useVm`) and the top-level router (`App.tsx`), adds a live nav affordance, and proves the
**whole** Level-1→Level-2→Level-3 navigation flow works end-to-end against the mock backend. Every UI
control that this block surfaces MUST be hooked to a real `useVm` command/selector — no dead handlers.

Prereqs already landed: `useVm.ts` (board/graph state, actions, command helpers — U-INT-2), `Board.tsx`
(prop-driven `Board({board, onOpenWorkstream})`), `GraphScreen.tsx` (prop-driven, U2b), `api.ts` +
`mockWorkstreams.ts` (mock backend). This block does NOT reimplement any of those.

## Files this block creates
- `src/screens/BoardScreen.tsx` — connected wrapper.
- `src/screens/graph/GraphScreenConnected.tsx` — connected wrapper.
- `tests/appWiring.test.tsx` — full-flow integration suite (Vitest + testing-library).
- `tests/e2e/board-flow.spec.ts` — Playwright golden-path (best-effort; runs when a dev server is up).

## Files this block modifies (surgically — do NOT regress existing behavior/tests)
- `src/App.tsx` — add router cases `"board"` → `<BoardScreen/>`, `"graph"` → `<GraphScreenConnected/>`.
- `src/components/AppShell.tsx` — add a `nav-board` sidebar button that calls `openBoard()` from `useVm`
  and shows an active state when `state.screen === "board" || "graph"` (mirror the existing overview nav
  button's styling/pattern). Do NOT remove or rename existing nav controls.

## Connected wrappers (each consumes `useVm()`, passes props down; NO prop-drilling from App)

### `BoardScreen`
```ts
export function BoardScreen(): JSX.Element
```
- `const { state, openWorkstream } = useVm();`
- If `state.board == null` → render a `data-testid="board-loading"` placeholder (mount effect: the
  provider fetches the board, so this is transient).
- Else render `<Board board={state.board} onOpenWorkstream={openWorkstream} />`.
- `openWorkstream` is the real `useVm` helper (single-node → opens its session; multi-node → opens graph).

### `GraphScreenConnected`
```ts
export function GraphScreenConnected(): JSX.Element
```
- `const { state, selectNode, openNode } = useVm();`
- If `state.graph == null` → render `data-testid="graph-loading"` placeholder.
- Else render `<GraphScreen graph={state.graph} selectedNode={state.nodeSel} onSelectNode={selectNode}
  onOpenNode={openNode} onOpenGate={onOpenGate} />`.
- `onOpenGate(connId)`: look up the edge in `state.graph.edges` by id and open the **receiving** node's
  session — `const e = state.graph.edges.find(x => x.id === connId); if (e) openNode(e.to);`. (The
  dedicated handoff-approval modal is a later Wasl-integration block; interim wiring routes the gate
  affordance to the receiving session so the control is LIVE and testable, never a dead handler.)

## App.tsx router
Add to the `switch (state.screen)`:
- `case "board": return <BoardScreen />;`
- `case "graph": return <GraphScreenConnected />;`
Keep all existing cases + the `default`. Import the two wrappers. No other change.

## Test plan — `tests/appWiring.test.tsx` (Vitest + testing-library, render the REAL `<App/>`)
Render `<App/>` (which mounts `ThemeProvider > VmProvider > Router`). In test env `isTauri()` is false, so
`useVm` drives the **mock backend** (`mockWorkstreams`) — reset it in `beforeEach` (import its reset). Use
`findBy*`/`waitFor` for the async board/graph fetches. Real `it(...)`, NO `.skip`/`.todo`/`.only`, no
tautologies. Every assertion must exercise a real wired control.
- W1  App boots on the default screen; clicking `nav-board` shows the Board (`await findByTestId` a
      board-only testid, e.g. a `workstream-card-*` or the board summary) — proves the nav button is wired
      to `openBoard()` and the provider-fetched board renders.
- W2  **multi-node flow:** on the board, click the multi-node workstream card
      (`ws-build-pipeline`) → screen becomes Graph: a `graph-node-*` and `graph-canvas` render (the graph
      was fetched + routed). Proves board card click → `openWorkstream` → graph.
- W3  **single-node nav rule:** click the single-node workstream card (`ws-auth-refactor`) → goes straight
      to the **session** screen (a SessionDetail testid), NOT the graph. Proves the nav rule is honored
      end-to-end through the real router.
- W4  **graph → session:** from the graph (open the multi-node ws first), click a node's open affordance
      (`graph-node-open-{id}` / `node-open-{id}`) → session screen for that node. Proves `openNode` wiring.
- W5  **node selection:** click a graph node (not its open affordance) → that node gets
      `data-selected="true"` (selection round-trips through `selectNode` → `state.nodeSel` → prop), and a
      non-neighbour node gets `data-dimmed="true"`. Stays on the graph screen.
- W6  **gate affordance is live:** from the graph, click the `gate-diamond-{connId}` (or
      `inspector-handoff`) → the receiving node's **session** opens (the interim onOpenGate wiring). Proves
      the gate control is not a dead handler.
- W7  **loading placeholders:** rendering `GraphScreenConnected` with no open graph shows `graph-loading`;
      `BoardScreen` before the board resolves shows `board-loading` (drive via a bare provider or by
      asserting the transient state resolves to a board). At least assert the connected wrapper renders a
      placeholder when its slice is null and the real screen once populated.
- W8  **regression:** the existing default screen still renders (e.g. the overview/wordmark testid is
      present at boot) — this block didn't break the existing router.

Every navigation assertion must go through the REAL `useVm` helpers + router (no direct `dispatch` in the
test to force a screen) — that is the "100% UI hooked to functionality" guarantee. A wrapper that renders
the screen but leaves a callback unwired, or a nav button that dispatches nothing, must fail a test here.

## Test plan — `tests/e2e/board-flow.spec.ts` (Playwright, best-effort)
Mirror `tests/e2e/smoke.spec.ts`. Golden path against a running dev server (`webServer` is a TODO in
`playwright.config.ts`, so this may be skipped in CI — keep it self-contained and real):
- open `/`, click the board nav (`getByTestId("nav-board")`), expect a workstream card visible; click a
  multi-node card, expect the graph canvas visible; click a node, expect the session view.
Guard nothing behind `.skip`; if the server isn't up the whole file simply fails to connect (acceptable —
the Vitest integration is the enforced gate). Do not add flaky waits; use Playwright auto-waiting locators.
