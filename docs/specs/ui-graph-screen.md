# U2b — Desktop GraphScreen (Level 2 render + interaction) — spec (`apps/desktop`)

**Additive UI block.** A prop-driven Graph canvas that renders a `GraphView` using the pure
`layoutByHost` geometry (from `src/screens/graph/layout.ts`) and fires callbacks on interaction. Testable
in isolation (Vitest), like `Board`. The app-wiring block connects it to `useVm`. Do NOT modify existing
files. elkjs "Flow" layout is a later block — this renders the **By-host** layout.

Design: `docs/design/DESIGN_HANDOFF.md` §"LEVEL 2 — Workstream Graph" (nodes, edges, gate diamond,
inspector). Tokens in `src/theme/tokens.ts`. Driver colors: claude `#d97757`, codex `#3fb6a8`, opencode
`#f0a050`. Edge status colors: normal=`--accent`, consulting=`--txt2`, needs=`--needs`.

## Files this block creates (ONLY these)
- `src/screens/graph/GraphScreen.tsx`
- `tests/GraphScreen.test.tsx`

## Component
```ts
import type { GraphView } from "../../bridge/workstreamTypes";
export function GraphScreen(props: {
  graph: GraphView;
  selectedNode: string | null;
  onSelectNode: (id: string | null) => void;   // click a node → select (neighbourhood highlight)
  onOpenNode: (id: string) => void;             // open a node's session
  onOpenGate: (connId: string) => void;         // click a gate diamond → open the handoff modal
}): JSX.Element
```
Uses `layoutByHost(props.graph)` for geometry. Renders, with stable `data-testid`s:
- `graph-canvas` container (width/height from the layout).
- For each host in `layout.hosts`: a `host-lane-{host}` swimlane column header showing the host id.
- For each positioned node: `graph-node-{id}` absolutely positioned at `(x,y)` with size `w×h`,
  `data-driver={driver}` (left-accent color = state color), `data-state={state}`, `data-selected`
  (true when `id === selectedNode`), showing the node name + a driver pill (`data-testid=driver-pill`,
  colored per driver) + status text. A working/needs node's state dot has `data-pulse="true"`.
  - Click a node → `onSelectNode(id)`. A second affordance `graph-node-open-{id}` (or double-role: the
    node's `›` chevron `node-open-{id}`) → `onOpenNode(id)`.
  - When `selectedNode` is set, non-neighbour nodes get `data-dimmed="true"` (opacity affordance); the
    selected node + its direct neighbours (via edges) are not dimmed.
- For each routed edge: `graph-edge-{id}` — an SVG `<path>` (from `edge.points`) with
  `data-status={status}` (color), `data-kind={kind}` (dash style is a visual detail), and if the edge has
  a gate, a `gate-diamond-{connId}` at `edge.gate` that calls `onOpenGate(connId)` on click.
- Inspector panel `graph-inspector`: workstream name/goal, a `inspector-working` count and
  `inspector-needs` count (from `graph.inspector`), and if `graph.inspector.pending_handoff` is set, a
  `inspector-handoff` callout (click → `onOpenGate(pending_handoff.conn_id)`), and a `Sessions` list where
  each `inspector-session-{id}` row → `onOpenNode(id)`.

Pure presentational: no `useVm`/Tauri. All from props.

## Test plan (`tests/GraphScreen.test.tsx`, Vitest + testing-library). Real `it(...)`, NO `.skip`/`.todo`/
`.only`. Build inline `GraphView` fixtures (2–3 nodes across 2 hosts, ≥1 edge, ≥1 gated edge).
- GS1  renders one `graph-node-{id}` per node, positioned per `layoutByHost` (assert style left/top match
       the layout's x/y for a known fixture).
- GS2  each node shows its name + a `driver-pill` with the driver color; `data-driver`/`data-state` set.
- GS3  clicking a node calls `onSelectNode(id)`; the node-open affordance calls `onOpenNode(id)`.
- GS4  when `selectedNode` is set, that node is `data-selected="true"` and a non-neighbour node is
       `data-dimmed="true"`; a neighbour (edge-connected) is NOT dimmed.
- GS5  one `graph-edge-{id}` per edge with `data-status` matching the edge status; a gated edge renders a
       `gate-diamond-{connId}`; clicking it calls `onOpenGate(connId)`.
- GS6  inspector shows working/needs counts from `graph.inspector`; a `pending_handoff` renders
       `inspector-handoff` and clicking it calls `onOpenGate` with the handoff conn id.
- GS7  inspector Sessions list: one row per node; clicking `inspector-session-{id}` calls `onOpenNode(id)`.
- GS8  working/needs node has `data-pulse="true"`; a done node does not.
