/**
 * Batch-fix frontend tests:
 *   #3/#4 — design-mode entry on a new / empty workstream (launch unblocker).
 *   #4/#2 — NodeModal driver picker + Settings default pre-pick,
 *           driver passed through to launchSession.
 *   #1    — Settings per-driver host command-override fields.
 *   #2    — Settings session defaults: default driver + per-driver model + posture.
 *   #5    — Pause / Resume / Delete lifecycle UI (session + workstream).
 *
 * Pattern mirrors NewWorkstreamAndNodeModal.test.tsx / SessionDetail.test.tsx:
 * inject VmProvider initialState, wrap in ThemeProvider, mock the api module,
 * assert real behavior.
 */
import React from "react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { VmProvider, VmContext, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { GraphScreenConnected } from "../src/screens/graph/GraphScreenConnected";
import { NodeModal } from "../src/screens/NodeModal";
import { Settings } from "../src/screens/Settings";
import { SessionDetail } from "../src/screens/SessionDetail";
import { BoardScreen } from "../src/screens/BoardScreen";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, CapabilitiesView, Settings as SettingsT } from "../src/bridge/types";
import type { GraphView, BoardView } from "../src/bridge/workstreamTypes";
import { DEFAULT_SETTINGS } from "../src/bridge/api";

// ---------------------------------------------------------------------------
// Mocked api surface
// ---------------------------------------------------------------------------

/* eslint-disable @typescript-eslint/no-explicit-any */
const launchSession = vi.fn((..._a: any[]) => Promise.resolve("new-sess-1"));
const assignSession = vi.fn((..._a: any[]) => Promise.resolve());
const emptyBoard: BoardView = {
  summary: { workstreams: 0, sessions: 0, hosts: 0 },
  host_chips: [],
  link_chips: [],
  attention: [],
  all: [],
};
const fetchGraph = vi.fn((..._a: any[]): Promise<GraphView | null> => Promise.resolve(null));
const fetchBoard = vi.fn((..._a: any[]): Promise<BoardView> => Promise.resolve(emptyBoard));
const pauseWorkstream = vi.fn((..._a: any[]) => Promise.resolve());
const resumeWorkstream = vi.fn((..._a: any[]) => Promise.resolve());
const deleteWorkstream = vi.fn((..._a: any[]) => Promise.resolve());
const stopSession = vi.fn((..._a: any[]) => Promise.resolve());
const resumeSession = vi.fn((..._a: any[]) => Promise.resolve());
const removeSession = vi.fn((..._a: any[]) => Promise.resolve());
const updateHost = vi.fn((..._a: any[]) => Promise.resolve());
const addHost = vi.fn((..._a: any[]) => Promise.resolve());
const setSettings = vi.fn((..._a: any[]) => Promise.resolve());
let settingsFixture: SettingsT = { ...DEFAULT_SETTINGS };
const getSettings = vi.fn((..._a: any[]): Promise<SettingsT> => Promise.resolve(settingsFixture));

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  // Wrap in arrow fns so these top-level consts are referenced lazily at call
  // time (the factory itself is hoisted above their initialization).
  return {
    ...original,
    launchSession: (...a: any[]) => launchSession(...a),
    assignSession: (...a: any[]) => assignSession(...a),
    fetchGraph: (...a: any[]) => fetchGraph(...a),
    fetchBoard: (...a: any[]) => fetchBoard(...a),
    pauseWorkstream: (...a: any[]) => pauseWorkstream(...a),
    resumeWorkstream: (...a: any[]) => resumeWorkstream(...a),
    deleteWorkstream: (...a: any[]) => deleteWorkstream(...a),
    stopSession: (...a: any[]) => stopSession(...a),
    resumeSession: (...a: any[]) => resumeSession(...a),
    removeSession: (...a: any[]) => removeSession(...a),
    updateHost: (...a: any[]) => updateHost(...a),
    addHost: (...a: any[]) => addHost(...a),
    getSettings: (...a: any[]) => getSettings(...a),
    setSettings: (...a: any[]) => setSettings(...a),
  };
});

