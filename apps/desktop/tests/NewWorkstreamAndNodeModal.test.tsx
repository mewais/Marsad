/**
 * SLICE E — New-workstream modal + Node modal — behavioral Vitest/RTL tests.
 *
 * Pattern mirrors GateNewSession.test.tsx: inject VmProvider initialState,
 * wrap in ThemeProvider, mock the api module, assert real behavior.
 */
import React from "react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { VmProvider, VmContext, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NewWorkstreamModal } from "../src/screens/NewWorkstreamModal";
import { NodeModal } from "../src/screens/NodeModal";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { CapabilitiesView } from "../src/bridge/types";
import type { GraphView } from "../src/bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// Mock the api module
// ---------------------------------------------------------------------------

const createWorkstream = vi.fn(
  async (_id: string, _name: string, _goal: string, _profile?: string | null) => {}
);
const launchSession = vi.fn(async (_args: unknown) => "new-sess-1");
const assignSession = vi.fn(async (_sessionId: string, _wsId: string, _lead: boolean) => {});
const addConnection = vi.fn(
  async (_id: string, _ws: string, _from: string, _to: string, _bi: boolean) => {}
);
const deleteWorkstream = vi.fn(async (_id: string) => {});
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
    createWorkstream: (...args: Parameters<typeof original.createWorkstream>) =>
      createWorkstream(...args),
    launchSession: (...args: Parameters<typeof original.launchSession>) =>
      launchSession(...args),
    assignSession: (...args: Parameters<typeof original.assignSession>) =>
      assignSession(...args),
    deleteWorkstream: (...args: Parameters<typeof original.deleteWorkstream>) =>
      deleteWorkstream(...args),
    addConnection: (...args: Parameters<typeof original.addConnection>) =>
      addConnection(...args as [string, string, string, string, boolean]),
    fetchGraph: (...args: Parameters<typeof original.fetchGraph>) => fetchGraph(...args),
    fetchBoard: (...args: Parameters<typeof original.fetchBoard>) => fetchBoard(...args),
  };
});

beforeEach(() => {
  vi.clearAllMocks();
  launchSession.mockResolvedValue("new-sess-1");
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
// State-capture helper (mirrors GateNewSession.test.tsx's StateCapture)
// ---------------------------------------------------------------------------

function StateCapture({
  onState,
  children,
}: {
  onState: (s: VmState) => void;
  children: React.ReactNode;
}) {
  return (
    <VmContext.Consumer>
      {(ctx) => {
        if (ctx) onState(ctx.state);
        return <>{children}</>;
      }}
    </VmContext.Consumer>
  );
}

// ---------------------------------------------------------------------------
// Capabilities fixture — claude is cli_available; codex/opencode are probed and
// KNOWN-false (cli_available: false). BUG #2 made driver availability
// optimistic: only a KNOWN-false probe greys a driver out, so codex/opencode are
// present here explicitly false to keep the "disabled driver" assertions valid.
// ---------------------------------------------------------------------------

function claudeOnlyCapabilities(hostId: string): Record<string, CapabilitiesView> {
  return {
    [`claude::${hostId}`]: {
      driver: "claude",
      host_id: hostId,
      cli_available: true,
      launchable: true,
      models: {
        models: [{ id: "claude-sonnet-4-6" }, { id: "claude-opus-4-8" }],
        source: "Listed",
        free_text: true,
      },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 0,
      errors: [],
    },
    [`codex::${hostId}`]: {
      driver: "codex",
      host_id: hostId,
      cli_available: false,
      launchable: true,
      models: { models: [], source: "Observed", free_text: true },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 1,
      errors: [],
    },
    [`opencode::${hostId}`]: {
      driver: "opencode",
      host_id: hostId,
      cli_available: false,
      launchable: true,
      models: { models: [], source: "Observed", free_text: true },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 1,
      errors: [],
    },
  };
}

// ---------------------------------------------------------------------------
// NewWorkstreamModal
// ---------------------------------------------------------------------------

function renderNewWorkstream(capturedState: { current: VmState | null }) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "new-workstream",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: claudeOnlyCapabilities("local"),
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <StateCapture onState={(s) => (capturedState.current = s)}>
          <NewWorkstreamModal />
        </StateCapture>
      </VmProvider>
    </ThemeProvider>
  );
}

