/**
 * onConfirmDraft — materializing a "Build it out" draft.
 *
 * Post-#8: a build-it-out (design-mode) workstream launches EVERY agent over the
 * Wasl transport — the picked driver is the NATIVE protocol, launched via Wasl
 * behind the scenes (the user only ever sees native names). So onConfirmDraft
 * always calls `launchWaslSession` (never `launchSession`), with the target key
 * carrying `protocol = node.driver` and `context: { cwd, model, session_mode }`.
 *
 * Verified here:
 *   1. Nodes launch in DEPENDENCY ORDER (via orderDraftLaunch) — lead/depth-0
 *      first, dependents after — regardless of authored order.
 *   2. Each node → launchWaslSession with the correct target-key shape; the
 *      native driver id becomes the wasl protocol. `launchSession` is NEVER used.
 *   3. temp→real edge resolution, including a draft↔real edge.
 *   4. On launch failure the draft is NOT cleared and designError is surfaced.
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

/** Read each launchWaslSession call's cwd (context.cwd), in call order. */
function launchedCwds(): string[] {
  return launchWaslSession.mock.calls.map(
    (c) => (c[0] as { wasl_target_key: { context: { cwd: string } } }).wasl_target_key.context.cwd
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
  return { tempId: `de-${from}-${to}`, from, to, bi: false };
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
    const draftA = makeDraftNode("draft-A", { isLead: true, name: "Agent A", project: "/code/A" });
    const draftB = makeDraftNode("draft-B", { name: "Agent B", project: "/code/B" });
    const draftC = makeDraftNode("draft-C", { name: "Agent C", project: "/code/C" });
    const edges = [
      makeDraftEdge("draft-A", "draft-B"),
      makeDraftEdge("draft-B", "draft-C"),
    ];

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [draftC, draftB, draftA],
      draftEdges: edges,
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(3), { timeout: 2000 });
    expect(launchSession).not.toHaveBeenCalled();

    const cwds = launchedCwds();
    expect(cwds[0]).toBe("/code/A");
    expect(cwds[1]).toBe("/code/B");
    expect(cwds[2]).toBe("/code/C");
  });

  it("lead (depth 0) is always first, even when authored last", async () => {
    const lead = makeDraftNode("lead", { isLead: true, name: "Lead", project: "/code/lead" });
    const sibling = makeDraftNode("sibling", { name: "Sibling", project: "/code/sibling" });
    const dep = makeDraftNode("dep", { name: "Dep", project: "/code/dep" });
    const edges = [makeDraftEdge("lead", "dep")];

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [dep, sibling, lead],
      draftEdges: edges,
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(3), { timeout: 2000 });

    const cwds = launchedCwds();
    expect(cwds[0]).toBe("/code/lead");
    expect(cwds.indexOf("/code/lead")).toBeLessThan(cwds.indexOf("/code/dep"));
  });
});

// ===========================================================================
// Test 2: every build-it-out node → launchWaslSession (native protocol)
// ===========================================================================

