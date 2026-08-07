/**
 * Behavioral tests for the GraphScreen screen (Level 2 — Workstream Graph).
 *
 * GraphScreen is pure/presentational: it takes `graph: GraphView`,
 * `selectedNode: string | null`, `onSelectNode`, `onOpenNode`, and
 * `onOpenGate` as props — no VmProvider, no Tauri runtime needed. Mirrors
 * the render/query/fireEvent style of tests/Board.test.tsx, wrapped in
 * ThemeProvider for token access.
 *
 * Covers spec §Test plan GS1-GS8 from docs/specs/ui-graph-screen.md.
 *
 * This file is written test-first: `../src/screens/graph/GraphScreen` does
 * not exist yet, so this suite is expected to fail to resolve until the
 * code-agent lands `src/screens/graph/GraphScreen.tsx`.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { GraphScreen } from "../src/screens/graph/GraphScreen";
import { layoutByHost, NODE_W, LANE_W, LANE_GAP, ROW_H } from "../src/screens/graph/layout";
import type {
  GraphView,
  GraphNode,
  GraphEdge,
  Inspector,
  HandoffSummary,
} from "../src/bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// Fixture builders — mirror tests/graphLayout.test.ts's node()/edge()/graph()
// helper style so GS1's expected coordinates line up with layoutByHost.
// ---------------------------------------------------------------------------

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
    bi: over.bi ?? false,
    status: over.status ?? "normal",
    has_gate: over.has_gate ?? false,
  };
}

function inspector(over: Partial<Inspector> = {}): Inspector {
  return {
    working: over.working ?? 0,
    needs: over.needs ?? 0,
    pending_handoff: over.pending_handoff ?? null,
  };
}

function handoff(over: Partial<HandoffSummary> & Pick<HandoffSummary, "conn_id"> ): HandoffSummary {
  return {
    conn_id: over.conn_id,
    from: over.from ?? "n1",
    to: over.to ?? "n2",
    payload: over.payload ?? "payload summary",
    risk: over.risk ?? "low",
  };
}

function graphView(over: Partial<GraphView> = {}): GraphView {
  return {
    ws_id: over.ws_id ?? "ws1",
    name: over.name ?? "Workstream 1",
    goal: over.goal ?? "ship the graph screen",
    nodes: over.nodes ?? [],
    edges: over.edges ?? [],
    inspector: over.inspector ?? inspector(),
  };
}

/**
 * Standard fixture: 3 nodes across 2 hosts ("host-a" has 2 nodes stacked in
 * one lane, "host-b" has 1), 2 edges (one plain, one gated with a "needs"
 * status), and an inspector with a pending_handoff. Used across most cases.
 */
function makeGraph(): GraphView {
  const nodes: GraphNode[] = [
    node({ id: "n1", host: "host-a", name: "Lead", driver: "claude", state: "Working", depth: 0, is_lead: true }),
    node({ id: "n2", host: "host-a", name: "Helper", driver: "codex", state: "Needs", depth: 1 }),
    node({ id: "n3", host: "host-b", name: "Reviewer", driver: "gemini", state: "Done", depth: 0 }),
  ];
  const edges: GraphEdge[] = [
    edge({ id: "e1", from: "n1", to: "n2", status: "normal", has_gate: false }),
    edge({ id: "e2", from: "n1", to: "n3", status: "needs", has_gate: true }),
  ];
  return graphView({
    nodes,
    edges,
    inspector: inspector({
      working: 1,
      needs: 1,
      pending_handoff: handoff({ conn_id: "e2", from: "n1", to: "n3" }),
    }),
  });
}

function renderGraph(
  graph: GraphView = makeGraph(),
  selectedNode: string | null = null,
  handlers: Partial<{
    onSelectNode: ReturnType<typeof vi.fn>;
    onOpenNode: ReturnType<typeof vi.fn>;
    onOpenGate: ReturnType<typeof vi.fn>;
  }> = {}
) {
  const onSelectNode = handlers.onSelectNode ?? vi.fn();
  const onOpenNode = handlers.onOpenNode ?? vi.fn();
  const onOpenGate = handlers.onOpenGate ?? vi.fn();
  const utils = render(
    <ThemeProvider initialTheme="dark">
      <GraphScreen
        graph={graph}
        selectedNode={selectedNode}
        onSelectNode={onSelectNode}
        onOpenNode={onOpenNode}
        onOpenGate={onOpenGate}
      />
    </ThemeProvider>
  );
  return { ...utils, onSelectNode, onOpenNode, onOpenGate };
}

