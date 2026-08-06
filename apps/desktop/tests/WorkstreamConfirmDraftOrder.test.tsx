/**
 * Phase C — RED suite for onConfirmDraft extended behavior.
 *
 * Target behaviors (NOT YET IMPLEMENTED):
 *
 *   1. onConfirmDraft launches draft nodes in DEPENDENCY ORDER (via
 *      `orderDraftLaunch`) — not just lead-first, but full topological sort
 *      so A→B→C authored out-of-order (C, B, A) still launches A, B, C.
 *
 *   2. onConfirmDraft calls `launchWaslSession` for `wasl-*` draft nodes,
 *      with the correct `wasl_target_key` shape:
 *        { protocol: waslProtocol(driver), context: { cwd, model, session_mode } }
 *      and calls `launchSession` for non-wasl (`claude`, etc.) nodes.
 *
 *   3. temp→real edge resolution still works, including a draft↔real edge
 *      (one endpoint is a real session already in the graph, the other is
 *      a draft node whose tempId is replaced with its real id after launch).
 *
 *   4. On launch failure, the draft is NOT cleared and the designError is
 *      surfaced.
 *
 * Mocking pattern mirrors WorkstreamNodeCreation.test.tsx.
 */
import React from "react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { VmProvider, VmState, VmContext } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { GraphScreenConnected } from "../src/screens/graph/GraphScreenConnected";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { GraphView } from "../src/bridge/workstreamTypes";
import type { DraftNode, DraftEdge } from "../src/bridge/useVm";
import type { CapabilitiesView } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// API mocks
// ---------------------------------------------------------------------------

let launchCounter = 0;
const launchSession = vi.fn(async (_args: unknown) => `real-sess-${++launchCounter}`);
const launchWaslSession = vi.fn(async (_args: unknown) => `real-wasl-${++launchCounter}`);
const assignSession = vi.fn(async (_sessionId: string, _wsId: string, _lead: boolean) => {});
const addConnection = vi.fn(async (..._args: unknown[]) => {});
const fetchGraph = vi.fn(async (_wsId: string): Promise<GraphView | null> => null);
const fetchBoard = vi.fn(async () => ({
  summary: { workstreams: 0, sessions: 0, hosts: 0 },
  host_chips: [],
  link_chips: [],
  attention: [],
  all: [],
}));

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    launchSession: (args: unknown) => launchSession(args),
    launchWaslSession: (args: unknown) => launchWaslSession(args),
    assignSession: (sessionId: string, wsId: string, lead: boolean) =>
      assignSession(sessionId, wsId, lead),
    addConnection: (...args: unknown[]) => addConnection(...args),
    fetchGraph: (wsId: string) => fetchGraph(wsId),
    fetchBoard: () => fetchBoard(),
  };
});