describe("onConfirmDraft — launchWaslSession for every draft node", () => {
  it("calls launchWaslSession (not launchSession) for a native draft node", async () => {
    const node = makeDraftNode("node", {
      isLead: true,
      driver: "claude",
      project: "/code/proj",
      name: "Agent",
    });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [node],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1), { timeout: 2000 });
    expect(launchSession).not.toHaveBeenCalled();
  });

  it("wasl_target_key.protocol === the node's native driver", async () => {
    const node = makeDraftNode("node", { isLead: true, driver: "claude", project: "/code/proj" });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [node],
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

  it("context.cwd / model / session_mode mirror the draft node", async () => {
    const node = makeDraftNode("node", {
      isLead: true,
      driver: "claude",
      project: "/code/wasl-proj",
      model: "claude-opus-latest",
      session_mode: "plan",
    });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [node],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1));
    expect(launchWaslSession).toHaveBeenCalledWith(
      expect.objectContaining({
        wasl_target_key: expect.objectContaining({
          context: expect.objectContaining({
            cwd: "/code/wasl-proj",
            model: "claude-opus-latest",
            session_mode: "plan",
          }),
        }),
      })
    );
  });

  it("two native drivers both launch via Wasl with their respective protocols", async () => {
    const claudeNode = makeDraftNode("claude-node", {
      isLead: true,
      driver: "claude",
      project: "/code/claude",
    });
    const codexNode = makeDraftNode("codex-node", {
      driver: "codex",
      project: "/code/codex",
    });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [claudeNode, codexNode],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(2), { timeout: 2000 });
    expect(launchSession).not.toHaveBeenCalled();

    const protocols = launchWaslSession.mock.calls.map(
      (c) => (c[0] as { wasl_target_key: { protocol: string } }).wasl_target_key.protocol
    );
    expect(protocols).toContain("claude");
    expect(protocols).toContain("codex");
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

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(2), { timeout: 2000 });
    await waitFor(() => expect(addConnection).toHaveBeenCalledTimes(1));

    const [, , from, to] = addConnection.mock.calls[0] as [string, string, string, string, ...unknown[]];
    // Both endpoints must be real ids (real-wasl-*), not temp ids.
    expect(from).toMatch(/^real-wasl-/);
    expect(to).toMatch(/^real-wasl-/);
    expect(from).not.toBe("draft-A");
    expect(to).not.toBe("draft-B");
  });

  it("draft↔real edge: draft endpoint replaced with real id, real endpoint unchanged", async () => {
    const draftNew = makeDraftNode("draft-new", { isLead: false, project: "/code/new" });
    const edgeMixed = makeDraftEdge("real-existing", "draft-new");

    renderGraphConnected({
      wsId: "ws1",
      graph: graphWithRealNode("real-existing", "ws1"),
      draftNodes: [draftNew],
      draftEdges: [edgeMixed],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1), { timeout: 2000 });
    await waitFor(() => expect(addConnection).toHaveBeenCalledTimes(1));

    const [, , from, to] = addConnection.mock.calls[0] as [string, string, string, string, ...unknown[]];
    // The already-real endpoint stays; the draft endpoint resolves to a real id.
    expect(from).toBe("real-existing");
    expect(to).toMatch(/^real-wasl-/);
    expect(to).not.toBe("draft-new");
  });
});

// ===========================================================================
// Test 4: launch failure → draft NOT cleared + error surfaced
// ===========================================================================

describe("onConfirmDraft — launch failure → draft not cleared + designError shown", () => {
  it("when launchWaslSession throws, the draft is not cleared", async () => {
    const captured: { current: VmState | null } = { current: null };
    launchWaslSession.mockRejectedValue(new Error("wasl backend unavailable"));

    const nodeA = makeDraftNode("draft-A", { isLead: true, project: "/code/A" });

    renderGraphConnected(
      {
        wsId: "ws1",
        graph: emptyGraph("ws1"),
        draftNodes: [nodeA],
        draftEdges: [],
        draftWsId: "ws1",
      },
      captured
    );

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1), { timeout: 2000 });
    await new Promise((r) => setTimeout(r, 50));

    expect(captured.current?.draftNodes).toHaveLength(1);
    expect(captured.current?.draftNodes[0].tempId).toBe("draft-A");
  });

  it("when launchWaslSession throws, designError banner is shown", async () => {
    launchWaslSession.mockRejectedValue(new Error("wasl backend unavailable"));

    const nodeA = makeDraftNode("draft-A", { isLead: true, project: "/code/A" });

    renderGraphConnected({
      wsId: "ws1",
      graph: emptyGraph("ws1"),
      draftNodes: [nodeA],
      draftEdges: [],
    });

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1));

    await waitFor(() => {
      expect(screen.getByTestId("design-conn-error")).toBeTruthy();
    }, { timeout: 1000 });
  });
});
