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
    const y = row * ROW_H;
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
  const height = maxRows * ROW_H;

  return {
    hosts,
    nodes: positionedNodes,
    edges: routedEdges,
    width,
    height,
  };
}
