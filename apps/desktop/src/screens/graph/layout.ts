/**
 * Pure, deterministic "By-host" geometry layout for the Level 2 Workstream
 * Graph. No React, no DOM, no elkjs — just geometry computed over a
 * `GraphView`.
 *
 * Spec: docs/specs/ui-graph-layout.md
 */
import type { GraphView, GraphNode, EdgeStatus } from "../../bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// Constants (design "Graph metrics")
// ---------------------------------------------------------------------------
export const NODE_W = 190;
export const NODE_H = 66;
export const LANE_W = 222;
export const LANE_GAP = 30;
export const ROW_H = 132;
export const STUB = 40;
/**
 * Vertical space RESERVED at the top of each host swimlane for its group/host
 * label (the "▪ host" chip GraphScreen renders inside the lane). Node rows are
 * inset below this so the first row never overlaps the label (BUG #1 — the label
 * used to be covered by the top row, with the equivalent slack sitting UNDER the
 * group instead). The lane's total height includes this inset, so the reserved
 * band is ABOVE the child nodes with no dead gap below them.
 */
export const LANE_LABEL_H = 34;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------
export interface Point {
  x: number;
  y: number;
}

export interface PositionedNode {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  node: GraphNode;
}

export interface RoutedEdge {
  id: string;
  from: string;
  to: string;
  points: Point[]; // polyline key points: [sourceBottom, stubDown, stubUp, targetTop]
  gate: Point | null; // gate-diamond midpoint when the edge has a gate, else null
  status: EdgeStatus;
}

