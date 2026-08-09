/**
 * Unit tests for layoutByHost — the pure, deterministic "By-host" geometry
 * layout for the Level 2 Workstream Graph.
 *
 * Spec: docs/specs/ui-graph-layout.md (§Test plan GL1-GL10). Constants:
 * NODE_W=190, NODE_H=66, LANE_W=222, LANE_GAP=30, ROW_H=132, STUB=40.
 *
 * This file is written test-first: `../src/screens/graph/layout` does not
 * exist yet, so this suite is expected to fail to resolve until the
 * code-agent lands `src/screens/graph/layout.ts`.
 */
import { describe, it, expect } from "vitest";
import { layoutByHost } from "../src/screens/graph/layout";
import type {
  GraphLayout,
  PositionedNode,
  RoutedEdge,
  Point,
} from "../src/screens/graph/layout";
import type {
  GraphView,
  GraphNode,
  GraphEdge,
  EdgeStatus,
} from "../src/bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// Geometry constants (mirrors docs/specs/ui-graph-layout.md §Constants)
// ---------------------------------------------------------------------------
const NODE_W = 190;
const NODE_H = 66;
const LANE_W = 222;
const LANE_GAP = 30;
const ROW_H = 132;
const STUB = 40;
// Reserved label band at the top of each lane (BUG #1) — every row is inset
// below it, and it is added to the lane's total height.
const LANE_LABEL_H = 34;

const laneX = (col: number): number => col * (LANE_W + LANE_GAP);
const centeredX = (col: number): number => laneX(col) + (LANE_W - NODE_W) / 2;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/** Build a minimal GraphNode, filling in unused fields with harmless defaults. */
function node(over: Partial<GraphNode> & Pick<GraphNode, "id" | "host">): GraphNode {
  return {
    id: over.id,
    name: over.name ?? over.id,
    driver: over.driver ?? "claude",
    host: over.host,
    state: over.state ?? "Working",
    status: over.status ?? "",
    context_pct: over.context_pct ?? 0,
    is_lead: over.is_lead ?? false,
    depth: over.depth ?? 0,
  };
}

/** Build a minimal GraphEdge, filling in unused fields with harmless defaults. */
function edge(over: Partial<GraphEdge> & Pick<GraphEdge, "id" | "from" | "to">): GraphEdge {
  return {
    id: over.id,
    from: over.from,
    to: over.to,
    bi: over.bi ?? false,
    status: over.status ?? "normal",
    has_gate: over.has_gate ?? false,
  };
}

function graph(nodes: GraphNode[], edges: GraphEdge[] = []): GraphView {
  return {
    ws_id: "ws1",
    name: "Workstream 1",
    goal: "goal",
    nodes,
    edges,
    inspector: { working: 0, needs: 0, pending_handoff: null },
  };
}

function findNode(layout: GraphLayout, id: string): PositionedNode {
  const found = layout.nodes.find((n) => n.id === id);
  if (!found) throw new Error(`positioned node ${id} not found in layout`);
  return found;
}

function findEdge(layout: GraphLayout, id: string): RoutedEdge {
  const found = layout.edges.find((e) => e.id === id);
  if (!found) throw new Error(`routed edge ${id} not found in layout`);
  return found;
}

// ---------------------------------------------------------------------------
// GL1 — host columns
// ---------------------------------------------------------------------------
describe("layoutByHost — GL1 host columns", () => {
  it("orders hosts by first appearance and places a column-1 node's x accordingly", () => {
    const g = graph([
      node({ id: "n1", host: "h1" }),
      node({ id: "n2", host: "h2" }),
      node({ id: "n3", host: "h1" }),
    ]);
    const layout = layoutByHost(g);

    expect(layout.hosts).toEqual(["h1", "h2"]);

    const n2 = findNode(layout, "n2");
    expect(n2.x).toBe(1 * (LANE_W + LANE_GAP) + (LANE_W - NODE_W) / 2);
    expect(n2.x).toBe(268);
  });
});

// ---------------------------------------------------------------------------
// GL2 — rows by depth
// ---------------------------------------------------------------------------
describe("layoutByHost — GL2 rows by depth", () => {
  it("assigns row 0 and row 1 (inset below the label band) to depths 0 and 1 on the same host", () => {
    const g = graph([
      node({ id: "a", host: "h1", depth: 0 }),
      node({ id: "b", host: "h1", depth: 1 }),
    ]);
    const layout = layoutByHost(g);

    const a = findNode(layout, "a");
    const b = findNode(layout, "b");
    // Rows sit at LANE_LABEL_H + row * ROW_H (BUG #1 reserved label band).
    expect(a.y).toBe(LANE_LABEL_H);
    expect(b.y).toBe(LANE_LABEL_H + ROW_H);
    expect(b.y).toBe(34 + 132);
  });
});