describe("NewWorkstreamModal — two-path choice", () => {
  it("renders name/goal inputs and both path cards", () => {
    renderNewWorkstream({ current: null });
    expect(screen.getByTestId("nw-name-input")).toBeTruthy();
    expect(screen.getByTestId("nw-goal-input")).toBeTruthy();
    expect(screen.getByTestId("nw-path-quick")).toBeTruthy();
    expect(screen.getByTestId("nw-path-build")).toBeTruthy();
  });

  it("Next is disabled until a path is chosen", () => {
    renderNewWorkstream({ current: null });
    const next = screen.getByTestId("nw-next-btn") as HTMLButtonElement;
    expect(next.disabled).toBe(true);
    fireEvent.click(screen.getByTestId("nw-path-quick"));
    expect(next.disabled).toBe(false);
  });

  it("'Single session' path creates the workstream then opens the node modal with isFirstNode", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderNewWorkstream(captured);

    fireEvent.change(screen.getByTestId("nw-name-input"), { target: { value: "Release v2.5" } });
    fireEvent.click(screen.getByTestId("nw-path-quick"));
    fireEvent.click(screen.getByTestId("nw-next-btn"));

    await waitFor(() => {
      expect(createWorkstream).toHaveBeenCalledWith(
        expect.any(String),
        "Release v2.5",
        "",
        undefined
      );
    });
    await waitFor(() => {
      expect(captured.current?.screen).toBe("node-modal");
      expect(captured.current?.nodeModalCtx?.isFirstNode).toBe(true);
      expect(captured.current?.nodeModalCtx?.wsName).toBe("Release v2.5");
    });
    // Build-it-out path never touched — no graph fetch.
    expect(fetchGraph).not.toHaveBeenCalled();
  });

  it("'Build it out' path creates the workstream then navigates to its graph", async () => {
    const captured: { current: VmState | null } = { current: null };
    fetchGraph.mockResolvedValueOnce({
      ws_id: "ignored",
      name: "Untitled workstream",
      goal: "",
      nodes: [],
      edges: [],
      inspector: { working: 0, needs: 0, pending_handoff: null },
    });
    renderNewWorkstream(captured);

    fireEvent.click(screen.getByTestId("nw-path-build"));
    fireEvent.click(screen.getByTestId("nw-next-btn"));

    await waitFor(() => expect(createWorkstream).toHaveBeenCalled());
    await waitFor(() => {
      expect(captured.current?.screen).toBe("graph");
    });
    expect(fetchGraph).toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// NodeModal
// ---------------------------------------------------------------------------

function renderNodeModal(
  captured: { current: VmState | null },
  opts?: {
    isFirstNode?: boolean;
    capabilities?: Record<string, CapabilitiesView>;
    board?: VmState["board"];
    draftWsId?: string | null;
  }
) {
  const isFirstNode = opts?.isFirstNode ?? true;
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "node-modal",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: opts?.capabilities ?? claudeOnlyCapabilities("local"),
    nodeModalCtx: { wsId: "ws-1", wsName: "Release v2.5", isFirstNode },
    board: opts?.board,
    draftWsId: opts?.draftWsId ?? null,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <StateCapture onState={(s) => (captured.current = s)}>
          <NodeModal />
        </StateCapture>
      </VmProvider>
    </ThemeProvider>
  );
}

describe("NodeModal — driver picker (capabilities-driven)", () => {
  it("claude is enabled; codex/opencode are disabled with a 'not yet launchable' note", () => {
    renderNodeModal({ current: null });
    const claudeBtn = screen.getByTestId("nm-driver-claude") as HTMLButtonElement;
    const codexBtn = screen.getByTestId("nm-driver-codex") as HTMLButtonElement;
    const opencodeBtn = screen.getByTestId("nm-driver-opencode") as HTMLButtonElement;
    expect(claudeBtn.disabled).toBe(false);
    expect(codexBtn.disabled).toBe(true);
    expect(opencodeBtn.disabled).toBe(true);
  });

  it("clicking a disabled driver does not change the selection or model catalog", () => {
    renderNodeModal({ current: null });
    fireEvent.click(screen.getByTestId("nm-driver-codex"));
    // Still showing claude's models (codex click was a no-op)
    expect(screen.getByTestId("nm-model-claude-sonnet-4-6")).toBeTruthy();
  });

  it("no hardcoded model list — models come from capabilities for the selected driver/host", () => {
    renderNodeModal(
      { current: null },
      {
        capabilities: {
          "claude::local": {
            driver: "claude",
            host_id: "local",
            cli_available: true,
            launchable: true,
            models: {
              models: [{ id: "totally-custom-model-xyz" }],
              source: "Listed",
              free_text: true,
            },
            session_scope: null,
            sessions: [],
            fetched_at_ms: 0,
            errors: [],
          },
        },
      }
    );
    expect(screen.getByTestId("nm-model-totally-custom-model-xyz")).toBeTruthy();
  });
});

describe("NodeModal — Controlled by / Feeds from", () => {
  it("hidden when the workstream has no existing nodes (first-node / single-session path)", () => {
    renderNodeModal({ current: null }, { isFirstNode: true });
    expect(screen.queryByText("Controlled by")).toBeNull();
    expect(fetchGraph).not.toHaveBeenCalled();
  });

  it("shown, one entry per existing node, when the workstream already has nodes", async () => {
    fetchGraph.mockResolvedValueOnce({
      ws_id: "ws-1",
      name: "Release v2.5",
      goal: "",
      nodes: [
        { id: "n1", name: "build-frontend", driver: "claude", host: "local", state: "Working", status: "working", context_pct: 10, is_lead: true, depth: 0 },
      ],
      edges: [],
      inspector: { working: 1, needs: 0, pending_handoff: null },
    });
    renderNodeModal({ current: null }, { isFirstNode: false });

    await waitFor(() => {
      expect(screen.getByTestId("nm-controller-n1")).toBeTruthy();
      expect(screen.getByTestId("nm-feeder-n1")).toBeTruthy();
    });
  });
});

describe("NodeModal — Launch behavior", () => {
  it("first-node launch calls launchSession, assignSession(lead:true), then selects the session (straight to Level-3)", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderNodeModal(captured, { isFirstNode: true });

    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/release" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => {
      expect(launchSession).toHaveBeenCalledWith(
        expect.objectContaining({ project: "~/code/release", host_id: "local" })
      );
    });
    await waitFor(() => {
      expect(assignSession).toHaveBeenCalledWith("new-sess-1", "ws-1", true);
    });
    await waitFor(() => {
      expect(captured.current?.screen).toBe("session");
      expect(captured.current?.selectedId).toBe("new-sess-1");
      expect(captured.current?.fromGraph).toBe(false);
    });
    expect(addConnection).not.toHaveBeenCalled();
  });

  it("selecting a Controlled-by/Feeds-from node calls addConnection for each wire", async () => {
    fetchGraph.mockResolvedValueOnce({
      ws_id: "ws-1",
      name: "Release v2.5",
      goal: "",
      nodes: [
        { id: "n1", name: "build-frontend", driver: "claude", host: "local", state: "Working", status: "working", context_pct: 10, is_lead: true, depth: 0 },
        { id: "n2", name: "data-pipeline", driver: "claude", host: "local", state: "Working", status: "working", context_pct: 10, is_lead: false, depth: 1 },
      ],
      edges: [],
      inspector: { working: 2, needs: 0, pending_handoff: null },
    });
    const captured: { current: VmState | null } = { current: null };
    renderNodeModal(captured, { isFirstNode: false });

    await waitFor(() => expect(screen.getByTestId("nm-controller-n1")).toBeTruthy());
    fireEvent.change(screen.getByTestId("nm-name-input"), { target: { value: "test-runner" } });
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/code/release" } });
    fireEvent.click(screen.getByTestId("nm-controller-n1"));
    fireEvent.click(screen.getByTestId("nm-feeder-n2"));
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => expect(launchSession).toHaveBeenCalled());
    await waitFor(() => {
      expect(addConnection).toHaveBeenCalledWith(
        expect.any(String),
        "ws-1",
        "n1",
        "new-sess-1",
        false
      );
      expect(addConnection).toHaveBeenCalledWith(
        expect.any(String),
        "ws-1",
        "n2",
        "new-sess-1",
        false
      );
    });
    // Non-first node: goes back to the graph, not straight to a session.
    await waitFor(() => expect(captured.current?.screen).toBe("graph"));
  });

  it("Launch is disabled while the project directory is empty", () => {
    renderNodeModal({ current: null });
    const btn = screen.getByTestId("nm-launch-btn") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// BUG #2 — cancelling the first-node modal discards the orphan draft workstream
// ---------------------------------------------------------------------------

function boardWith(wsId: string, sessionCount: number): VmState["board"] {
  return {
    summary: { workstreams: 1, sessions: sessionCount, hosts: 1 },
    host_chips: [],
    link_chips: [],
    attention: [],
    all: [
      {
        id: wsId,
        name: "Release v2.5",
        goal: "",
        head_state: null,
        driver_dots: [],
        host_chips: [],
        session_count: sessionCount,
        counts: { working: 0, needs: 0, error: 0, done: 0, paused: 0 },
        needs_attention: false,
        is_single_node: false,
      },
    ],
  };
}

describe("NodeModal — draft-discard on cancel (BUG #2)", () => {
  it("cancelling a first-node modal for an empty just-created draft deletes the workstream", async () => {
    // Fresh draft: board card exists but has 0 sessions.
    renderNodeModal({ current: null }, { isFirstNode: true, board: boardWith("ws-1", 0) });

    // Two "Cancel" affordances (header ✕ + footer Cancel) — click the footer one.
    fireEvent.click(screen.getByText("Cancel"));

    await waitFor(() => {
      expect(deleteWorkstream).toHaveBeenCalledWith("ws-1");
    });
  });

  it("cancelling does NOT delete a workstream that already has a session", async () => {
    renderNodeModal({ current: null }, { isFirstNode: true, board: boardWith("ws-1", 1) });

    fireEvent.click(screen.getByText("Cancel"));

    // Give any async delete a chance to fire, then assert it never did.
    await new Promise((r) => setTimeout(r, 30));
    expect(deleteWorkstream).not.toHaveBeenCalled();
  });

  it("cancelling a NON-first-node modal never deletes the workstream", async () => {
    renderNodeModal({ current: null }, { isFirstNode: false, board: boardWith("ws-1", 0) });

    fireEvent.click(screen.getByText("Cancel"));

    await new Promise((r) => setTimeout(r, 30));
    expect(deleteWorkstream).not.toHaveBeenCalled();
  });

  it("launching then the workstream is no longer a discardable draft (launch clears the marker)", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderNodeModal(captured, { isFirstNode: true, board: boardWith("ws-1", 0) });

    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/release" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => expect(launchSession).toHaveBeenCalled());
    // A successful launch navigates to the session; the draft was never discarded.
    await waitFor(() => expect(captured.current?.screen).toBe("session"));
    expect(deleteWorkstream).not.toHaveBeenCalled();
  });
});
