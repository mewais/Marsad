# U2a — Desktop graph layout module (Level 2, by-host) — spec (additive, `apps/desktop`)

**Additive, pure module.** No React, no DOM, no elkjs — deterministic geometry over a `GraphView`,
Vitest-testable. Add ONE module + ONE test file. Do NOT modify existing files. The elkjs-based "Flow"
layout is a later block; this block is the hand-computed **By-host** layout (the design default).

Design source: `docs/design/DESIGN_HANDOFF.md` §"LEVEL 2 — Workstream Graph" (By-host layout + graph
metrics) and §"Design Tokens → Graph metrics". Types: import from
`apps/desktop/src/bridge/workstreamTypes.ts` (`GraphView`, `GraphNode`, `GraphEdge`, `EdgeStatus` — created
by the Board block).

## Files this block creates (ONLY these)
- `src/screens/graph/layout.ts`
- `tests/graphLayout.test.ts`

## Constants (design "Graph metrics")
`NODE_W = 190`, `NODE_H = 66`, `LANE_W = 222`, `LANE_GAP = 30`, `ROW_H = 132`, `STUB = 40` (by-host).

## Types + function (`layout.ts`)
```ts
import type { GraphView, GraphNode, GraphEdge, EdgeStatus } from "../../bridge/workstreamTypes";

export interface Point { x: number; y: number }
export interface PositionedNode { id: string; x: number; y: number; w: number; h: number; node: GraphNode }
export interface RoutedEdge {
  id: string; from: string; to: string;
  points: Point[];            // polyline key points: [sourceBottom, stubDown, stubUp, targetTop]
  gate: Point | null;         // gate-diamond midpoint when the edge has a gate, else null
  status: EdgeStatus;
}
export interface GraphLayout {
  hosts: string[];            // host columns, in first-appearance order among nodes
  nodes: PositionedNode[];
  edges: RoutedEdge[];
  width: number;
  height: number;
}

export function layoutByHost(graph: GraphView): GraphLayout;
```

### Semantics (deterministic)
- **Host columns:** `hosts` = distinct `node.host` values in first-appearance order across `graph.nodes`.
  Column index `col = hosts.indexOf(host)`. Lane left `laneX = col * (LANE_W + LANE_GAP)`.
- **Rows within a lane:** take that host's nodes, order by `(depth asc, then original graph.nodes order)`;
  the node's `row` = its index in that ordered list (guarantees no overlap even when two nodes share a
  depth). `y = row * ROW_H`.
- **Node position:** centered in its lane → `x = laneX + (LANE_W - NODE_W) / 2`; `w = NODE_W`, `h = NODE_H`.
  `nodes` is returned in the SAME order as `graph.nodes` (stable), each carrying its source `node`.
- **Edge routing** (per `graph.edges`, in input order): let A = source positioned node, B = target.
  - `sourceBottom = { x: A.x + NODE_W/2, y: A.y + NODE_H }`
  - `targetTop    = { x: B.x + NODE_W/2, y: B.y }`
  - `points = [ sourceBottom, { x: sourceBottom.x, y: sourceBottom.y + STUB }, { x: targetTop.x, y: targetTop.y - STUB }, targetTop ]`
  - `gate` = `edge.has_gate ? { x: (sourceBottom.x + targetTop.x)/2, y: (sourceBottom.y + targetTop.y)/2 } : null`
  - `status = edge.status` (passthrough).
  - If an edge references a node id not present in `graph.nodes`, SKIP it (do not emit; do not throw).
- **Canvas size:** `width = hosts.length > 0 ? hosts.length * (LANE_W + LANE_GAP) - LANE_GAP : 0`;
  `height = maxRows * ROW_H` where `maxRows` = the largest per-lane node count (0 if no nodes).

## Test plan (`tests/graphLayout.test.ts`, Vitest). Every test a real `it(...)`; NO `.skip`/`.todo`/`.only`,
no empty/tautological tests. Build small `GraphView` fixtures inline.
- GL1  host columns: 3 nodes on hosts [h1,h2,h1] → `hosts == ["h1","h2"]`; a node on h2 gets `x` in column 1
       (`x = 1*(LANE_W+LANE_GAP) + (LANE_W-NODE_W)/2`).
- GL2  rows by depth: two nodes same host, depths 0 and 1 → rows 0 and 1 (`y` = 0 and `ROW_H`).
- GL3  same-depth collision: two nodes same host same depth → distinct consecutive rows (no overlapping y).
- GL4  node centered in lane: `x == laneX + (LANE_W-NODE_W)/2`; `w==190`, `h==66`; `nodes` order matches input.
- GL5  edge routing points: for A→B, `points[0]` = A bottom-center, `points[3]` = B top-center, and the two
       middle points are the ±STUB stubs (assert exact coordinates).
- GL6  gate midpoint: a `has_gate` edge → `gate` = midpoint of sourceBottom/targetTop; a non-gated edge →
       `gate == null`.
- GL7  edge status passthrough (Normal/Consulting/Needs preserved).
- GL8  an edge referencing an unknown node id is skipped (not in `edges`), no throw.
- GL9  canvas size: `width` for N host columns = `N*(LANE_W+LANE_GAP)-LANE_GAP`; `height = maxRows*ROW_H`.
- GL10 empty graph (no nodes) → `hosts:[]`, `nodes:[]`, `edges:[]`, `width:0`, `height:0`, no throw.