beforeEach(() => {
  vi.clearAllMocks();
  launchCounter = 0;
  launchSession.mockImplementation(async () => `real-sess-${++launchCounter}`);
  launchWaslSession.mockImplementation(async () => `real-wasl-${++launchCounter}`);
  assignSession.mockResolvedValue(undefined);
  addConnection.mockResolvedValue(undefined);
  fetchGraph.mockResolvedValue(null);
  fetchBoard.mockResolvedValue({
    summary: { workstreams: 0, sessions: 0, hosts: 0 },
    host_chips: [],
    link_chips: [],
    attention: [],
    all: [],
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function StateCapture({ onState, children }: { onState: (s: VmState) => void; children: React.ReactNode }) {
  return (
    <VmContext.Consumer>
      {(ctx) => {
        if (ctx) onState(ctx.state);
        return <>{children}</>;
      }}
    </VmContext.Consumer>
  );
}

function emptyGraph(wsId = "ws1"): GraphView {
  return {
    ws_id: wsId,
    name: "Test WS",
    goal: "",
    nodes: [],
    edges: [],
    inspector: { working: 0, needs: 0, pending_handoff: null },
  };
}

function graphWithRealNode(realNodeId: string, wsId = "ws1"): GraphView {
  return {
    ws_id: wsId,
    name: "Test WS",
    goal: "",
    nodes: [{
      id: realNodeId,
      name: "Real Node",
      driver: "claude",
      host: "local",
      state: "Done",
      status: "",
      context_pct: 0,
      is_lead: true,
      depth: 0,
    }],
    edges: [],
    inspector: { working: 0, needs: 0, pending_handoff: null },
  };
}

function makeDraftNode(tempId: string, opts: Partial<DraftNode> = {}): DraftNode {
  return {
    tempId,
    name: opts.name ?? tempId,
    project: opts.project ?? `/code/${tempId}`,
    hostId: opts.hostId ?? "local",
    driver: opts.driver ?? "claude",
    model: opts.model ?? "claude-sonnet-latest",
    session_mode: opts.session_mode ?? "",
    firstPrompt: opts.firstPrompt,
    isLead: opts.isLead ?? false,
  };
}

function makeDraftEdge(from: string, to: string): DraftEdge {
  return { tempId: `de-${from}-${to}`, from, to, kind: "data", bi: false };
}

function claudeCapabilities(hostId = "local"): Record<string, CapabilitiesView> {
  return {
    [`claude::${hostId}`]: {
      driver: "claude",
      host_id: hostId,
      cli_available: true,
      wasl_available: true,
      launchable: true,
      models: { models: [{ id: "claude-sonnet-latest" }], source: "Listed", free_text: true },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 0,
      errors: [],
    },
    [`wasl-claude::${hostId}`]: {
      driver: "wasl-claude",
      host_id: hostId,
      cli_available: true,
      wasl_available: true,
      launchable: true,
      models: { models: [{ id: "claude-sonnet-latest" }], source: "Listed", free_text: true },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 0,
      errors: [],
    },
  };
}

function renderGraphConnected(preState: Partial<VmState>, captured?: { current: VmState | null }) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "graph",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    draftWsId: null,
    capabilities: claudeCapabilities(),
    board: {
      summary: { workstreams: 0, sessions: 0, hosts: 0 },
      host_chips: [],
      link_chips: [],
      attention: [],
      all: [],
    },
    ...preState,
  };

  const capturedRef = captured ?? { current: null };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <StateCapture onState={(s) => (capturedRef.current = s)}>
          <GraphScreenConnected />
        </StateCapture>
      </VmProvider>
    </ThemeProvider>
  );
}

// ===========================================================================
// Test 1: dependency order — A→B→C authored C, B, A
// ===========================================================================

describe("onConfirmDraft — dependency order via orderDraftLaunch", () => {
  it("launches A before B before C for a chain authored in reverse (C, B, A)", async () => {
    // Draft order (authored order): C, B, A
    // Edges: A→B, B→C (chain)
    // Expected launch order: A (depth 0 + lead), B (depth 1), C (depth 2)
    const draftA = makeDraftNode("draft-A", { isLead: true, name: "Agent A", project: "/code/A" });
    const draftB = makeDraftNode("draft-B", { name: "Agent B", project: "/code/B" });
    const draftC = makeDraftNode("draft-C", { name: "Agent C", project: "/code/C" });
    const edges = [
      makeDraftEdge("draft-A", "draft-B"),
      makeDraftEdge("draft-B", "draft-C"),
    ];

    // Authored in reverse (C, B, A) to test that the sort is NOT authored order
    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [draftC, draftB, draftA],
      draftEdges: edges,
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchSession).toHaveBeenCalledTimes(3), { timeout: 2000 });

    // The order of launchSession calls must be A, B, C (by dependency depth)
    const projects = launchSession.mock.calls.map((c) => (c[0] as { project: string }).project);
    expect(projects[0]).toBe("/code/A");
    expect(projects[1]).toBe("/code/B");
    expect(projects[2]).toBe("/code/C");
  });

  it("lead (depth 0) is always first, even when authored last", async () => {
    // Two depth-0 nodes (lead + sibling), one depth-1 dependent
    const lead = makeDraftNode("lead", { isLead: true, name: "Lead", project: "/code/lead" });
    const sibling = makeDraftNode("sibling", { name: "Sibling", project: "/code/sibling" });
    const dep = makeDraftNode("dep", { name: "Dep", project: "/code/dep" });
    // lead → dep, sibling has no edges
    const edges = [makeDraftEdge("lead", "dep")];

    // Authored order: dep, sibling, lead
    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [dep, sibling, lead],
      draftEdges: edges,
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchSession).toHaveBeenCalledTimes(3), { timeout: 2000 });

    const projects = launchSession.mock.calls.map((c) => (c[0] as { project: string }).project);
    // Lead must be first
    expect(projects[0]).toBe("/code/lead");
    // dep must come after lead (depth 1 > depth 0)
    expect(projects.indexOf("/code/lead")).toBeLessThan(projects.indexOf("/code/dep"));
  });
});

// ===========================================================================
// Test 2: wasl-claude draft node → launchWaslSession with correct target_key
// ===========================================================================