export interface GraphLayout {
  hosts: string[]; // host columns, in first-appearance order among nodes
  nodes: PositionedNode[];
  edges: RoutedEdge[];
  width: number;
  height: number;
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------
export function layoutByHost(graph: GraphView): GraphLayout {
  const nodes = graph.nodes;

  // Host columns: distinct host values in FIRST-APPEARANCE order among the
  // nodes — matching the board's host_chips (marsad-viewmodel) so the same
  // workstream shows its hosts in the same order in the board and the graph.
  const hosts: string[] = [];
  for (const n of nodes) {
    if (!hosts.includes(n.host)) {
      hosts.push(n.host);
    }
  }

  // Rows within a lane: for each host, order that host's nodes by
  // (depth asc, then original graph.nodes order), row = index in that list.
  const rowById = new Map<string, number>();
  let maxRows = 0;
  for (const host of hosts) {
    const hostNodes = nodes
      .map((n, idx) => ({ n, idx }))
      .filter((entry) => entry.n.host === host)
      .sort((a, b) => a.n.depth - b.n.depth || a.idx - b.idx);
    hostNodes.forEach((entry, row) => {
      rowById.set(entry.n.id, row);
    });
    if (hostNodes.length > maxRows) {
      maxRows = hostNodes.length;
    }
  }

  // Position nodes, preserving graph.nodes order in the output.
  const positionedById = new Map<string, PositionedNode>();
  const positionedNodes: PositionedNode[] = nodes.map((n) => {
    const col = hosts.indexOf(n.host);
    const row = rowById.get(n.id) ?? 0;
    const laneX = col * (LANE_W + LANE_GAP);
    const x = laneX + (LANE_W - NODE_W) / 2;
    // Inset every row below the reserved label band so the first row clears the
    // lane's host label (BUG #1).
    const y = LANE_LABEL_H + row * ROW_H;
    const positioned: PositionedNode = { id: n.id, x, y, w: NODE_W, h: NODE_H, node: n };
    positionedById.set(n.id, positioned);
    return positioned;
  });

  // Route edges, skipping ones with unknown endpoints.
  const routedEdges: RoutedEdge[] = [];
  for (const e of graph.edges) {
    const A = positionedById.get(e.from);
    const B = positionedById.get(e.to);
    if (!A || !B) {
      continue;
    }
    const sourceBottom: Point = { x: A.x + NODE_W / 2, y: A.y + NODE_H };
    const targetTop: Point = { x: B.x + NODE_W / 2, y: B.y };
    const stubDown: Point = { x: sourceBottom.x, y: sourceBottom.y + STUB };
    const stubUp: Point = { x: targetTop.x, y: targetTop.y - STUB };
    const gate: Point | null = e.has_gate
      ? { x: (sourceBottom.x + targetTop.x) / 2, y: (sourceBottom.y + targetTop.y) / 2 }
      : null;

    routedEdges.push({
      id: e.id,
      from: e.from,
      to: e.to,
      points: [sourceBottom, stubDown, stubUp, targetTop],
      gate,
      status: e.status,
    });
  }

  const width = hosts.length > 0 ? hosts.length * (LANE_W + LANE_GAP) - LANE_GAP : 0;
  // Total height covers the reserved label band plus the node rows (the label
  // band sits ABOVE the rows — no extra gap is added below them).
  const height = maxRows > 0 ? LANE_LABEL_H + maxRows * ROW_H : 0;

  return {
    hosts,
    nodes: positionedNodes,
    edges: routedEdges,
    width,
    height,
  };
}

// ---------------------------------------------------------------------------
// "Flow" layout (elkjs) — layered DAG, direction DOWN, spline routing.
//
// Split into an impure/async half (`computeElkLayout`, which loads elkjs and
// runs its layered algorithm) and a pure/sync half (`buildFlowLayout`, which
// turns elkjs's raw positions/routes into the SAME `GraphLayout` shape
// `layoutByHost` produces, so `GraphScreen` can render either mode uniformly).
// `buildFlowLayout` is the one geometry unit tests exercise directly (with a
// hand-built `ElkRawLayout` fixture) — no elkjs / async needed to test the
// math. Design: docs/design/DESIGN_HANDOFF.md §"Graph rendering" (Flow) +
// §"Tech notes" (elkjs, direction DOWN, spline routing, crossing-min).
// ---------------------------------------------------------------------------

/** Vertical stub length entering/leaving a node in Flow mode (design "Graph
 *  metrics": 24px Flow vs 40px By-host). */
export const FLOW_STUB = 24;

/** elkjs layered-DAG options mirroring the design reference's `computeElk()`
 *  (Marsad Workstreams.dc.html ~line 907): direction DOWN, spline edge
 *  routing, generous node/layer spacing so labels don't collide. */
const ELK_LAYOUT_OPTIONS: Record<string, string> = {
  "elk.algorithm": "layered",
  "elk.direction": "DOWN",
  "elk.edgeRouting": "SPLINES",
  "elk.layered.spacing.nodeNodeBetweenLayers": "78",
  "elk.spacing.nodeNode": "48",
  "elk.layered.spacing.edgeNodeBetweenLayers": "26",
  "elk.layered.crossingMinimization.semiInteractive": "false",
};

export interface ElkRawNode {
  id: string;
  x: number;
  y: number;
}

export interface ElkRawEdge {
  id: string;
  /** Absolute canvas-space points: [start, ...bendPoints, end]. Empty when
   *  elkjs produced no routable section for this edge. */
  points: Point[];
}

export interface ElkRawLayout {
  nodes: ElkRawNode[];
  edges: ElkRawEdge[];
  width: number;
  height: number;
}

/**
 * Run elkjs's layered-DAG algorithm over a graph's nodes/edges. Impure
 * (dynamic-imports elkjs, awaits its async `layout()`). Edges referencing an
 * unknown node id are skipped (same contract as `layoutByHost`), never
 * thrown. Returns a raw layout with NO fallback geometry — callers decide
 * what to render while this is in flight / if it rejects.
 */
export async function computeElkLayout(graph: GraphView): Promise<ElkRawLayout> {
  const nodeIds = new Set(graph.nodes.map((n) => n.id));
  const validEdges = graph.edges.filter((e) => nodeIds.has(e.from) && nodeIds.has(e.to));

  const elkGraph = {
    id: "root",
    layoutOptions: ELK_LAYOUT_OPTIONS,
    children: graph.nodes.map((n) => ({ id: n.id, width: NODE_W, height: NODE_H })),
    edges: validEdges.map((e) => ({ id: e.id, sources: [e.from], targets: [e.to] })),
  };

  // Import elkjs's plain `elk-api` entry point directly, NOT the package
  // root (`elkjs` → `lib/main.js`). `main.js` wraps elk-api with optional
  // Web Worker support via a guarded `require('web-worker')` — harmless in
  // Node, but bundlers that hoist CJS `require()` calls into static ESM
  // imports (Rollup/Vite's commonjs interop) turn that guarded require into
  // an eager import that 404s in the browser (no such bare module exists
  // there). `elk-api`'s bare `ELK` constructor REQUIRES an explicit
  // `workerFactory` though (it has no built-in fallback), so supply the same
  // in-process fake-Worker `main.js` would have defaulted to
  // (`elk-worker.min.js`) ourselves — self-contained, no optional deps, runs
  // the layered algorithm synchronously in this thread either way.
  const [{ default: ElkCtor }, { Worker: ElkWorker }] = await Promise.all([
    import("elkjs/lib/elk-api.js") as Promise<{
      default: new (opts: {
        workerFactory: (url: string) => unknown;
      }) => {
        layout: (g: unknown) => Promise<{
          children?: { id: string; x?: number; y?: number }[];
          edges?: { id: string; sections?: { startPoint: Point; bendPoints?: Point[]; endPoint: Point }[] }[];
          width?: number;
          height?: number;
        }>;
      };
    }>,
    import("elkjs/lib/elk-worker.min.js") as Promise<{ Worker: new (url: string) => unknown }>,
  ]);
  const elk = new ElkCtor({ workerFactory: (url: string) => new ElkWorker(url) });
  const result = await elk.layout(elkGraph);

  const nodes: ElkRawNode[] = (result.children ?? []).map((c) => ({
    id: c.id,
    x: c.x ?? 0,
    y: c.y ?? 0,
  }));

  const edges: ElkRawEdge[] = (result.edges ?? []).map((e) => {
    const section = (e.sections ?? [])[0];
    if (!section) return { id: e.id, points: [] };
    return {
      id: e.id,
      points: [section.startPoint, ...(section.bendPoints ?? []), section.endPoint],
    };
  });

  return { nodes, edges, width: result.width ?? 0, height: result.height ?? 0 };
}

/**
 * Build a `GraphLayout` from elkjs's raw per-node positions + edge routes.
 * Pure/synchronous — the actual geometry math is tested here, against a
 * hand-built `ElkRawLayout`, without running elkjs.
 *
 * - Node position = elk's `(x, y)` verbatim (falls back to `{0,0}` if elkjs
 *   didn't return a position for that id — never throws).
 * - Edge points = `[sourceBottom, stubDown, ...elk bend points, stubUp,
 *   targetTop]` (same bottom-of-source / top-of-target anchoring as
 *   `layoutByHost`, but with the shorter `FLOW_STUB`).
 * - Gate midpoint = the `sourceBottom`/`targetTop` midpoint (same semantics
 *   as `layoutByHost`'s GL6, independent of interior bend points).
 * - `hosts` = distinct `node.host`, first-appearance order (unused for
 *   swimlanes in Flow — Flow has no host lanes — but each node still carries
 *   its `host` for the per-node host chip).
 */
export function buildFlowLayout(graph: GraphView, elk: ElkRawLayout): GraphLayout {
  const hosts: string[] = [];
  for (const n of graph.nodes) {
    if (!hosts.includes(n.host)) hosts.push(n.host);
  }

  const posById = new Map(elk.nodes.map((n) => [n.id, n]));
  const positionedById = new Map<string, PositionedNode>();
  const nodes: PositionedNode[] = graph.nodes.map((n) => {
    const p = posById.get(n.id);
    const positioned: PositionedNode = {
      id: n.id,
      x: p?.x ?? 0,
      y: p?.y ?? 0,
      w: NODE_W,
      h: NODE_H,
      node: n,
    };
    positionedById.set(n.id, positioned);
    return positioned;
  });

  const elkEdgeById = new Map(elk.edges.map((e) => [e.id, e]));
  const edges: RoutedEdge[] = [];
  for (const e of graph.edges) {
    const A = positionedById.get(e.from);
    const B = positionedById.get(e.to);
    if (!A || !B) continue;

    const sourceBottom: Point = { x: A.x + NODE_W / 2, y: A.y + NODE_H };
    const targetTop: Point = { x: B.x + NODE_W / 2, y: B.y };
    const raw = elkEdgeById.get(e.id);
    const bends = raw && raw.points.length >= 2 ? raw.points.slice(1, -1) : [];

    const points: Point[] = [
      sourceBottom,
      { x: sourceBottom.x, y: sourceBottom.y + FLOW_STUB },
      ...bends,
      { x: targetTop.x, y: targetTop.y - FLOW_STUB },
      targetTop,
    ];

    const gate: Point | null = e.has_gate
      ? { x: (sourceBottom.x + targetTop.x) / 2, y: (sourceBottom.y + targetTop.y) / 2 }
      : null;

    edges.push({ id: e.id, from: e.from, to: e.to, points, gate, status: e.status });
  }

  return {
    hosts,
    nodes,
    edges,
    width: Math.max(elk.width, 1),
    height: Math.max(elk.height, 1),
  };
}

/**
 * Smoothed quadratic-midpoint spline through a polyline's interior points
 * (design: "smoothed spline interior" for both layouts' edge geometry). The
 * first and last segments stay straight lines into the fixed stub endpoints;
 * each interior vertex becomes a quadratic control point curving toward the
 * midpoint of itself and its successor — the same construction as the design
 * reference's `smoothPath()` (Marsad Workstreams.dc.html ~line 889).
 */
export function smoothEdgePath(points: Point[]): string {
  if (points.length === 0) return "";
  if (points.length === 1) return `M ${points[0].x} ${points[0].y}`;
  if (points.length === 2) {
    return `M ${points[0].x} ${points[0].y} L ${points[1].x} ${points[1].y}`;
  }
  let d = `M ${points[0].x} ${points[0].y}`;
  for (let i = 1; i < points.length - 1; i++) {
    const mx = (points[i].x + points[i + 1].x) / 2;
    const my = (points[i].y + points[i + 1].y) / 2;
    d += ` Q ${points[i].x} ${points[i].y}, ${mx} ${my}`;
  }
  d += ` L ${points[points.length - 1].x} ${points[points.length - 1].y}`;
  return d;
}