// ---------------------------------------------------------------------------
// GS1 — node positions match layoutByHost geometry
// ---------------------------------------------------------------------------

describe("GraphScreen — GS1 node positions from layoutByHost", () => {
  it("positions each graph-node-{id} at the left/top layoutByHost computes for the fixture", () => {
    const g = makeGraph();
    const layout = layoutByHost(g);
    renderGraph(g);

    for (const positioned of layout.nodes) {
      const el = screen.getByTestId(`graph-node-${positioned.id}`);
      expect(el.style.left).toBe(`${positioned.x}px`);
      expect(el.style.top).toBe(`${positioned.y}px`);
    }
  });

  it("computes n1/n2 (host-a, stacked rows) and n3 (host-b) coordinates from the raw spec constants", () => {
    // host-a is the first-appearance host -> column 0; host-b -> column 1.
    const laneX = (col: number) => col * (LANE_W + LANE_GAP);
    const centeredX = (col: number) => laneX(col) + (LANE_W - NODE_W) / 2;

    const g = makeGraph();
    renderGraph(g);

    const n1 = screen.getByTestId("graph-node-n1");
    const n2 = screen.getByTestId("graph-node-n2");
    const n3 = screen.getByTestId("graph-node-n3");

    // n1: host-a, depth 0 -> row 0
    expect(n1.style.left).toBe(`${centeredX(0)}px`);
    expect(n1.style.top).toBe(`${0 * ROW_H}px`);
    // n2: host-a, depth 1 -> row 1 (below n1 in the same lane)
    expect(n2.style.left).toBe(`${centeredX(0)}px`);
    expect(n2.style.top).toBe(`${1 * ROW_H}px`);
    // n3: host-b, depth 0 -> row 0, second lane column
    expect(n3.style.left).toBe(`${centeredX(1)}px`);
    expect(n3.style.top).toBe(`${0 * ROW_H}px`);
  });

  it("renders exactly one graph-node-{id} per node in graph.nodes (no extras, no omissions)", () => {
    const g = makeGraph();
    renderGraph(g);
    for (const n of g.nodes) {
      expect(screen.getByTestId(`graph-node-${n.id}`)).toBeTruthy();
    }
    const { container } = render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={g}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
        />
      </ThemeProvider>
    );
    expect(container.querySelectorAll('[data-testid^="graph-node-"]').length).toBe(
      g.nodes.length
    );
  });
});

// ---------------------------------------------------------------------------
// GS2 — node content: name, driver pill color, data-driver/data-state
// ---------------------------------------------------------------------------

describe("GraphScreen — GS2 node content", () => {
  it("shows each node's name and a driver-pill with data-driver set to the node's driver", () => {
    const g = makeGraph();
    renderGraph(g);
    for (const n of g.nodes) {
      const el = screen.getByTestId(`graph-node-${n.id}`);
      expect(el.textContent).toContain(n.name);
      const pill = el.querySelector('[data-testid="driver-pill"]');
      expect(pill).toBeTruthy();
      expect(pill!.getAttribute("data-driver")).toBe(n.driver);
    }
  });

  it("colors the claude driver-pill with the claude accent color (#d97757)", () => {
    const g = makeGraph(); // n1 is driver "claude"
    renderGraph(g);
    const el = screen.getByTestId("graph-node-n1");
    const pill = el.querySelector('[data-testid="driver-pill"]') as HTMLElement;
    expect(pill).toBeTruthy();
    const styleAttr = pill.getAttribute("style") ?? "";
    expect(styleAttr.toLowerCase()).toContain("#d97757");
  });

  it("sets data-state on the node to the node's state", () => {
    const g = makeGraph();
    renderGraph(g);
    expect(screen.getByTestId("graph-node-n1").getAttribute("data-state")).toBe("Working");
    expect(screen.getByTestId("graph-node-n2").getAttribute("data-state")).toBe("Needs");
    expect(screen.getByTestId("graph-node-n3").getAttribute("data-state")).toBe("Done");
  });

  it("shows each node's status text within the node", () => {
    const g = graphView({
      nodes: [
        node({ id: "n1", host: "host-a", status: "compiling widgets" }),
      ],
    });
    renderGraph(g);
    expect(screen.getByTestId("graph-node-n1").textContent).toContain("compiling widgets");
  });
});