describe("onConfirmDraft — launchWaslSession for wasl-* draft nodes", () => {
  it("calls launchWaslSession (not launchSession) for a wasl-claude draft node", async () => {
    const waslNode = makeDraftNode("wasl-node", {
      isLead: true,
      driver: "wasl-claude",
      model: "claude-sonnet-latest",
      session_mode: "",
      project: "/code/wasl-proj",
      name: "Wasl Agent",
    });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [waslNode],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1), { timeout: 2000 });
    expect(launchSession).not.toHaveBeenCalled();
  });

  it("launchWaslSession is called with wasl_target_key.protocol === 'claude'", async () => {
    const waslNode = makeDraftNode("wasl-node", {
      isLead: true,
      driver: "wasl-claude",
      model: "claude-sonnet-latest",
      session_mode: "",
      project: "/code/proj",
    });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [waslNode],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1));
    expect(launchWaslSession).toHaveBeenCalledWith(
      expect.objectContaining({
        wasl_target_key: expect.objectContaining({ protocol: "claude" }),
      })
    );
  });

  it("launchWaslSession context.cwd === the draft node's project", async () => {
    const project = "/code/wasl-proj";
    const waslNode = makeDraftNode("wasl-node", {
      isLead: true,
      driver: "wasl-claude",
      project,
      model: "claude-sonnet-latest",
      session_mode: "",
    });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [waslNode],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1));
    expect(launchWaslSession).toHaveBeenCalledWith(
      expect.objectContaining({
        wasl_target_key: expect.objectContaining({
          context: expect.objectContaining({ cwd: project }),
        }),
      })
    );
  });

  it("launchWaslSession context.model === the draft node's model", async () => {
    const model = "claude-opus-latest";
    const waslNode = makeDraftNode("wasl-node", {
      isLead: true,
      driver: "wasl-claude",
      model,
      session_mode: "",
      project: "/code/proj",
    });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [waslNode],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1));
    expect(launchWaslSession).toHaveBeenCalledWith(
      expect.objectContaining({
        wasl_target_key: expect.objectContaining({
          context: expect.objectContaining({ model }),
        }),
      })
    );
  });

  it("launchWaslSession context.session_mode === the draft node's session_mode", async () => {
    const session_mode = "plan";
    const waslNode = makeDraftNode("wasl-node", {
      isLead: true,
      driver: "wasl-claude",
      session_mode,
      project: "/code/proj",
      model: "claude-sonnet-latest",
    });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [waslNode],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1));
    expect(launchWaslSession).toHaveBeenCalledWith(
      expect.objectContaining({
        wasl_target_key: expect.objectContaining({
          context: expect.objectContaining({ session_mode }),
        }),
      })
    );
  });

  it("mixed draft: wasl-claude calls launchWaslSession, claude calls launchSession", async () => {
    const claudeNode = makeDraftNode("claude-node", {
      isLead: true,
      driver: "claude",
      project: "/code/claude",
      model: "claude-sonnet-latest",
    });
    const waslNode = makeDraftNode("wasl-node", {
      driver: "wasl-claude",
      project: "/code/wasl",
      model: "claude-sonnet-latest",
      session_mode: "",
    });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [claudeNode, waslNode],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => {
      expect(launchSession).toHaveBeenCalledTimes(1);
      expect(launchWaslSession).toHaveBeenCalledTimes(1);
    }, { timeout: 2000 });

    // claude node used launchSession
    expect(launchSession).toHaveBeenCalledWith(
      expect.objectContaining({ project: "/code/claude", driver: "claude" })
    );
    // wasl-claude node used launchWaslSession
    expect(launchWaslSession).toHaveBeenCalledWith(
      expect.objectContaining({
        wasl_target_key: expect.objectContaining({ protocol: "claude" }),
      })
    );
  });
});

// ===========================================================================
// Test 3: temp→real edge resolution (draft↔real edge)
// ===========================================================================

