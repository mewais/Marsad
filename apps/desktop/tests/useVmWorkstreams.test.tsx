/**
 * Vitest suite for the U-INT-2 desktop useVm workstream integration.
 * See docs/specs/ui-usevm-integration.md §Test plan (V1-V10).
 *
 * This is test-first: it exercises reducer actions (SET_BOARD, OPEN_GRAPH,
 * OPEN_BOARD, SELECT_NODE, SET_ACTIVE_PROFILE, VM_UPDATE board/workstreams)
 * and the new command helpers (createWorkstream, addConnection,
 * setActiveProfile, deleteWorkstream, assignSession, createProfile,
 * deleteProfile, openWorkstream, openBoard, openNode) that `useVm()` must
 * expose per the spec. None of this exists yet on `src/bridge/useVm.tsx` —
 * this file is expected to fail to typecheck/run until the code-agent lands
 * the additive extension described in the spec.
 *
 * Render style mirrors tests/Overview.test.tsx: a VmProvider wrapping a
 * probe/consumer component that reads useVm() and can trigger dispatch /
 * command helpers via clickable buttons, observed through onRender capture.
 */
import { describe, it, expect, beforeEach, vi, afterEach } from "vitest";
import { render, waitFor, act } from "@testing-library/react";
import React from "react";
import { VmProvider, VmContext, VmState } from "../src/bridge/useVm";
import { mockWorkstreams } from "../src/bridge/mockWorkstreams";
import * as api from "../src/bridge/api";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { BoardView, GraphView } from "../src/bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// Seed knowledge (from src/bridge/mockWorkstreams.ts seedState()):
//   - "ws-auth-refactor" has exactly 1 member session (s01)
//     => is_single_node === true.
//   - "ws-build-pipeline" has 2 member sessions (s07,
//     s06) => is_single_node === false (multi-node).
// ---------------------------------------------------------------------------
const SINGLE_NODE_WS_ID = "ws-auth-refactor";
const MULTI_NODE_WS_ID = "ws-build-pipeline";

// ---------------------------------------------------------------------------
// Probe: renders nothing visible itself, but exposes the live useVm()
// context value to the test via a captor callback invoked on every render.
// ---------------------------------------------------------------------------

function Probe({ onCtx }: { onCtx: (ctx: ReturnType<typeof useCtxType>) => void }) {
  const ctx = React.useContext(VmContext);
  React.useEffect(() => {
    if (ctx) onCtx(ctx);
  });
  return null;
}
// Type helper only — never called at runtime.
declare function useCtxType(): NonNullable<React.ContextType<typeof VmContext>>;

function preloadedState(overrides: Partial<VmState> = {}): Partial<VmState> {
  return {
    snapshot: MOCK_SNAPSHOT,
    screen: "overview",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    ...overrides,
  };
}

/** Render VmProvider + Probe, returning a getter for the latest ctx value. */
function renderVm(overrides: Partial<VmState> = {}) {
  let latest: any = null;
  const capture = (ctx: any) => {
    latest = ctx;
  };
  const utils = render(
    <VmProvider initialState={preloadedState(overrides)}>
      <Probe onCtx={capture} />
    </VmProvider>
  );
  return {
    ...utils,
    getCtx: () => latest as {
      state: VmState & {
        board: BoardView | null;
        wsId: string | null;
        graph: GraphView | null;
        nodeSel: string | null;
        activeProfile: string | null;
        screen: string;
      };
      dispatch: React.Dispatch<any>;
      // Command helpers (asserted to exist by the spec).
      openBoard: () => void;
      openWorkstream: (id: string) => Promise<void>;
      selectNode: (id: string | null) => void;
      openNode: (sessionId: string) => void;
      createWorkstream: (
        id: string,
        name: string,
        goal: string,
        profile?: string | null
      ) => Promise<void>;
      deleteWorkstream: (id: string) => Promise<void>;
      addConnection: (
        id: string,
        ws: string,
        from: string,
        to: string,
        kind: string,
        bi: boolean
      ) => Promise<void>;
      removeConnection: (id: string) => Promise<void>;
      createProfile: (id: string, name: string) => Promise<void>;
      deleteProfile: (id: string) => Promise<void>;
      assignSession: (
        sessionId: string,
        wsId: string,
        lead: boolean
      ) => Promise<void>;
      setActiveProfile: (profile: string | null) => Promise<void>;
    },
  };
}