// ---------------------------------------------------------------------------
// GL3 — same-depth collision
// ---------------------------------------------------------------------------
describe("layoutByHost — GL3 same-depth collision stacking", () => {
  it("gives two same-host same-depth nodes distinct consecutive rows", () => {
    const g = graph([
      node({ id: "a", host: "h1", depth: 2 }),
      node({ id: "b", host: "h1", depth: 2 }),
    ]);
    const layout = layoutByHost(g);

    const a = findNode(layout, "a");
    const b = findNode(layout, "b");

    // Original graph.nodes order breaks the tie: a (index 0) comes first.
    // Rows are inset below the reserved label band (BUG #1).
    expect(a.y).toBe(LANE_LABEL_H);
    expect(b.y).toBe(LANE_LABEL_H + ROW_H);
    expect(a.y).not.toBe(b.y);
  });
});

// ---------------------------------------------------------------------------
// GL4 — node centering, size, and output order
// ---------------------------------------------------------------------------
describe("layoutByHost — GL4 node centering and stable order", () => {
  it("centers each node in its lane, fixes w/h, and preserves input node order", () => {
    const g = graph([
      node({ id: "z", host: "h2" }),
      node({ id: "a", host: "h1" }),
    ]);
    const layout = layoutByHost(g);

    const z = findNode(layout, "z");
    const a = findNode(layout, "a");

    // Host columns are FIRST-APPEARANCE order (consistent with the board's
    // host_chips): z is on h2 (first in graph.nodes) → h2 is column 0; a is on
    // h1 (second) → h1 is column 1.
    expect(z.x).toBe(centeredX(0));
    expect(z.x).toBe(16);
    expect(a.x).toBe(centeredX(1));
    expect(a.x).toBe(268);

    for (const n of layout.nodes) {
      expect(n.w).toBe(190);
      expect(n.h).toBe(66);
    }

    // "nodes is returned in the SAME order as graph.nodes"
    expect(layout.nodes.map((n) => n.id)).toEqual(["z", "a"]);
  });
});

// ---------------------------------------------------------------------------
// GL5 — edge routing points
// ---------------------------------------------------------------------------
describe("layoutByHost — GL5 edge routing points", () => {
  it("computes source bottom-center, target top-center, and the ±STUB stub points", () => {
    const g = graph(
      [
        node({ id: "A", host: "h1", depth: 0 }),
        node({ id: "B", host: "h2", depth: 0 }),
      ],
      [edge({ id: "e1", from: "A", to: "B" })],
    );
    const layout = layoutByHost(g);

    const A = findNode(layout, "A");
    const B = findNode(layout, "B");
    const e1 = findEdge(layout, "e1");

    const sourceBottom: Point = { x: A.x + NODE_W / 2, y: A.y + NODE_H };
    const targetTop: Point = { x: B.x + NODE_W / 2, y: B.y };
    const stubDown: Point = { x: sourceBottom.x, y: sourceBottom.y + STUB };
    const stubUp: Point = { x: targetTop.x, y: targetTop.y - STUB };

    expect(e1.points).toHaveLength(4);
    expect(e1.points[0]).toEqual(sourceBottom);
    expect(e1.points[1]).toEqual(stubDown);
    expect(e1.points[2]).toEqual(stubUp);
    expect(e1.points[3]).toEqual(targetTop);

    // Exact numeric values for this fixture (rows inset by LANE_LABEL_H=34, BUG #1):
    // A is at col 0 -> x=16, row 0 -> y=34. B is at col 1 -> x=268, row 0 -> y=34.
    expect(e1.points[0]).toEqual({ x: 16 + 95, y: 34 + 66 }); // { x: 111, y: 100 }
    expect(e1.points[1]).toEqual({ x: 111, y: 140 }); // 100 + STUB(40)
    expect(e1.points[2]).toEqual({ x: 268 + 95, y: 34 - 40 }); // { x: 363, y: -6 }
    expect(e1.points[3]).toEqual({ x: 363, y: 34 });
  });
});