describe("onConfirmDraft — temp→real edge resolution", () => {
  it("draft edge between two draft nodes resolves both endpoints to real ids", async () => {
    const nodeA = makeDraftNode("draft-A", { isLead: true, project: "/code/A" });
    const nodeB = makeDraftNode("draft-B", { project: "/code/B" });
    const edge = makeDraftEdge("draft-A", "draft-B");

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [nodeA, nodeB],
      draftEdges: [edge],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchSession).toHaveBeenCalledTimes(2), { timeout: 2000 });
    await waitFor(() => expect(addConnection).toHaveBeenCalledTimes(1));

    const [, , from, to] = addConnection.mock.calls[0] as [string, string, string, string, ...unknown[]];
    // Both endpoints must be real ids (real-sess-*), not temp ids
    expect(from).toMatch(/^real-sess-/);
    expect(to).toMatch(/^real-sess-/);
    expect(from).not.toBe("draft-A");
    expect(to).not.toBe("draft-B");
  });

  it("draft↔real edge: draft endpoint replaced with real id, real endpoint unchanged", async () => {
    // Real node "real-existing" is already in the graph.
    // Draft node "draft-new" is authored.
    // Draft edge: real-existing → draft-new (one endpoint is real, one is a temp id).
    const draftNew = makeDraftNode("draft-new", { isLead: false, project: "/code/new" });
    const edgeMixed = makeDraftEdge("real-existing", "draft-new");

    renderGraphConnected({
      wsId: "ws1",
      graph: graphWithRealNode("real-existing", "ws1"),
      draftNodes: [draftNew],
      draftEdges: [edgeMixed],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchSession).toHaveBeenCalledTimes(1), { timeout: 2000 });
    await waitFor(() => expect(addConnection).toHaveBeenCalledTimes(1));

    const [, , from, to] = addConnection.mock.calls[0] as [string, string, string, string, ...unknown[]];
    // The "real-existing" endpoint is already real — must stay as "real-existing"
    expect(from).toBe("real-existing");
    // The draft endpoint must be resolved to the real id returned by launchSession
    expect(to).toMatch(/^real-sess-/);
    expect(to).not.toBe("draft-new");
  });
});

// ===========================================================================
// Test 4: launch failure → draft NOT cleared + error surfaced
// ===========================================================================

describe("onConfirmDraft — launch failure → draft not cleared + designError shown", () => {
  it("when launchSession throws, the draft is not cleared", async () => {
    const captured: { current: VmState | null } = { current: null };
    launchSession.mockRejectedValue(new Error("backend unavailable"));

    const nodeA = makeDraftNode("draft-A", { isLead: true, project: "/code/A" });

    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "graph",
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [nodeA],
      draftEdges: [],
      selectedId: null,
      tab: "Activity",
      filter: "All",
      search: "",
      wsMenuOpen: false,
      draftWsId: "ws1",
      capabilities: claudeCapabilities(),
      board: {
        summary: { workstreams: 0, sessions: 0, hosts: 0 },
        host_chips: [],
        link_chips: [],
        attention: [],
        all: [],
      },
    };

    render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <StateCapture onState={(s) => (captured.current = s)}>
            <GraphScreenConnected />
          </StateCapture>
        </VmProvider>
      </ThemeProvider>
    );

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    // Wait for the async confirm to complete (with failure)
    await waitFor(() => expect(launchSession).toHaveBeenCalledTimes(1), { timeout: 2000 });
    await new Promise((r) => setTimeout(r, 50));

    // Draft must NOT be cleared (draftNodes still contains nodeA)
    expect(captured.current?.draftNodes).toHaveLength(1);
    expect(captured.current?.draftNodes[0].tempId).toBe("draft-A");
  });

  it("when launchSession throws, designError banner is shown", async () => {
    launchSession.mockRejectedValue(new Error("backend unavailable"));

    const nodeA = makeDraftNode("draft-A", { isLead: true, project: "/code/A" });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [nodeA],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchSession).toHaveBeenCalledTimes(1));

    // After failure, the error should be shown
    await waitFor(() => {
      expect(screen.getByTestId("design-conn-error")).toBeTruthy();
    }, { timeout: 1000 });
  });

  it("when launchWaslSession throws for a wasl node, the draft is not cleared", async () => {
    launchWaslSession.mockRejectedValue(new Error("wasl backend unavailable"));

    const captured: { current: VmState | null } = { current: null };
    const waslNode = makeDraftNode("wasl-node", {
      isLead: true,
      driver: "wasl-claude",
      project: "/code/wasl",
      model: "claude-sonnet-latest",
      session_mode: "",
    });

    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "graph",
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [waslNode],
      draftEdges: [],
      selectedId: null,
      tab: "Activity",
      filter: "All",
      search: "",
      wsMenuOpen: false,
      draftWsId: "ws1",
      capabilities: claudeCapabilities(),
      board: {
        summary: { workstreams: 0, sessions: 0, hosts: 0 },
        host_chips: [],
        link_chips: [],
        attention: [],
        all: [],
      },
    };

    render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <StateCapture onState={(s) => (captured.current = s)}>
            <GraphScreenConnected />
          </StateCapture>
        </VmProvider>
      </ThemeProvider>
    );

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => {
      // Either launchWaslSession or launchSession was tried
      return launchWaslSession.mock.calls.length > 0 || launchSession.mock.calls.length > 0;
    }, { timeout: 2000 });
    await new Promise((r) => setTimeout(r, 50));

    // Draft must NOT be cleared
    expect(captured.current?.draftNodes).toHaveLength(1);
  });
});