// ---------------------------------------------------------------------------
// GS3 — click interactions: select vs open
// ---------------------------------------------------------------------------

describe("GraphScreen — GS3 click interactions", () => {
  it("clicking a node calls onSelectNode with that node's id", () => {
    const g = makeGraph();
    const { onSelectNode } = renderGraph(g);
    fireEvent.click(screen.getByTestId("graph-node-n2"));
    expect(onSelectNode).toHaveBeenCalledWith("n2");
    expect(onSelectNode).toHaveBeenCalledTimes(1);
  });

  it("clicking the node-open affordance calls onOpenNode with that node's id, not onSelectNode-only", () => {
    const g = makeGraph();
    const { onOpenNode } = renderGraph(g);
    const openAffordance =
      screen.queryByTestId("graph-node-open-n1") ?? screen.getByTestId("node-open-n1");
    fireEvent.click(openAffordance);
    expect(onOpenNode).toHaveBeenCalledWith("n1");
    expect(onOpenNode).toHaveBeenCalledTimes(1);
  });

  it("clicking one node's select affordance does not fire onSelectNode for a different id", () => {
    const g = makeGraph();
    const { onSelectNode } = renderGraph(g);
    fireEvent.click(screen.getByTestId("graph-node-n3"));
    expect(onSelectNode).toHaveBeenCalledWith("n3");
    expect(onSelectNode).not.toHaveBeenCalledWith("n1");
    expect(onSelectNode).not.toHaveBeenCalledWith("n2");
  });
});

// ---------------------------------------------------------------------------
// GS4 — selection + neighbourhood dimming
// ---------------------------------------------------------------------------

