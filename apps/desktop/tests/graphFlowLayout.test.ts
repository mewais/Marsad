/**
 * Unit tests for the Flow-layout geometry: `buildFlowLayout` (pure, exercised
 * against a hand-built `ElkRawLayout` — no elkjs needed for the math), plus a
 * smoke test that `computeElkLayout` (the real elkjs-backed async half)
 * actually runs the layered-DAG algorithm without throwing.
 *
 * Spec: docs/design/DESIGN_HANDOFF.md §"Tech notes" (elkjs, direction DOWN,
 * spline routing) + §"Graph metrics" (FLOW_STUB = 24px).
 */
import { describe, it, expect } from "vitest";
import {
  buildFlowLayout,
  computeElkLayout,
  smoothEdgePath,
  FLOW_STUB,
  NODE_W,
  NODE_H,
} from "../src/screens/graph/layout";
import type { ElkRawLayout } from "../src/screens/graph/layout";
import type { GraphNode, GraphEdge, GraphView, EdgeStatus } from "../src/bridge/workstreamTypes";

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

function edge(over: Partial<GraphEdge> & Pick<GraphEdge, "id" | "from" | "to">): GraphEdge {
  return {
    id: over.id,
    from: over.from,
    to: over.to,
    kind: over.kind ?? "data",
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

// ---------------------------------------------------------------------------
// buildFlowLayout — pure geometry over a hand-built ElkRawLayout
// ---------------------------------------------------------------------------

describe("buildFlowLayout — node positions come straight from elk", () => {
  it("places each node at elk's (x, y), sized NODE_W x NODE_H", () => {
    const g = graph([node({ id: "A", host: "h1" }), node({ id: "B", host: "h2" })]);
    const elk: ElkRawLayout = {
      nodes: [
        { id: "A", x: 10, y: 20 },
        { id: "B", x: 210, y: 130 },
      ],
      edges: [],
      width: 500,
      height: 300,
    };
    const layout = buildFlowLayout(g, elk);

    const a = layout.nodes.find((n) => n.id === "A")!;
    const b = layout.nodes.find((n) => n.id === "B")!;
    expect(a.x).toBe(10);
    expect(a.y).toBe(20);
    expect(a.w).toBe(NODE_W);
    expect(a.h).toBe(NODE_H);
    expect(b.x).toBe(210);
    expect(b.y).toBe(130);
  });

  it("falls back to {0,0} for a node elk didn't return a position for, without throwing", () => {
    const g = graph([node({ id: "A", host: "h1" })]);
    const elk: ElkRawLayout = { nodes: [], edges: [], width: 100, height: 100 };
    let layout;
    expect(() => {
      layout = buildFlowLayout(g, elk);
    }).not.toThrow();
    expect(layout!.nodes[0].x).toBe(0);
    expect(layout!.nodes[0].y).toBe(0);
  });
});

describe("buildFlowLayout — edge routing: bottom-of-source/top-of-target stubs + elk bends", () => {
  it("builds points as [sourceBottom, stubDown(+FLOW_STUB), ...elk bends, stubUp(-FLOW_STUB), targetTop]", () => {
    const g = graph(
      [node({ id: "A", host: "h1" }), node({ id: "B", host: "h1" })],
      [edge({ id: "e1", from: "A", to: "B" })],
    );
    const elk: ElkRawLayout = {
      nodes: [
        { id: "A", x: 0, y: 0 },
        { id: "B", x: 40, y: 200 },
      ],
      edges: [{ id: "e1", points: [{ x: 95, y: 66 }, { x: 100, y: 120 }, { x: 135, y: 200 }] }],
      width: 300,
      height: 300,
    };
    const layout = buildFlowLayout(g, elk);
    const e1 = layout.edges.find((e) => e.id === "e1")!;

    const sourceBottom = { x: 0 + NODE_W / 2, y: 0 + NODE_H };
    const targetTop = { x: 40 + NODE_W / 2, y: 200 };

    expect(e1.points[0]).toEqual(sourceBottom);
    expect(e1.points[1]).toEqual({ x: sourceBottom.x, y: sourceBottom.y + FLOW_STUB });
    // interior elk bend (the middle of the 3 raw points, since first/last are
    // elk's own start/end and get sliced off in favour of our own stubs)
    expect(e1.points[2]).toEqual({ x: 100, y: 120 });
    expect(e1.points[3]).toEqual({ x: targetTop.x, y: targetTop.y - FLOW_STUB });
    expect(e1.points[4]).toEqual(targetTop);
  });

  it("skips an edge referencing an unknown node id, without throwing", () => {
    const g = graph([node({ id: "A", host: "h1" })], [edge({ id: "bad", from: "A", to: "ghost" })]);
    const elk: ElkRawLayout = { nodes: [{ id: "A", x: 0, y: 0 }], edges: [], width: 10, height: 10 };
    let layout;
    expect(() => {
      layout = buildFlowLayout(g, elk);
    }).not.toThrow();
    expect(layout!.edges).toEqual([]);
  });

  it("computes the gate as the sourceBottom/targetTop midpoint (same semantics as by-host)", () => {
    const g = graph(
      [node({ id: "A", host: "h1" }), node({ id: "B", host: "h1" })],
      [edge({ id: "e1", from: "A", to: "B", has_gate: true })],
    );
    const elk: ElkRawLayout = {
      nodes: [
        { id: "A", x: 0, y: 0 },
        { id: "B", x: 0, y: 200 },
      ],
      edges: [],
      width: 300,
      height: 300,
    };
    const layout = buildFlowLayout(g, elk);
    const e1 = layout.edges.find((e) => e.id === "e1")!;
    const sourceBottom = { x: NODE_W / 2, y: NODE_H };
    const targetTop = { x: NODE_W / 2, y: 200 };
    expect(e1.gate).toEqual({
      x: (sourceBottom.x + targetTop.x) / 2,
      y: (sourceBottom.y + targetTop.y) / 2,
    });
  });

  it("returns gate: null for a non-gated edge", () => {
    const g = graph(
      [node({ id: "A", host: "h1" }), node({ id: "B", host: "h1" })],
      [edge({ id: "e1", from: "A", to: "B", has_gate: false })],
    );
    const elk: ElkRawLayout = {
      nodes: [
        { id: "A", x: 0, y: 0 },
        { id: "B", x: 0, y: 200 },
      ],
      edges: [],
      width: 300,
      height: 300,
    };
    const layout = buildFlowLayout(g, elk);
    expect(layout.edges.find((e) => e.id === "e1")!.gate).toBeNull();
  });

  it("passes each edge's status through unchanged (Normal/Consulting/Needs)", () => {
    const statuses: EdgeStatus[] = ["normal", "consulting", "needs"];
    const g = graph(
      [node({ id: "A", host: "h1" }), node({ id: "B", host: "h1" })],
      statuses.map((status, i) => edge({ id: `e${i}`, from: "A", to: "B", status })),
    );
    const elk: ElkRawLayout = {
      nodes: [
        { id: "A", x: 0, y: 0 },
        { id: "B", x: 0, y: 200 },
      ],
      edges: [],
      width: 300,
      height: 300,
    };
    const layout = buildFlowLayout(g, elk);
    expect(layout.edges.find((e) => e.id === "e0")!.status).toBe("normal");
    expect(layout.edges.find((e) => e.id === "e1")!.status).toBe("consulting");
    expect(layout.edges.find((e) => e.id === "e2")!.status).toBe("needs");
  });
});

describe("buildFlowLayout — canvas size + hosts", () => {
  it("uses elk's width/height for the canvas (min 1x1, never 0 — avoids a degenerate SVG)", () => {
    const g = graph([node({ id: "A", host: "h1" })]);
    const elk: ElkRawLayout = { nodes: [{ id: "A", x: 0, y: 0 }], edges: [], width: 0, height: 0 };
    const layout = buildFlowLayout(g, elk);
    expect(layout.width).toBeGreaterThanOrEqual(1);
    expect(layout.height).toBeGreaterThanOrEqual(1);
  });

  it("still reports hosts in first-appearance order (used for each node's Flow-mode host chip)", () => {
    const g = graph([node({ id: "A", host: "h2" }), node({ id: "B", host: "h1" })]);
    const elk: ElkRawLayout = {
      nodes: [
        { id: "A", x: 0, y: 0 },
        { id: "B", x: 0, y: 0 },
      ],
      edges: [],
      width: 10,
      height: 10,
    };
    const layout = buildFlowLayout(g, elk);
    expect(layout.hosts).toEqual(["h2", "h1"]);
  });
});

// ---------------------------------------------------------------------------
// smoothEdgePath — spline construction
// ---------------------------------------------------------------------------

describe("smoothEdgePath", () => {
  it("renders a straight line for a 2-point path", () => {
    const d = smoothEdgePath([{ x: 0, y: 0 }, { x: 10, y: 20 }]);
    expect(d).toBe("M 0 0 L 10 20");
  });

  it("returns an empty string for an empty point list, without throwing", () => {
    expect(smoothEdgePath([])).toBe("");
  });

  it("uses quadratic (Q) segments through interior points for a longer path", () => {
    const d = smoothEdgePath([{ x: 0, y: 0 }, { x: 5, y: 40 }, { x: 5, y: 80 }, { x: 10, y: 120 }]);
    expect(d.startsWith("M 0 0")).toBe(true);
    expect(d).toContain("Q");
    expect(d.trim().endsWith("L 10 120")).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// computeElkLayout — smoke test against the REAL elkjs layered algorithm
// ---------------------------------------------------------------------------

describe("computeElkLayout — real elkjs smoke test", () => {
  it("lays out a small linear DAG DOWNward without throwing, positioning every node", async () => {
    const g = graph(
      [node({ id: "A", host: "h1" }), node({ id: "B", host: "h1" }), node({ id: "C", host: "h2" })],
      [edge({ id: "e1", from: "A", to: "B" }), edge({ id: "e2", from: "B", to: "C" })],
    );
    const raw = await computeElkLayout(g);
    expect(raw.nodes).toHaveLength(3);
    for (const id of ["A", "B", "C"]) {
      expect(raw.nodes.find((n) => n.id === id)).toBeTruthy();
    }
    // Direction DOWN: each successive node in the chain should end up lower
    // (larger y) than its predecessor.
    const a = raw.nodes.find((n) => n.id === "A")!;
    const b = raw.nodes.find((n) => n.id === "B")!;
    const c = raw.nodes.find((n) => n.id === "C")!;
    expect(b.y).toBeGreaterThan(a.y);
    expect(c.y).toBeGreaterThan(b.y);
    expect(raw.edges.length).toBe(2);
  }, 10000);

  it("skips an edge with an unknown endpoint rather than throwing", async () => {
    const g = graph([node({ id: "A", host: "h1" })], [edge({ id: "bad", from: "A", to: "ghost" })]);
    const raw = await computeElkLayout(g);
    expect(raw.edges.find((e) => e.id === "bad")).toBeUndefined();
  }, 10000);
});