beforeEach(() => {
  vi.clearAllMocks();
  launchSession.mockResolvedValue("new-sess-1");
  fetchGraph.mockResolvedValue(null);
  fetchBoard.mockResolvedValue(emptyBoard);
  settingsFixture = { ...DEFAULT_SETTINGS };
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Capabilities fixtures
// ---------------------------------------------------------------------------

function caps(hostId: string, models: string[], codexAvailable = false): Record<string, CapabilitiesView> {
  return {
    [`claude::${hostId}`]: {
      driver: "claude",
      host_id: hostId,
      cli_available: true,
      launchable: true,
      models: { models: models.map((id) => ({ id })), source: "Listed", free_text: true },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 0,
      errors: [],
    },
    [`codex::${hostId}`]: {
      driver: "codex",
      host_id: hostId,
      cli_available: codexAvailable,
      launchable: true,
      models: { models: codexAvailable ? [{ id: "codex-mini" }] : [], source: "Observed", free_text: true },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 0,
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
      fetched_at_ms: 0,
      errors: [],
    },
  };
}

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

function graphView(nodes: GraphView["nodes"]): GraphView {
  return {
    ws_id: "ws-1",
    name: "Release v2.5",
    goal: "ship it",
    nodes,
    edges: [],
    inspector: { working: nodes.length, needs: 0, pending_handoff: null },
  };
}

// ---------------------------------------------------------------------------
// #3/#4 — design-mode on empty workstream
// ---------------------------------------------------------------------------

describe("#3/#4 GraphScreenConnected — design-mode entry", () => {
  function renderGraph(nodes: GraphView["nodes"], captured?: { current: VmState | null }) {
    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "graph",
      wsId: "ws-1",
      graph: graphView(nodes),
      capabilities: caps("local", ["claude-x-latest"]),
    };
    return render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <StateCapture onState={(s) => captured && (captured.current = s)}>
            <GraphScreenConnected />
          </StateCapture>
        </VmProvider>
      </ThemeProvider>
    );
  }

  it("empty workstream opens in DESIGN mode — shows ＋ Add agent, not ✎ Update workflow", async () => {
    renderGraph([]);
    await waitFor(() => expect(screen.getByTestId("add-agent-btn")).toBeTruthy());
    expect(screen.queryByTestId("enter-design-btn")).toBeNull();
  });

  it("workstream WITH sessions opens in OPERATE mode — shows ✎ Update workflow", async () => {
    renderGraph([
      { id: "n1", name: "lead", driver: "claude", host: "local", state: "Working", status: "working", context_pct: 5, is_lead: true, depth: 0 },
    ]);
    await waitFor(() => expect(screen.getByTestId("enter-design-btn")).toBeTruthy());
    expect(screen.queryByTestId("add-agent-btn")).toBeNull();
  });

  it("＋ Add agent on an empty workstream opens the node modal with isFirstNode:true", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderGraph([], captured);
    await waitFor(() => expect(screen.getByTestId("add-agent-btn")).toBeTruthy());
    fireEvent.click(screen.getByTestId("add-agent-btn"));
    await waitFor(() => {
      expect(captured.current?.screen).toBe("node-modal");
      expect(captured.current?.nodeModalCtx?.isFirstNode).toBe(true);
    });
  });
});

// ---------------------------------------------------------------------------
// #4/#2 — NodeModal driver + defaults pre-pick
// ---------------------------------------------------------------------------

function renderNodeModal(capabilities: Record<string, CapabilitiesView>) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "node-modal",
    capabilities,
    nodeModalCtx: { wsId: "ws-1", wsName: "Release v2.5", isFirstNode: true },
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NodeModal />
      </VmProvider>
    </ThemeProvider>
  );
}