describe("GraphScreen — GS4 selection and neighbourhood dimming", () => {
  it("marks the selected node data-selected=\"true\" and non-selected nodes data-selected=\"false\" (or unset)", () => {
    const g = makeGraph();
    renderGraph(g, "n1");
    const selected = screen.getByTestId("graph-node-n1");
    expect(selected.getAttribute("data-selected")).toBe("true");
    const other = screen.getByTestId("graph-node-n2");
    expect(other.getAttribute("data-selected")).not.toBe("true");
  });

  it("dims a non-neighbour node (data-dimmed=\"true\") when a node is selected", () => {
    // n1 -> n2 and n1 -> n3 are the only edges; select n2. n2's only
    // neighbour is n1 (via e1); there is no edge between n2 and n3, so n3
    // must be dimmed.
    const g = makeGraph();
    renderGraph(g, "n2");
    const nonNeighbour = screen.getByTestId("graph-node-n3");
    expect(nonNeighbour.getAttribute("data-dimmed")).toBe("true");
  });

  it("does not dim the selected node itself or a direct edge-connected neighbour", () => {
    const g = makeGraph();
    renderGraph(g, "n1");
    const selected = screen.getByTestId("graph-node-n1");
    const neighbourViaE1 = screen.getByTestId("graph-node-n2"); // n1->n2
    const neighbourViaE2 = screen.getByTestId("graph-node-n3"); // n1->n3
    expect(selected.getAttribute("data-dimmed")).not.toBe("true");
    expect(neighbourViaE1.getAttribute("data-dimmed")).not.toBe("true");
    expect(neighbourViaE2.getAttribute("data-dimmed")).not.toBe("true");
  });

  it("dims no nodes at all when selectedNode is null", () => {
    const g = makeGraph();
    const { container } = renderGraph(g, null);
    const dimmed = container.querySelectorAll('[data-dimmed="true"]');
    expect(dimmed.length).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// GS5 — edges: rendering, status, gate diamond
// ---------------------------------------------------------------------------

describe("GraphScreen — GS5 edges and gate diamond", () => {
  it("renders one graph-edge-{id} SVG path per edge, with data-status matching the edge's status", () => {
    const g = makeGraph();
    renderGraph(g);
    const e1 = screen.getByTestId("graph-edge-e1");
    expect(e1.tagName.toLowerCase()).toBe("path");
    expect(e1.getAttribute("data-status")).toBe("normal");

    const e2 = screen.getByTestId("graph-edge-e2");
    expect(e2.tagName.toLowerCase()).toBe("path");
    expect(e2.getAttribute("data-status")).toBe("needs");
  });

  it("renders exactly the number of graph-edge-* elements as graph.edges (no extras)", () => {
    const g = makeGraph();
    const { container } = renderGraph(g);
    expect(container.querySelectorAll('[data-testid^="graph-edge-"]').length).toBe(
      g.edges.length
    );
  });

  it("renders a gate-diamond-{connId} for a gated edge, and none for a non-gated edge", () => {
    const g = makeGraph();
    renderGraph(g);
    // e2 has_gate: true, conn id "e2"
    expect(screen.getByTestId("gate-diamond-e2")).toBeTruthy();
    // e1 has_gate: false -> no diamond for it
    expect(screen.queryByTestId("gate-diamond-e1")).toBeNull();
  });

  it("clicking a gate diamond calls onOpenGate with that edge's connection id", () => {
    const g = makeGraph();
    const { onOpenGate } = renderGraph(g);
    fireEvent.click(screen.getByTestId("gate-diamond-e2"));
    expect(onOpenGate).toHaveBeenCalledWith("e2");
    expect(onOpenGate).toHaveBeenCalledTimes(1);
  });
});

// ---------------------------------------------------------------------------
// GS6 — inspector: working/needs counts + pending handoff
// ---------------------------------------------------------------------------

describe("GraphScreen — GS6 inspector counts and pending handoff", () => {
  it("shows inspector-working and inspector-needs counts from graph.inspector", () => {
    const g = graphView({
      nodes: [node({ id: "n1", host: "host-a" })],
      inspector: inspector({ working: 4, needs: 2 }),
    });
    renderGraph(g);
    const inspectorPanel = screen.getByTestId("graph-inspector");
    expect(inspectorPanel).toBeTruthy();
    expect(screen.getByTestId("inspector-working").textContent).toContain("4");
    expect(screen.getByTestId("inspector-needs").textContent).toContain("2");
  });

  it("shows the workstream name and goal inside the inspector panel", () => {
    const g = graphView({
      name: "checkout-flow",
      goal: "land the checkout redesign",
      nodes: [node({ id: "n1", host: "host-a" })],
    });
    renderGraph(g);
    const inspectorPanel = screen.getByTestId("graph-inspector");
    expect(inspectorPanel.textContent).toContain("checkout-flow");
    expect(inspectorPanel.textContent).toContain("land the checkout redesign");
  });

  it("renders inspector-handoff when pending_handoff is set, and clicking it calls onOpenGate with the handoff's conn_id", () => {
    const g = makeGraph(); // pending_handoff.conn_id === "e2"
    const { onOpenGate } = renderGraph(g);
    const handoffEl = screen.getByTestId("inspector-handoff");
    expect(handoffEl).toBeTruthy();
    fireEvent.click(handoffEl);
    expect(onOpenGate).toHaveBeenCalledWith("e2");
  });

  it("does not render inspector-handoff when pending_handoff is null", () => {
    const g = graphView({
      nodes: [node({ id: "n1", host: "host-a" })],
      inspector: inspector({ working: 0, needs: 0, pending_handoff: null }),
    });
    renderGraph(g);
    expect(screen.queryByTestId("inspector-handoff")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// GS7 — inspector Sessions list
// ---------------------------------------------------------------------------

describe("GraphScreen — GS7 inspector Sessions list", () => {
  it("renders one inspector-session-{id} row per node in graph.nodes", () => {
    const g = makeGraph();
    renderGraph(g);
    for (const n of g.nodes) {
      expect(screen.getByTestId(`inspector-session-${n.id}`)).toBeTruthy();
    }
  });

  it("clicking an inspector-session-{id} row calls onOpenNode with that node's id", () => {
    const g = makeGraph();
    const { onOpenNode } = renderGraph(g);
    fireEvent.click(screen.getByTestId("inspector-session-n3"));
    expect(onOpenNode).toHaveBeenCalledWith("n3");
    expect(onOpenNode).toHaveBeenCalledTimes(1);
  });

  it("clicking a different session row does not call onOpenNode with the wrong id", () => {
    const g = makeGraph();
    const { onOpenNode } = renderGraph(g);
    fireEvent.click(screen.getByTestId("inspector-session-n2"));
    expect(onOpenNode).toHaveBeenCalledWith("n2");
    expect(onOpenNode).not.toHaveBeenCalledWith("n1");
    expect(onOpenNode).not.toHaveBeenCalledWith("n3");
  });
});

// ---------------------------------------------------------------------------
// GS8 — pulsing state dot for working/needs nodes, not for done
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// GS9 — layout toggle (By host / Flow)
// ---------------------------------------------------------------------------

describe("GraphScreen — GS9 layout toggle", () => {
  it("defaults to By-host mode and switches graph-canvas's mode to flow on click", () => {
    const g = makeGraph();
    renderGraph(g);
    const canvas = screen.getByTestId("graph-canvas");
    expect(canvas.getAttribute("data-layout-mode")).toBe("host");
    fireEvent.click(screen.getByTestId("layout-toggle-flow"));
    expect(canvas.getAttribute("data-layout-mode")).toBe("flow");
  });

  it("switches back to By-host mode from Flow", () => {
    const g = makeGraph();
    renderGraph(g);
    fireEvent.click(screen.getByTestId("layout-toggle-flow"));
    fireEvent.click(screen.getByTestId("layout-toggle-host"));
    expect(screen.getByTestId("graph-canvas").getAttribute("data-layout-mode")).toBe("host");
  });

  it("shows a host chip on nodes only in Flow mode, not in By-host mode", () => {
    const g = makeGraph();
    renderGraph(g);
    expect(screen.queryByTestId("node-host-chip-n1")).toBeNull();
    fireEvent.click(screen.getByTestId("layout-toggle-flow"));
    expect(screen.getByTestId("node-host-chip-n1")).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// GS10 — zoom controls
// ---------------------------------------------------------------------------

describe("GraphScreen — GS10 zoom controls", () => {
  it("zoom-in increases the displayed percentage, zoom-out decreases it, and the pct label resets to 100% on click", () => {
    const g = makeGraph();
    renderGraph(g);
    const pct = screen.getByTestId("zoom-pct");
    const before = pct.textContent;
    fireEvent.click(screen.getByTestId("zoom-in"));
    expect(pct.textContent).not.toBe(before);
    const afterIn = parseInt(pct.textContent ?? "0", 10);
    fireEvent.click(screen.getByTestId("zoom-out"));
    fireEvent.click(screen.getByTestId("zoom-out"));
    const afterOut = parseInt(pct.textContent ?? "0", 10);
    expect(afterOut).toBeLessThan(afterIn);
    fireEvent.click(pct);
    expect(pct.textContent).toBe("100%");
  });
});

// ---------------------------------------------------------------------------
// GS11 — gate diamond placement matches the routed edge's gate midpoint
// ---------------------------------------------------------------------------

describe("GraphScreen — GS11 gate diamond placement", () => {
  it("positions the 30x30 gate diamond centered on the routed edge's gate point", () => {
    const g = makeGraph();
    const layout = layoutByHost(g);
    renderGraph(g);
    const gateEdge = layout.edges.find((e) => e.id === "e2")!;
    expect(gateEdge.gate).not.toBeNull();
    const diamond = screen.getByTestId("gate-diamond-e2");
    expect(diamond.style.left).toBe(`${gateEdge.gate!.x - 15}px`);
    expect(diamond.style.top).toBe(`${gateEdge.gate!.y - 15}px`);
    expect(diamond.style.width).toBe("30px");
    expect(diamond.style.height).toBe("30px");
  });
});

// ---------------------------------------------------------------------------
// GS12 — design mode: connect handles, drag-to-connect, connection dialog
// ---------------------------------------------------------------------------

describe("GraphScreen — GS12 design mode", () => {
  it("shows connect handles and an Add-agent button in design mode instead of the node-open chevron / Update-workflow button", () => {
    const g = makeGraph();
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={g}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
        />
      </ThemeProvider>
    );
    expect(screen.getByTestId("connect-handle-n1")).toBeTruthy();
    expect(screen.getByTestId("add-agent-btn")).toBeTruthy();
    expect(screen.queryByTestId("enter-design-btn")).toBeNull();
    expect(screen.queryByTestId("node-open-n1")).toBeNull();
  });

  it("shows the Update-workflow button (not Add-agent) when designMode is false/omitted", () => {
    const g = makeGraph();
    renderGraph(g);
    expect(screen.getByTestId("enter-design-btn")).toBeTruthy();
    expect(screen.queryByTestId("add-agent-btn")).toBeNull();
  });

  it("clicking Add-agent calls onAddAgent", () => {
    const g = makeGraph();
    const onAddAgent = vi.fn();
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={g}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          onAddAgent={onAddAgent}
        />
      </ThemeProvider>
    );
    fireEvent.click(screen.getByTestId("add-agent-btn"));
    expect(onAddAgent).toHaveBeenCalledTimes(1);
  });

  it("clicking Update-workflow calls onEnterDesign; Confirm/Discard call their handlers in the design inspector panel", () => {
    const g = makeGraph();
    const onEnterDesign = vi.fn();
    const onConfirmDraft = vi.fn();
    const onDiscardDraft = vi.fn();
    const { rerender } = render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={g}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          onEnterDesign={onEnterDesign}
        />
      </ThemeProvider>
    );
    fireEvent.click(screen.getByTestId("enter-design-btn"));
    expect(onEnterDesign).toHaveBeenCalledTimes(1);

    rerender(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={g}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          onConfirmDraft={onConfirmDraft}
          onDiscardDraft={onDiscardDraft}
        />
      </ThemeProvider>
    );
    // Design mode inspector: no stats, an Agents(N) list instead.
    expect(screen.queryByTestId("inspector-working")).toBeNull();
    expect(screen.getByText(`Agents (${g.nodes.length})`)).toBeTruthy();
    fireEvent.click(screen.getByTestId("confirm-draft-btn"));
    expect(onConfirmDraft).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByTestId("discard-draft-btn"));
    expect(onDiscardDraft).toHaveBeenCalledTimes(1);
  });

  it("drag-connecting from one node's handle to another opens the connection dialog, and picking Consult calls onConnect(from, to, true)", () => {
    const g = makeGraph();
    const onConnect = vi.fn();
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={g}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          onConnect={onConnect}
        />
      </ThemeProvider>
    );

    const canvas = screen.getByTestId("graph-canvas");
    canvas.getBoundingClientRect = () =>
      ({ left: 0, top: 0, right: 2000, bottom: 2000, width: 2000, height: 2000, x: 0, y: 0, toJSON() {} } as DOMRect);

    const handle = screen.getByTestId("connect-handle-n1");
    const targetNode = screen.getByTestId("graph-node-n3");
    const spy = vi.spyOn(document, "elementFromPoint").mockReturnValue(targetNode);

    fireEvent.mouseDown(handle, { clientX: 10, clientY: 10 });
    act(() => {
      window.dispatchEvent(new MouseEvent("mousemove", { clientX: 60, clientY: 90, bubbles: true }));
    });
    act(() => {
      window.dispatchEvent(new MouseEvent("mouseup", { clientX: 60, clientY: 90, bubbles: true }));
    });

    expect(screen.getByTestId("connection-dialog")).toBeTruthy();
    // A NEW edge has no Delete option (nothing to delete yet).
    expect(screen.queryByTestId("edge-delete")).toBeNull();
    fireEvent.click(screen.getByTestId("edge-dir-consult"));
    expect(onConnect).toHaveBeenCalledWith("n1", "n3", true);
    expect(screen.queryByTestId("connection-dialog")).toBeNull();

    spy.mockRestore();
  });

  it("picking Directed in the connection dialog calls onConnect(from, to, false)", () => {
    const g = makeGraph();
    const onConnect = vi.fn();
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={g}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          onConnect={onConnect}
        />
      </ThemeProvider>
    );
    const canvas = screen.getByTestId("graph-canvas");
    canvas.getBoundingClientRect = () =>
      ({ left: 0, top: 0, right: 2000, bottom: 2000, width: 2000, height: 2000, x: 0, y: 0, toJSON() {} } as DOMRect);
    const handle = screen.getByTestId("connect-handle-n1");
    const targetNode = screen.getByTestId("graph-node-n3");
    const spy = vi.spyOn(document, "elementFromPoint").mockReturnValue(targetNode);
    fireEvent.mouseDown(handle, { clientX: 10, clientY: 10 });
    act(() => {
      window.dispatchEvent(new MouseEvent("mouseup", { clientX: 60, clientY: 90, bubbles: true }));
    });
    fireEvent.click(screen.getByTestId("edge-dir-directed"));
    expect(onConnect).toHaveBeenCalledWith("n1", "n3", false);
    spy.mockRestore();
  });

  it("dropping a drag-connect on the SAME node it started from does not open the connection dialog", () => {
    const g = makeGraph();
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={g}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
        />
      </ThemeProvider>
    );
    const canvas = screen.getByTestId("graph-canvas");
    canvas.getBoundingClientRect = () =>
      ({ left: 0, top: 0, right: 2000, bottom: 2000, width: 2000, height: 2000, x: 0, y: 0, toJSON() {} } as DOMRect);
    const handle = screen.getByTestId("connect-handle-n1");
    const sameNode = screen.getByTestId("graph-node-n1");
    const spy = vi.spyOn(document, "elementFromPoint").mockReturnValue(sameNode);

    fireEvent.mouseDown(handle, { clientX: 10, clientY: 10 });
    act(() => {
      window.dispatchEvent(new MouseEvent("mouseup", { clientX: 12, clientY: 12, bubbles: true }));
    });

    expect(screen.queryByTestId("connection-dialog")).toBeNull();
    spy.mockRestore();
  });
});

describe("GraphScreen — GS8 pulsing node state dot", () => {
  it("a Working-state node's state dot has data-pulse=\"true\"", () => {
    const g = graphView({ nodes: [node({ id: "n1", host: "host-a", state: "Working" })] });
    renderGraph(g);
    const el = screen.getByTestId("graph-node-n1");
    const dot = el.querySelector("[data-pulse]");
    expect(dot).toBeTruthy();
    expect(dot!.getAttribute("data-pulse")).toBe("true");
  });

  it("a Needs-state node's state dot has data-pulse=\"true\"", () => {
    const g = graphView({ nodes: [node({ id: "n1", host: "host-a", state: "Needs" })] });
    renderGraph(g);
    const el = screen.getByTestId("graph-node-n1");
    const dot = el.querySelector("[data-pulse]");
    expect(dot).toBeTruthy();
    expect(dot!.getAttribute("data-pulse")).toBe("true");
  });

  it("a Done-state node's state dot does NOT pulse", () => {
    const g = graphView({ nodes: [node({ id: "n1", host: "host-a", state: "Done" })] });
    renderGraph(g);
    const el = screen.getByTestId("graph-node-n1");
    const dot = el.querySelector("[data-pulse]");
    if (dot) {
      expect(dot.getAttribute("data-pulse")).not.toBe("true");
    } else {
      expect(dot).toBeNull();
    }
  });
});

// ---------------------------------------------------------------------------
// P3 — design-mode remove affordances (node + connection)
// ---------------------------------------------------------------------------

describe("GraphScreen — design-mode remove affordances", () => {
  it("each design-agent row has a remove button that calls onRemoveNode", () => {
    const onRemoveNode = vi.fn();
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={makeGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          onRemoveNode={onRemoveNode}
        />
      </ThemeProvider>
    );
    fireEvent.click(screen.getByTestId("agent-remove-n2"));
    expect(onRemoveNode).toHaveBeenCalledWith("n2");
  });

  it("clicking an edge handle opens the connection dialog whose Delete calls onRemoveConnection (design mode)", () => {
    const onRemoveConnection = vi.fn();
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={makeGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          onRemoveConnection={onRemoveConnection}
        />
      </ThemeProvider>
    );
    fireEvent.click(screen.getByTestId("edge-edit-e1"));
    // Existing edge → the dialog offers Delete.
    fireEvent.click(screen.getByTestId("edge-delete"));
    expect(onRemoveConnection).toHaveBeenCalledWith("e1");
  });

  it("clicking an edge handle then Consult calls onSetConnectionBi to flip its direction", () => {
    const onSetConnectionBi = vi.fn();
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={makeGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          designMode
          onSetConnectionBi={onSetConnectionBi}
        />
      </ThemeProvider>
    );
    fireEvent.click(screen.getByTestId("edge-edit-e1"));
    fireEvent.click(screen.getByTestId("edge-dir-consult"));
    expect(onSetConnectionBi).toHaveBeenCalledWith("e1", true);
  });

  it("edge edit handles are NOT shown in operate mode", () => {
    render(
      <ThemeProvider initialTheme="dark">
        <GraphScreen
          graph={makeGraph()}
          selectedNode={null}
          onSelectNode={vi.fn()}
          onOpenNode={vi.fn()}
          onOpenGate={vi.fn()}
          onRemoveConnection={vi.fn()}
        />
      </ThemeProvider>
    );
    expect(screen.queryByTestId("edge-edit-e1")).toBeNull();
  });
});