beforeEach(() => {
  mockWorkstreams.reset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// V1 — reducer: SET_BOARD / OPEN_GRAPH / OPEN_BOARD / SELECT_NODE /
//      SET_ACTIVE_PROFILE
// ---------------------------------------------------------------------------

describe("V1: new reducer actions", () => {
  it("SET_BOARD sets state.board to the dispatched board", async () => {
    const { getCtx } = renderVm();
    const board = await api.fetchBoard();

    act(() => {
      getCtx().dispatch({ type: "SET_BOARD", board });
    });

    await waitFor(() => {
      expect(getCtx().state.board).toEqual(board);
    });
  });

  it("OPEN_GRAPH sets screen 'graph', sets wsId, and clears nodeSel", async () => {
    const { getCtx } = renderVm({
      // Pre-seed a stray nodeSel so we can prove OPEN_GRAPH clears it.
      // (Cast through `as any` isn't needed — VmState is additively extended.)
      ...( { nodeSel: "stale-node" } as Partial<VmState> ),
    });

    act(() => {
      getCtx().dispatch({ type: "OPEN_GRAPH", wsId: MULTI_NODE_WS_ID });
    });

    await waitFor(() => {
      expect(getCtx().state.screen).toBe("graph");
      expect(getCtx().state.wsId).toBe(MULTI_NODE_WS_ID);
      expect(getCtx().state.nodeSel).toBeNull();
    });
  });

  it("OPEN_BOARD sets screen 'board' and clears wsId/graph/nodeSel", async () => {
    const { getCtx } = renderVm();

    act(() => {
      getCtx().dispatch({ type: "OPEN_GRAPH", wsId: MULTI_NODE_WS_ID });
    });
    await waitFor(() => expect(getCtx().state.wsId).toBe(MULTI_NODE_WS_ID));

    act(() => {
      getCtx().dispatch({ type: "SELECT_NODE", id: "s07" });
    });
    await waitFor(() => expect(getCtx().state.nodeSel).toBe("s07"));

    act(() => {
      getCtx().dispatch({ type: "OPEN_BOARD" });
    });

    await waitFor(() => {
      expect(getCtx().state.screen).toBe("board");
      expect(getCtx().state.wsId).toBeNull();
      expect(getCtx().state.graph).toBeNull();
      expect(getCtx().state.nodeSel).toBeNull();
    });
  });

  it("SELECT_NODE sets nodeSel to the given id", async () => {
    const { getCtx } = renderVm();

    act(() => {
      getCtx().dispatch({ type: "SELECT_NODE", id: "s06" });
    });

    await waitFor(() => {
      expect(getCtx().state.nodeSel).toBe("s06");
    });
  });

  it("SELECT_NODE with null clears nodeSel", async () => {
    const { getCtx } = renderVm();

    act(() => {
      getCtx().dispatch({ type: "SELECT_NODE", id: "s06" });
    });
    await waitFor(() => expect(getCtx().state.nodeSel).toBe("s06"));

    act(() => {
      getCtx().dispatch({ type: "SELECT_NODE", id: null });
    });

    await waitFor(() => {
      expect(getCtx().state.nodeSel).toBeNull();
    });
  });

  it("SET_ACTIVE_PROFILE sets state.activeProfile", async () => {
    const { getCtx } = renderVm();

    act(() => {
      getCtx().dispatch({ type: "SET_ACTIVE_PROFILE", profile: "prof-eng" });
    });

    await waitFor(() => {
      expect(getCtx().state.activeProfile).toBe("prof-eng");
    });
  });
});

// ---------------------------------------------------------------------------
// V2 — regression guard: pre-existing actions still work
// ---------------------------------------------------------------------------

describe("V2: regression guard — existing actions unaffected", () => {
  it("SET_SCREEN still sets state.screen (existing action)", async () => {
    const { getCtx } = renderVm();

    act(() => {
      getCtx().dispatch({ type: "SET_SCREEN", screen: "settings" });
    });

    await waitFor(() => {
      expect(getCtx().state.screen).toBe("settings");
    });
  });

  it("SELECT_SESSION still sets selectedId and navigates to 'session' (existing action)", async () => {
    const { getCtx } = renderVm();

    act(() => {
      getCtx().dispatch({ type: "SELECT_SESSION", id: "s01" });
    });

    await waitFor(() => {
      expect(getCtx().state.selectedId).toBe("s01");
      expect(getCtx().state.screen).toBe("session");
    });
  });

  it("VM_UPDATE {kind:'hosts'} still replaces snapshot.hosts (existing case, no regression)", async () => {
    const { getCtx } = renderVm();
    const newHosts = [
      { id: "h-new", label: "new-host", status: "connected" as const },
    ];

    act(() => {
      getCtx().dispatch({
        type: "VM_UPDATE",
        payload: { kind: "hosts", hosts: newHosts } as any,
      });
    });

    await waitFor(() => {
      expect(getCtx().state.snapshot?.hosts).toEqual(newHosts);
    });
  });
});

// ---------------------------------------------------------------------------
// V3 — VM_UPDATE board / session
// ---------------------------------------------------------------------------

describe("V3: VM_UPDATE board + session (no regression)", () => {
  it("VM_UPDATE {kind:'board', board} updates state.board", async () => {
    const { getCtx } = renderVm();
    const board = await api.fetchBoard();

    act(() => {
      getCtx().dispatch({
        type: "VM_UPDATE",
        payload: { kind: "board", board } as any,
      });
    });

    await waitFor(() => {
      expect(getCtx().state.board).toEqual(board);
    });
  });

  it("VM_UPDATE {kind:'session'} still upserts a session (existing behavior, no regression)", async () => {
    const { getCtx } = renderVm();
    const existing = MOCK_SNAPSHOT.sessions[0];
    const updated = { ...existing, name: "renamed-by-update" };

    act(() => {
      getCtx().dispatch({
        type: "VM_UPDATE",
        payload: { kind: "session", session: updated } as any,
      });
    });

    await waitFor(() => {
      const found = getCtx().state.snapshot?.sessions.find(
        (s) => s.id === existing.id
      );
      expect(found?.name).toBe("renamed-by-update");
    });
  });
});

// ---------------------------------------------------------------------------
// V4 — createWorkstream helper: calls api AND refreshes board
// ---------------------------------------------------------------------------

describe("V4: createWorkstream helper", () => {
  it("calls api.createWorkstream and results in state.board.all containing the new workstream", async () => {
    const spy = vi.spyOn(api, "createWorkstream");
    const { getCtx } = renderVm();

    // Ensure board is loaded first so we can observe the refresh.
    act(() => {
      getCtx().openBoard();
    });
    await waitFor(() => expect(getCtx().state.board).not.toBeNull());

    await act(async () => {
      await getCtx().createWorkstream(
        "ws-v4-new",
        "V4 new workstream",
        "prove end-to-end wiring",
        null
      );
    });

    expect(spy).toHaveBeenCalledWith(
      "ws-v4-new",
      "V4 new workstream",
      "prove end-to-end wiring",
      null
    );

    await waitFor(() => {
      const card = getCtx().state.board?.all.find((c) => c.id === "ws-v4-new");
      expect(card).toBeTruthy();
      expect(card?.name).toBe("V4 new workstream");
    });
  });
});

// ---------------------------------------------------------------------------
// V5 — addConnection helper: open a ws first, then add, graph reflects it
// ---------------------------------------------------------------------------

describe("V5: addConnection helper", () => {
  it("calls api.addConnection and the open graph reflects the new edge", async () => {
    const spy = vi.spyOn(api, "addConnection");
    const { getCtx } = renderVm();

    await act(async () => {
      await getCtx().openWorkstream(MULTI_NODE_WS_ID);
    });
    await waitFor(() => {
      expect(getCtx().state.screen).toBe("graph");
      expect(getCtx().state.graph).not.toBeNull();
    });

    const nodes = getCtx().state.graph!.nodes;
    expect(nodes.length).toBeGreaterThanOrEqual(2);
    const [nodeA, nodeB] = nodes;

    await act(async () => {
      await getCtx().addConnection(
        "conn-v5-new",
        MULTI_NODE_WS_ID,
        nodeA.id,
        nodeB.id,
        "data",
        false
      );
    });

    expect(spy).toHaveBeenCalledWith(
      "conn-v5-new",
      MULTI_NODE_WS_ID,
      nodeA.id,
      nodeB.id,
      "data",
      false
    );

    await waitFor(() => {
      const edge = getCtx().state.graph?.edges.find((e) => e.id === "conn-v5-new");
      expect(edge).toBeTruthy();
      expect(edge?.from).toBe(nodeA.id);
      expect(edge?.to).toBe(nodeB.id);
    });
  });
});

// ---------------------------------------------------------------------------
// V6 — openWorkstream on a MULTI-node workstream navigates to "graph"
// ---------------------------------------------------------------------------

describe("V6: openWorkstream — multi-node workstream", () => {
  it("navigates to screen 'graph', sets wsId, and populates graph", async () => {
    const { getCtx } = renderVm();

    // Sanity: confirm this seed id is indeed multi-node before asserting nav.
    const board = await api.fetchBoard();
    const card = board.all.find((c) => c.id === MULTI_NODE_WS_ID);
    expect(card).toBeTruthy();
    expect(card!.is_single_node).toBe(false);

    await act(async () => {
      await getCtx().openWorkstream(MULTI_NODE_WS_ID);
    });

    await waitFor(() => {
      expect(getCtx().state.screen).toBe("graph");
      expect(getCtx().state.wsId).toBe(MULTI_NODE_WS_ID);
      expect(getCtx().state.graph).not.toBeNull();
      expect(getCtx().state.graph?.ws_id).toBe(MULTI_NODE_WS_ID);
      expect(getCtx().state.graph!.nodes.length).toBeGreaterThan(1);
    });
  });
});

// ---------------------------------------------------------------------------
// V7 — openWorkstream on a SINGLE-node workstream navigates to "session"
// ---------------------------------------------------------------------------

describe("V7: openWorkstream — single-node workstream nav rule", () => {
  it("navigates to screen 'session' (not 'graph') for a single-node workstream", async () => {
    const { getCtx } = renderVm();

    // Sanity: confirm this seed id is indeed single-node before asserting nav.
    const board = await api.fetchBoard();
    const card = board.all.find((c) => c.id === SINGLE_NODE_WS_ID);
    expect(card).toBeTruthy();
    expect(card!.is_single_node).toBe(true);

    await act(async () => {
      await getCtx().openWorkstream(SINGLE_NODE_WS_ID);
    });

    await waitFor(() => {
      expect(getCtx().state.screen).toBe("session");
      expect(getCtx().state.screen).not.toBe("graph");
      // The single member session (s01) must be selected/opened.
      expect(getCtx().state.selectedId).toBe("s01");
    });
  });
});

// ---------------------------------------------------------------------------
// V8 — setActiveProfile helper
// ---------------------------------------------------------------------------

describe("V8: setActiveProfile helper", () => {
  it("sets state.activeProfile and refreshes state.board.all to reflect the filter", async () => {
    const spy = vi.spyOn(api, "setActiveProfile");
    const { getCtx } = renderVm();

    // Build two workstreams under two different profiles.
    await api.createProfile("prof-v8-a", "V8 Alpha");
    await api.createProfile("prof-v8-b", "V8 Beta");
    await api.createWorkstream("ws-v8-a", "V8 WS Alpha", "goal a", "prof-v8-a");
    await api.createWorkstream("ws-v8-b", "V8 WS Beta", "goal b", "prof-v8-b");

    act(() => {
      getCtx().openBoard();
    });
    await waitFor(() => expect(getCtx().state.board).not.toBeNull());

    await act(async () => {
      await getCtx().setActiveProfile("prof-v8-a");
    });

    expect(spy).toHaveBeenCalledWith("prof-v8-a");

    await waitFor(() => {
      expect(getCtx().state.activeProfile).toBe("prof-v8-a");
      const all = getCtx().state.board?.all ?? [];
      expect(all.find((c) => c.id === "ws-v8-a")).toBeTruthy();
      expect(all.find((c) => c.id === "ws-v8-b")).toBeUndefined();
    });
  });
});

// ---------------------------------------------------------------------------
// V9 — deleteWorkstream helper
// ---------------------------------------------------------------------------

describe("V9: deleteWorkstream helper", () => {
  it("calls api.deleteWorkstream and the board no longer lists it", async () => {
    const spy = vi.spyOn(api, "deleteWorkstream");
    const { getCtx } = renderVm();

    await api.createWorkstream("ws-v9-doomed", "V9 doomed", "will be deleted");

    act(() => {
      getCtx().openBoard();
    });
    await waitFor(() => {
      expect(
        getCtx().state.board?.all.find((c) => c.id === "ws-v9-doomed")
      ).toBeTruthy();
    });

    await act(async () => {
      await getCtx().deleteWorkstream("ws-v9-doomed");
    });

    expect(spy).toHaveBeenCalledWith("ws-v9-doomed");

    await waitFor(() => {
      expect(
        getCtx().state.board?.all.find((c) => c.id === "ws-v9-doomed")
      ).toBeUndefined();
    });
  });
});

// ---------------------------------------------------------------------------
// V10 — assignSession / createProfile / deleteProfile helpers
// ---------------------------------------------------------------------------

describe("V10: assignSession / createProfile / deleteProfile helpers", () => {
  it("assignSession calls api.assignSession and refreshes board (session_count reflects the assignment)", async () => {
    const spy = vi.spyOn(api, "assignSession");
    const { getCtx } = renderVm();

    act(() => {
      getCtx().openBoard();
    });
    await waitFor(() => expect(getCtx().state.board).not.toBeNull());

    const before = getCtx().state.board?.all.find(
      (c) => c.id === SINGLE_NODE_WS_ID
    )?.session_count;
    expect(before).toBe(1);

    await act(async () => {
      await getCtx().assignSession("sess-v10-new", SINGLE_NODE_WS_ID, false);
    });

    expect(spy).toHaveBeenCalledWith("sess-v10-new", SINGLE_NODE_WS_ID, false);

    await waitFor(() => {
      const after = getCtx().state.board?.all.find(
        (c) => c.id === SINGLE_NODE_WS_ID
      )?.session_count;
      expect(after).toBe(2);
    });
  });

  it("createProfile calls api.createProfile and refreshes board (usable immediately for filtering)", async () => {
    const spy = vi.spyOn(api, "createProfile");
    const { getCtx } = renderVm();

    act(() => {
      getCtx().openBoard();
    });
    await waitFor(() => expect(getCtx().state.board).not.toBeNull());

    await act(async () => {
      await getCtx().createProfile("prof-v10-new", "V10 Profile");
    });

    expect(spy).toHaveBeenCalledWith("prof-v10-new", "V10 Profile");

    // Prove it landed in the mock backend (board refresh happened / helper
    // isn't a dead stub) by attaching a workstream to it and filtering.
    await api.createWorkstream("ws-v10-tagged", "tagged", "goal", "prof-v10-new");

    await act(async () => {
      await getCtx().setActiveProfile("prof-v10-new");
    });

    await waitFor(() => {
      expect(
        getCtx().state.board?.all.find((c) => c.id === "ws-v10-tagged")
      ).toBeTruthy();
    });
  });

  it("deleteProfile calls api.deleteProfile and refreshes board (profile no longer filters anything)", async () => {
    const spy = vi.spyOn(api, "deleteProfile");
    const { getCtx } = renderVm();

    await api.createProfile("prof-v10-doomed", "Doomed profile");
    await api.createWorkstream(
      "ws-v10-doomed-member",
      "member",
      "goal",
      "prof-v10-doomed"
    );

    act(() => {
      getCtx().openBoard();
    });
    await waitFor(() => expect(getCtx().state.board).not.toBeNull());

    await act(async () => {
      await getCtx().setActiveProfile("prof-v10-doomed");
    });
    await waitFor(() => {
      expect(getCtx().state.activeProfile).toBe("prof-v10-doomed");
    });

    await act(async () => {
      await getCtx().deleteProfile("prof-v10-doomed");
    });

    expect(spy).toHaveBeenCalledWith("prof-v10-doomed");

    // The mock clears activeProfile when the deleted profile was active, and
    // a board refresh must pick that up — proving deleteProfile isn't a dead
    // stub that calls api but never refreshes.
    await waitFor(() => {
      expect(mockWorkstreams.state().activeProfile).toBeNull();
      const all = getCtx().state.board?.all ?? [];
      expect(all.find((c) => c.id === "ws-v10-doomed-member")).toBeTruthy();
    });
  });
});

// ---------------------------------------------------------------------------
// Extra: openBoard / openNode helper wiring (used across V1/V6/V7 but also
// verified directly here since the spec calls them out explicitly).
// ---------------------------------------------------------------------------

describe("openBoard / openNode helpers", () => {
  it("openBoard() dispatches OPEN_BOARD and fetches+sets the board", async () => {
    const spy = vi.spyOn(api, "fetchBoard");
    const { getCtx } = renderVm({ screen: "graph" as VmState["screen"] });

    act(() => {
      getCtx().openBoard();
    });

    await waitFor(() => {
      expect(getCtx().state.screen).toBe("board");
      expect(getCtx().state.board).not.toBeNull();
    });
    expect(spy).toHaveBeenCalled();
  });

  it("openNode(sessionId) selects the session and navigates to screen 'session'", async () => {
    const { getCtx } = renderVm();

    act(() => {
      getCtx().openNode("s06");
    });

    await waitFor(() => {
      expect(getCtx().state.selectedId).toBe("s06");
      expect(getCtx().state.screen).toBe("session");
    });
  });
});