describe("#4/#2 NodeModal — driver + defaults", () => {
  it("passes the selected driver to launchSession", async () => {
    renderNodeModal(caps("local", ["m-a", "m-b"]));
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/code/x" } });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));
    await waitFor(() => {
      expect(launchSession).toHaveBeenCalledWith(
        expect.objectContaining({ driver: "claude", project: "~/code/x" })
      );
    });
  });

  it("pre-picks the Settings default model for the driver when it exists in the catalog", async () => {
    settingsFixture = { ...DEFAULT_SETTINGS, default_models: [["claude", "m-b"]] };
    renderNodeModal(caps("local", ["m-a", "m-b"]));
    // Wait for the async Settings load to seed the model pre-pick. `m-b` is a
    // catalog entry, so the free-text box stays EMPTY (#1 fix) and the `m-b`
    // radio becomes selected (accent border/color) instead.
    const freetext = screen.getByTestId("nm-model-freetext") as HTMLInputElement;
    expect(freetext.value).toBe("");
    await waitFor(() => {
      const mb = screen.getByTestId("nm-model-m-b") as HTMLButtonElement;
      expect(mb.style.color).toContain("accent");
    });
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/code/x" } });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));
    await waitFor(() => {
      expect(launchSession).toHaveBeenCalledWith(expect.objectContaining({ model: "m-b" }));
    });
  });

  it("passes session_mode (empty when no modes in catalog) to launchSession", async () => {
    // With no available_modes in the capability, sessionMode defaults to "".
    renderNodeModal(caps("local", ["m-a"]));
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/code/x" } });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));
    await waitFor(() => {
      expect(launchSession).toHaveBeenCalledWith(expect.objectContaining({ session_mode: "" }));
    });
  });

  it("opencode is disabled (cli_available:false) with a not-yet-launchable note", () => {
    renderNodeModal(caps("local", ["m-a"]));
    expect((screen.getByTestId("nm-driver-opencode") as HTMLButtonElement).disabled).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// #4 — NodeModal driver picker + driver passthrough (migrated from NewSession)
// ---------------------------------------------------------------------------

describe("#4 NodeModal — driver picker", () => {
  it("renders a driver picker; opencode disabled; claude enabled", () => {
    renderNodeModal(caps("local", ["m-a"]));
    expect((screen.getByTestId("nm-driver-claude") as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByTestId("nm-driver-opencode") as HTMLButtonElement).disabled).toBe(true);
  });

  it("renders a button for all three known drivers (claude, codex, opencode)", () => {
    renderNodeModal(caps("local", ["m-a"]));
    expect(screen.getByTestId("nm-driver-claude")).toBeTruthy();
    expect(screen.getByTestId("nm-driver-codex")).toBeTruthy();
    expect(screen.getByTestId("nm-driver-opencode")).toBeTruthy();
  });

  it("passes the driver to launchSession", async () => {
    renderNodeModal(caps("local", ["m-a"]));
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/code/x" } });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));
    await waitFor(() => {
      expect(launchSession).toHaveBeenCalledWith(expect.objectContaining({ driver: "claude" }));
    });
  });
});

// ---------------------------------------------------------------------------
// #1 / #2 — Settings
// ---------------------------------------------------------------------------

describe("#1/#2 Settings", () => {
  function renderSettings(capabilities: Record<string, CapabilitiesView>) {
    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "settings",
      capabilities,
    };
    return render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <Settings />
        </VmProvider>
      </ThemeProvider>
    );
  }

  it("#1 Hosts: local card has per-driver Codex/opencode command fields; save wires agentCommands", async () => {
    renderSettings(caps("local", ["m-a"]));
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    fireEvent.change(screen.getByTestId("local-codex-cmd"), { target: { value: "codex --sandbox" } });
    fireEvent.change(screen.getByTestId("local-opencode-cmd"), { target: { value: "/opt/opencode" } });
    fireEvent.click(screen.getByTestId("local-save-btn"));
    await waitFor(() => {
      expect(updateHost).toHaveBeenCalled();
    });
    // agentCommands is the 6th positional arg of updateHost.
    const call = updateHost.mock.calls[0] as unknown[];
    const agentCommands = call[5] as Array<[string, string]>;
    expect(agentCommands).toEqual(
      expect.arrayContaining([
        ["codex", "codex --sandbox"],
        ["opencode", "/opt/opencode"],
      ])
    );
  });

  it("#1 Hosts: add-host form has Codex/opencode command fields", async () => {
    renderSettings(caps("local", ["m-a"]));
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    expect(screen.getByTestId("host-add-codex-cmd")).toBeTruthy();
    expect(screen.getByTestId("host-add-opencode-cmd")).toBeTruthy();
  });

  it("#2 Session defaults: driver picker persists default_driver", async () => {
    settingsFixture = { ...DEFAULT_SETTINGS, default_driver: "claude" };
    renderSettings(caps("local", ["m-a"]));
    fireEvent.click(screen.getByTestId("settings-tab-session-defaults"));
    fireEvent.click(screen.getByTestId("defaults-driver-codex"));
    await waitFor(() => {
      expect(setSettings).toHaveBeenCalledWith(expect.objectContaining({ default_driver: "codex" }));
    });
  });

  it("#2 Session defaults: per-driver model persists into default_models", async () => {
    renderSettings(caps("local", ["m-a", "m-b"]));
    fireEvent.click(screen.getByTestId("settings-tab-session-defaults"));
    fireEvent.click(screen.getByTestId("defaults-model-m-b"));
    await waitFor(() => {
      const calls = setSettings.mock.calls;
      const last = calls[calls.length - 1]?.[0] as SettingsT;
      expect(last.default_models).toEqual(expect.arrayContaining([["claude", "m-b"]]));
    });
  });
});

// ---------------------------------------------------------------------------
// #5 — lifecycle UI
// ---------------------------------------------------------------------------

describe("#5 Session lifecycle (SessionDetail)", () => {
  function renderSD(selectedId: string) {
    const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, selected_id: selectedId };
    const preloaded: Partial<VmState> = {
      snapshot,
      screen: "session",
      selectedId,
      capabilities: caps("local", ["m-a"]),
    };
    return render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <SessionDetail />
        </VmProvider>
      </ThemeProvider>
    );
  }

  it("running session shows Pause (stopSession) + Delete (removeSession, confirmed)", () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    renderSD("s01"); // s01 is Working
    fireEvent.click(screen.getByTestId("pause-btn"));
    expect(stopSession).toHaveBeenCalledWith("s01");
    fireEvent.click(screen.getByTestId("remove-btn"));
    expect(removeSession).toHaveBeenCalledWith("s01");
    confirmSpy.mockRestore();
  });

  it("Delete is a no-op when the confirm is declined", () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    renderSD("s01");
    fireEvent.click(screen.getByTestId("remove-btn"));
    expect(removeSession).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });
});