// ---------------------------------------------------------------------------
// GL6 — gate midpoint
// ---------------------------------------------------------------------------
describe("layoutByHost — GL6 gate midpoint", () => {
  it("computes the gate as the sourceBottom/targetTop midpoint when has_gate is true", () => {
    const g = graph(
      [
        node({ id: "A", host: "h1", depth: 0 }),
        node({ id: "B", host: "h2", depth: 0 }),
      ],
      [edge({ id: "e1", from: "A", to: "B", has_gate: true })],
    );
    const layout = layoutByHost(g);
    const e1 = findEdge(layout, "e1");

    // sourceBottom = {111, 100}, targetTop = {363, 34} (see GL5 for derivation).
    expect(e1.gate).toEqual({ x: (111 + 363) / 2, y: (100 + 34) / 2 });
    expect(e1.gate).toEqual({ x: 237, y: 67 });
  });

  it("returns gate: null for a non-gated edge", () => {
    const g = graph(
      [
        node({ id: "A", host: "h1", depth: 0 }),
        node({ id: "B", host: "h2", depth: 0 }),
      ],
      [edge({ id: "e1", from: "A", to: "B", has_gate: false })],
    );
    const layout = layoutByHost(g);
    const e1 = findEdge(layout, "e1");

    expect(e1.gate).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// GL7 — edge status passthrough
// ---------------------------------------------------------------------------
describe("layoutByHost — GL7 edge status passthrough", () => {
  it("preserves normal, consulting, and needs edge statuses", () => {
    const statuses: EdgeStatus[] = ["normal", "consulting", "needs"];
    const g = graph(
      [
        node({ id: "A", host: "h1", depth: 0 }),
        node({ id: "B", host: "h2", depth: 0 }),
      ],
      statuses.map((status, i) =>
        edge({ id: `e${i}`, from: "A", to: "B", status }),
      ),
    );
    const layout = layoutByHost(g);

    expect(findEdge(layout, "e0").status).toBe("normal");
    expect(findEdge(layout, "e1").status).toBe("consulting");
    expect(findEdge(layout, "e2").status).toBe("needs");
  });
});

// ---------------------------------------------------------------------------
// GL8 — unknown-endpoint edges are skipped, not thrown
// ---------------------------------------------------------------------------
describe("layoutByHost — GL8 unknown-endpoint edge skipped", () => {
  it("omits an edge whose from/to references a missing node id, without throwing", () => {
    const g = graph(
      [node({ id: "A", host: "h1", depth: 0 })],
      [
        edge({ id: "bad-to", from: "A", to: "does-not-exist" }),
        edge({ id: "bad-from", from: "does-not-exist", to: "A" }),
      ],
    );

    let layout: GraphLayout | undefined;
    expect(() => {
      layout = layoutByHost(g);
    }).not.toThrow();

    expect(layout!.edges).toEqual([]);
    expect(layout!.edges.find((e) => e.id === "bad-to")).toBeUndefined();
    expect(layout!.edges.find((e) => e.id === "bad-from")).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// GL9 — canvas size
// ---------------------------------------------------------------------------
describe("layoutByHost — GL9 canvas size", () => {
  it("computes width from host column count and height from the max per-lane row count", () => {
    // 3 hosts; h1 has 2 nodes (maxRows), h2 and h3 have 1 each.
    const g = graph([
      node({ id: "a1", host: "h1", depth: 0 }),
      node({ id: "a2", host: "h1", depth: 1 }),
      node({ id: "b1", host: "h2", depth: 0 }),
      node({ id: "c1", host: "h3", depth: 0 }),
    ]);
    const layout = layoutByHost(g);

    expect(layout.hosts).toEqual(["h1", "h2", "h3"]);
    expect(layout.width).toBe(3 * (LANE_W + LANE_GAP) - LANE_GAP);
    expect(layout.width).toBe(726);
    // Height covers the reserved label band + rows (BUG #1).
    expect(layout.height).toBe(LANE_LABEL_H + 2 * ROW_H);
    expect(layout.height).toBe(298);
  });

  it("computes width for a two-host graph as N*(LANE_W+LANE_GAP)-LANE_GAP", () => {
    const g = graph([
      node({ id: "a1", host: "h1" }),
      node({ id: "b1", host: "h2" }),
    ]);
    const layout = layoutByHost(g);

    expect(layout.width).toBe(2 * (LANE_W + LANE_GAP) - LANE_GAP);
    expect(layout.width).toBe(474);
    // Height covers the reserved label band + rows (BUG #1).
    expect(layout.height).toBe(LANE_LABEL_H + 1 * ROW_H);
    expect(layout.height).toBe(166);
  });
});

// ---------------------------------------------------------------------------
// GL10 — empty graph
// ---------------------------------------------------------------------------
describe("layoutByHost — GL10 empty graph", () => {
  it("returns empty hosts/nodes/edges and zeroed width/height without throwing", () => {
    const g = graph([], []);

    let layout: GraphLayout | undefined;
    expect(() => {
      layout = layoutByHost(g);
    }).not.toThrow();

    expect(layout!.hosts).toEqual([]);
    expect(layout!.nodes).toEqual([]);
    expect(layout!.edges).toEqual([]);
    expect(layout!.width).toBe(0);
    expect(layout!.height).toBe(0);
  });
});