describe("#5 Workstream lifecycle (Board)", () => {
  // State-appropriate actions: running workstream → Pause button;
  // stopped/done workstream → Resume button. Each board row shows only one
  // lifecycle action at a time (not both simultaneously).
  const workingBoardWith: BoardView = {
    summary: { workstreams: 1, sessions: 2, hosts: 1 },
    host_chips: [{ id: "local", label: "localhost", kind: "local", status: "connected" }],
    link_chips: [],
    attention: [],
    all: [
      {
        id: "ws-1",
        name: "Release v2.5",
        goal: "ship",
        head_state: "working",
        driver_dots: ["claude"],
        host_chips: ["local"],
        session_count: 2,
        counts: { working: 2, needs: 0, error: 0, done: 0, paused: 0 },
        needs_attention: false,
        is_single_node: false,
      },
    ],
  };

  const doneBoardWith: BoardView = {
    ...workingBoardWith,
    all: [
      {
        ...workingBoardWith.all[0],
        head_state: "done",
        counts: { working: 0, needs: 0, error: 0, done: 2, paused: 0 },
      },
    ],
  };

  function renderBoardWith(board: BoardView) {
    fetchBoard.mockResolvedValue(board);
    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "board",
      board,
      capabilities: caps("local", ["m-a"]),
    };
    return render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <BoardScreen />
        </VmProvider>
      </ThemeProvider>
    );
  }

  // Keep backward-compat alias for the Delete tests below.
  function renderBoard() { return renderBoardWith(workingBoardWith); }

  it("running workstream: Pause button calls pauseWorkstream", async () => {
    renderBoardWith(workingBoardWith);
    // Working → only Pause visible (no Resume)
    expect(screen.queryByTestId("ws-resume-ws-1")).toBeNull();
    fireEvent.click(screen.getByTestId("ws-pause-ws-1"));
    await waitFor(() => expect(pauseWorkstream).toHaveBeenCalledWith("ws-1"));
  });

  it("done workstream: Resume button calls resumeWorkstream", async () => {
    renderBoardWith(doneBoardWith);
    // Done → only Resume visible (no Pause)
    expect(screen.queryByTestId("ws-pause-ws-1")).toBeNull();
    fireEvent.click(screen.getByTestId("ws-resume-ws-1"));
    await waitFor(() => expect(resumeWorkstream).toHaveBeenCalledWith("ws-1"));
  });

  it("Delete calls deleteWorkstream after a confirm", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    renderBoard();
    fireEvent.click(screen.getByTestId("ws-delete-ws-1"));
    await waitFor(() => expect(deleteWorkstream).toHaveBeenCalledWith("ws-1"));
    confirmSpy.mockRestore();
  });

  it("Delete does nothing when the confirm is declined", () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    renderBoard();
    fireEvent.click(screen.getByTestId("ws-delete-ws-1"));
    expect(deleteWorkstream).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });
});

describe("#5 Workstream lifecycle (Graph inspector)", () => {
  // BUG #6 — Pause-all / Resume-all are now STATEFUL: a graph node is treated as
  // paused when it isn't live. The connected wrapper prefers each node's
  // SessionView.live (matching the dashboard) and falls back to the graph Phase
  // (Stale/Detached = paused) when no matching SessionView exists. These graph
  // fixture ids have no SessionView in MOCK_SNAPSHOT, so the Phase fallback drives
  // the conditions: Working ⇒ running ⇒ Pause only; Stale/Detached ⇒ Resume only.
  function renderGraph(nodes: GraphView["nodes"]) {
    const g = graphView(nodes);
    // Post-mutation refresh re-fetches the graph; keep operate-mode nodes present.
    fetchGraph.mockResolvedValue(g);
    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "graph",
      wsId: "ws-1",
      graph: g,
      capabilities: caps("local", ["m-a"]),
    };
    return render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <GraphScreenConnected />
        </VmProvider>
      </ThemeProvider>
    );
  }

  const runningNode: GraphView["nodes"][number] = { id: "n1", name: "lead", driver: "claude", host: "local", state: "Working", status: "working", context_pct: 5, is_lead: true, depth: 0 };
  const stoppedNode: GraphView["nodes"][number] = { id: "n2", name: "worker", driver: "claude", host: "local", state: "Stale", status: "paused", context_pct: 0, is_lead: false, depth: 1 };

  it("all-running workstream: only Pause all is shown and calls pauseWorkstream", async () => {
    renderGraph([runningNode]);
    await waitFor(() => expect(screen.getByTestId("inspector-pause-ws")).toBeTruthy());
    // Nothing paused → no Resume all.
    expect(screen.queryByTestId("inspector-resume-ws")).toBeNull();
    fireEvent.click(screen.getByTestId("inspector-pause-ws"));
    await waitFor(() => expect(pauseWorkstream).toHaveBeenCalledWith("ws-1"));
  });

  it("all-stopped workstream: only Resume all is shown and calls resumeWorkstream", async () => {
    renderGraph([stoppedNode]);
    await waitFor(() => expect(screen.getByTestId("inspector-resume-ws")).toBeTruthy());
    // Nothing running → no Pause all.
    expect(screen.queryByTestId("inspector-pause-ws")).toBeNull();
    fireEvent.click(screen.getByTestId("inspector-resume-ws"));
    await waitFor(() => expect(resumeWorkstream).toHaveBeenCalledWith("ws-1"));
  });

  it("mixed workstream: both Pause all and Resume all are shown", async () => {
    renderGraph([runningNode, stoppedNode]);
    await waitFor(() => expect(screen.getByTestId("inspector-pause-ws")).toBeTruthy());
    expect(screen.getByTestId("inspector-resume-ws")).toBeTruthy();
    fireEvent.click(screen.getByTestId("inspector-pause-ws"));
    await waitFor(() => expect(pauseWorkstream).toHaveBeenCalledWith("ws-1"));
    fireEvent.click(screen.getByTestId("inspector-resume-ws"));
    await waitFor(() => expect(resumeWorkstream).toHaveBeenCalledWith("ws-1"));
  });

  it("inspector Delete calls deleteWorkstream after confirm", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    renderGraph([runningNode]);
    await waitFor(() => expect(screen.getByTestId("inspector-delete-ws")).toBeTruthy());
    fireEvent.click(screen.getByTestId("inspector-delete-ws"));
    await waitFor(() => expect(deleteWorkstream).toHaveBeenCalledWith("ws-1"));
    confirmSpy.mockRestore();
  });
});
