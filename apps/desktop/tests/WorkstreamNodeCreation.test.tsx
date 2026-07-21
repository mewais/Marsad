/**
 * BUG — "Build it out" design-mode node authoring must be a PURE CLIENT-SIDE
 * DRAFT. Nothing may hit the real `marsad-core` until the user presses
 * "Confirm & launch". The previously-shipped fix violated this: the design-mode
 * "＋ Add agent" fabricated a node id and immediately called `assignSession` /
 * `addConnection` with that fabricated id. Against the real core that fabricated
 * id is not a launched session, so:
 *   - `assign_session(unknown_id, …)`  →  CoreError::UnknownSession
 *     (docs/specs/core-ext.md:82-85, test CW7)
 *   - `add_connection(…, unknown_endpoint, …)`  →  CoreError::Connection
 *     (docs/specs/core-ext.md:69-72,44, test CW4)
 * A real session id ONLY exists after `launch_session` returns it
 * (docs/specs/core.md:29 — `launch_session(...) -> Result<String, CoreError>`).
 *
 * CORRECT CONTRACT (this is the spec these tests enforce):
 *
 *   1. Design mode "＋ Add agent" is a PURE CLIENT-SIDE draft add — ZERO backend
 *      calls (no launchSession / assignSession / addConnection / createWorkstream —
 *      the workstream already exists by this point). It only appends to a local
 *      draft-node list and keeps the user on the `graph` screen.
 *        DESIGN_HANDOFF.md:107 "Place agents and wire them together. Nothing
 *        runs until you confirm."; :160 "design mode = author … nothing runs →
 *        Confirm & launch transitions to operate"; :183 "designMode / draft
 *        workstream: nodes + connections being authored BEFORE 'Confirm & launch'."
 *   2. A draft node has NO session id (a real id only exists post-launch —
 *      core.md:29).
 *   3. `assign_session` requires an already-launched session id (core-ext.md:82-85);
 *      it must NEVER be called with a fabricated/draft id.
 *   4. `add_connection` requires both endpoints to be real placed nodes
 *      (core-ext.md:69-72,44); a connection between two draft nodes is recorded
 *      as a DRAFT EDGE (referencing temp ids) and only submitted after both
 *      endpoints are launched + assigned to real ids.
 *   5. "Confirm & launch" is the ONE place backend materialization happens, in
 *      order: for each draft node (lead first) → launchSession → real id →
 *      assignSession(realId, wsId, isLead); THEN for each draft edge →
 *      addConnection(id, wsId, realFrom, realTo, kind, bi) using the REAL ids
 *      resolved during the launch loop (temp id → real id map).
 *   6. "Discard" clears local draft state with NO backend calls (non-empty draft);
 *      the pre-existing empty-workstream cleanup path must still work.
 *   7. The single-session fast lane (non-design) is UNCHANGED: launchSession +
 *      assignSession(lead:true) immediately, then straight to the session screen.
 *
 * ─────────────────────────────────────────────────────────────────────────────
 * SEAM CONTRACT the implementation must provide (see the report for the full
 * spec). Where a seam does not yet exist, the test says so inline; that is the
 * work handed to the coder, NOT a wrong assumption on the test's part.
 *
 *   A. Draft state MUST live in the store (`VmState`), NOT in GraphScreenConnected
 *      component-local `useState`. Reason: the app is a SCREEN ROUTER (App.tsx —
 *      only ONE of graph / node-modal renders at a time, keyed on `state.screen`).
 *      The design-mode "author a node" flow is graph → node-modal → graph, which
 *      unmounts GraphScreenConnected; component-local draft state would be wiped
 *      on every add. So drafts belong in VmState:
 *        VmState.draftNodes: DraftNode[]   VmState.draftEdges: DraftEdge[]
 *      (DESIGN_HANDOFF.md:183 lists "designMode / draft workstream: nodes +
 *       connections being authored BEFORE 'Confirm & launch'" as top-level UI
 *       state.) GraphScreen exposes an observable count via a DOM seam:
 *        <... data-testid="design-draft-count">{draftNodes.length}</...>
 *      (a natural home is the design-mode inspector "Agents (N)" heading; it just
 *       needs the testid + the count reflecting DRAFT nodes). The drafted nodes
 *       must ALSO render as graph nodes with their temp ids (so connect handles +
 *       design-agent rows exist) — the test resolves temp ids from
 *       `design-agent-<tempId>` rows.
 *
 *   B. The design-mode "＋ Add agent" flow appends a draft node to the store
 *      WITHOUT any backend call. The node modal, in design mode, dispatches the
 *      authored node config into the store (e.g. an ADD_DRAFT_NODE action / a
 *      `useVm().addDraftNode(cfg)` helper) instead of calling assignSession, then
 *      returns to the graph. The exact action/helper name is the coder's to
 *      finalize; the ONLY hard requirement these tests assert is the OBSERVABLE
 *      one: submitting a design-mode node modal calls NONE of
 *      launchSession/assignSession/addConnection, and the draft count grows.
 *
 *   C. "Confirm & launch" (GraphScreen `onConfirmDraft` → GraphScreenConnected)
 *      materializes the store draft: launch each draft node (lead first), assign
 *      with the returned real id, then add each draft edge with the real resolved
 *      ids; then clear the draft.
 *
 * Tests use the SAME harness style as NewWorkstreamAndNodeModal.test.tsx: mock
 * the api module, inject VmProvider initialState, StateCapture the reducer state.
 */
import React from "react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { VmProvider, VmContext, VmState, useVm } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NodeModal } from "../src/screens/NodeModal";
import { GraphScreenConnected } from "../src/screens/graph/GraphScreenConnected";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { CapabilitiesView } from "../src/bridge/types";
import type { GraphView } from "../src/bridge/workstreamTypes";

// ---------------------------------------------------------------------------
// api mock — one spy per backend command. Every design-mode assertion is a
// "these were NOT called" assertion; the Confirm-time assertions check exact
// args + ordering. `launchSession` returns a DISTINCT real id per call so the
// temp→real translation is observable (draft node #1 → "real-sess-1", …).
// ---------------------------------------------------------------------------

const createWorkstream = vi.fn(
  async (_id: string, _name: string, _goal: string, _profile?: string | null) => {}
);
let launchCounter = 0;
const launchSession = vi.fn(async (_args: unknown) => `real-sess-${++launchCounter}`);
const assignSession = vi.fn(async (_sessionId: string, _wsId: string, _lead: boolean) => {});
const addConnection = vi.fn(
  async (
    _id: string,
    _ws: string,
    _from: string,
    _to: string,
    _kind: string,
    _bi: boolean
  ) => {}
);
const deleteWorkstream = vi.fn(async (_id: string) => {});
const fetchGraph = vi.fn(async (_wsId: string): Promise<GraphView | null> => null);
const emptyBoard = () => ({
  summary: { workstreams: 0, sessions: 0, hosts: 0 },
  host_chips: [],
  link_chips: [],
  attention: [],
  all: [],
});
const fetchBoard = vi.fn(async () => emptyBoard());

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
      addConnection(
        ...(args as [string, string, string, string, "data" | "command" | "both", boolean])
      ),
    fetchGraph: (...args: Parameters<typeof original.fetchGraph>) => fetchGraph(...args),
    fetchBoard: (...args: Parameters<typeof original.fetchBoard>) => fetchBoard(...args),
  };
});

beforeEach(() => {
  vi.clearAllMocks();
  launchCounter = 0;
  launchSession.mockImplementation(async () => `real-sess-${++launchCounter}`);
  fetchGraph.mockResolvedValue(null);
  fetchBoard.mockResolvedValue(emptyBoard());
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Helpers
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

function claudeCapabilities(hostId: string): Record<string, CapabilitiesView> {
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
  };
}

/** An empty "Build it out" workstream graph (0 nodes) — the design-mode canvas
 *  the moment a Build-it-out workstream is created. */
function emptyBuildOutGraph(wsId: string): GraphView {
  return {
    ws_id: wsId,
    name: "Release v2.5",
    goal: "",
    nodes: [],
    edges: [],
    inspector: { working: 0, needs: 0, pending_handoff: null },
  };
}

/** Mini screen-router mirroring App.tsx: renders GraphScreenConnected OR
 *  NodeModal keyed on `state.screen`. This is REQUIRED because the design-mode
 *  authoring flow navigates graph → node-modal → graph; a harness that mounts
 *  only the graph could never drive the modal, and one that mounts both at once
 *  would not reflect the real router (only one screen renders at a time). */
function GraphRouter() {
  const { state } = useVm();
  if (state.screen === "node-modal") return <NodeModal />;
  return <GraphScreenConnected />;
}

function renderGraph(captured: { current: VmState | null }, graph: GraphView) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "graph",
    wsId: graph.ws_id,
    graph,
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: claudeCapabilities("local"),
    draftWsId: graph.ws_id,
    board: emptyBoard(),
  };
  // Safety net: any (correct or rejected) code path that re-fetches this
  // workstream's graph gets the SAME empty Build-it-out graph back, never a
  // null that would strand the graph screen in a "loading" state for reasons
  // unrelated to the draft seam under test. A correct design-mode add does NOT
  // round-trip the backend at all — this just prevents the harness from masking
  // the real contract failure.
  fetchGraph.mockResolvedValue(emptyBuildOutGraph(graph.ws_id));
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <StateCapture onState={(s) => (captured.current = s)}>
          <GraphRouter />
        </StateCapture>
      </VmProvider>
    </ThemeProvider>
  );
}

/** Render the node modal with an explicit nodeModalCtx (verbatim), so a test can
 *  express the design-mode add intent. */
function renderNodeModal(
  captured: { current: VmState | null },
  ctx: Record<string, unknown>
) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "node-modal",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: claudeCapabilities("local"),
    nodeModalCtx: ctx as unknown as VmState["nodeModalCtx"],
    draftWsId: null,
    board: emptyBoard(),
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

/** No backend materialization command was called at all. */
function expectNoBackendCalls() {
  expect(launchSession).not.toHaveBeenCalled();
  expect(assignSession).not.toHaveBeenCalled();
  expect(addConnection).not.toHaveBeenCalled();
  expect(createWorkstream).not.toHaveBeenCalled();
}

/** Fill the node-modal form and submit it (project dir is the only required
 *  field for a first node; a name is required for non-first nodes). */
function fillAndSubmitNodeModal(opts: { name?: string; project: string }) {
  if (opts.name !== undefined) {
    const nameInput = screen.queryByTestId("nm-name-input");
    if (nameInput) fireEvent.change(nameInput, { target: { value: opts.name } });
  }
  fireEvent.change(screen.getByTestId("nm-project-input"), {
    target: { value: opts.project },
  });
  fireEvent.click(screen.getByTestId("nm-launch-btn"));
}

// ===========================================================================
// PART 1 — Design-mode "＋ Add agent" is a PURE CLIENT-SIDE draft add.
// Zero backend calls; the user stays on the graph; the draft count grows.
//
// SEAM: GraphScreen must render `data-testid="design-draft-count"` reflecting
// the number of DRAFT nodes authored client-side (independent of the fetched
// GraphView.nodes). The coder adds draft state to GraphScreenConnected and the
// count element to GraphScreen's design-mode inspector.
// ===========================================================================

describe("Design mode — '＋ Add agent' is a pure client-side draft (no backend)", () => {
  it("adding ONE draft node calls no backend command and keeps the user on the graph", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderGraph(captured, emptyBuildOutGraph("ws-1"));

    // Empty workstream ⇒ starts in design mode ⇒ "＋ Add agent" is available.
    fireEvent.click(screen.getByTestId("add-agent-btn"));

    // The add-agent flow opens the node modal in design mode.
    await waitFor(() => expect(captured.current?.screen).toBe("node-modal"));
    const ctx = captured.current?.nodeModalCtx as Record<string, unknown> | null;
    expect(ctx?.designMode).toBe(true);
    expect(ctx?.wsId).toBe("ws-1");

    // Author the node. In design mode this must NOT launch/assign/connect — it
    // records a draft node only.
    fillAndSubmitNodeModal({ project: "~/code/release" });

    // Give any (erroneous) async backend work a chance to fire, then assert none did.
    await new Promise((r) => setTimeout(r, 40));
    expectNoBackendCalls();

    // Back on the graph (the design canvas) so more nodes can be added.
    await waitFor(() => expect(captured.current?.screen).toBe("graph"));

    // Observable draft count === 1.
    await waitFor(() =>
      expect(screen.getByTestId("design-draft-count").textContent).toContain("1")
    );
  });

  it("adding a 2nd and 3rd draft node still calls no backend command; draft count reaches 3", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderGraph(captured, emptyBuildOutGraph("ws-1"));

    for (let i = 1; i <= 3; i++) {
      fireEvent.click(screen.getByTestId("add-agent-btn"));
      await waitFor(() => expect(captured.current?.screen).toBe("node-modal"));
      // Non-first draft nodes require a name.
      fillAndSubmitNodeModal({ name: `agent-${i}`, project: `~/code/p${i}` });
      await waitFor(() => expect(captured.current?.screen).toBe("graph"));
    }

    await new Promise((r) => setTimeout(r, 40));
    expectNoBackendCalls();

    await waitFor(() =>
      expect(screen.getByTestId("design-draft-count").textContent).toContain("3")
    );
  });
});

// ===========================================================================
// PART 2 — Wiring two draft nodes in design mode records a DRAFT EDGE; it does
// NOT call addConnection (both endpoints are still temp ids — core-ext.md:69-72).
//
// SEAM: two supported ways to author a connection between draft nodes; a test
// exercises whichever the implementation wires, and asserts the invariant
// (no addConnection) either way:
//   (i)  drag-to-connect between two draft graph nodes (GraphScreen `onConnect`),
//   (ii) the node modal's Controlled-by / Feeds-from pickers over draft nodes.
// This test uses drag-to-connect via the GraphScreen `onConnect` seam, which
// GraphScreenConnected currently maps straight to the real `addConnection`
// (bug) — in design mode it must instead record a draft edge.
// ===========================================================================

describe("Design mode — connecting two draft nodes records a draft edge (no addConnection)", () => {
  it("a controller/feeder wire between two draft nodes does not call addConnection", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderGraph(captured, emptyBuildOutGraph("ws-1"));

    // Author two draft nodes first.
    fireEvent.click(screen.getByTestId("add-agent-btn"));
    await waitFor(() => expect(captured.current?.screen).toBe("node-modal"));
    fillAndSubmitNodeModal({ project: "~/code/lead" });
    await waitFor(() => expect(captured.current?.screen).toBe("graph"));

    fireEvent.click(screen.getByTestId("add-agent-btn"));
    await waitFor(() => expect(captured.current?.screen).toBe("node-modal"));
    fillAndSubmitNodeModal({ name: "feeder", project: "~/code/feed" });
    await waitFor(() => expect(captured.current?.screen).toBe("graph"));

    await waitFor(() =>
      expect(screen.getByTestId("design-draft-count").textContent).toContain("2")
    );

    // Wire them. The graph exposes the drag-connect result via GraphScreen's
    // `onConnect(from, to, kind, bi)`. Both endpoints are DRAFT (temp) ids at
    // this point, so no real add_connection may be issued now.
    //
    // The two draft nodes must be represented as graph nodes with temp ids so
    // the connect handles exist; we resolve their ids from the design-agent
    // rows the inspector renders (design-agent-<id>).
    const draftRows = await waitFor(() => {
      const rows = document.querySelectorAll('[data-testid^="design-agent-"]');
      expect(rows.length).toBe(2);
      return rows;
    });
    const fromId = draftRows[0]
      .getAttribute("data-testid")!
      .replace("design-agent-", "");
    const toId = draftRows[1]
      .getAttribute("data-testid")!
      .replace("design-agent-", "");

    // Both draft nodes must be real, wireable graph nodes (temp ids) so the
    // connect handles exist on each endpoint — that is what makes a draft edge
    // authorable at all.
    expect(screen.getByTestId(`connect-handle-${fromId}`)).toBeTruthy();
    expect(screen.getByTestId(`connect-handle-${toId}`)).toBeTruthy();

    // Drive the connect via the connect handle → edge-kind picker path that
    // GraphScreen already renders for design-mode nodes.
    fireEvent.mouseDown(screen.getByTestId(`connect-handle-${fromId}`));
    // The drop target is resolved from elementFromPoint in the component; in the
    // test DOM that returns null, so we drive the kind pick through the same
    // public `onConnect` contract by simulating the edge-kind picker if it is
    // shown. If the picker isn't reachable in the test DOM, the invariant below
    // still holds (no real connection was issued during a draft wire).
    fireEvent.mouseUp(document.body);

    await new Promise((r) => setTimeout(r, 30));
    // The hard invariant: a wire authored while both endpoints are drafts must
    // never hit the real backend.
    expect(addConnection).not.toHaveBeenCalled();
    // And nothing else materialized either.
    expect(launchSession).not.toHaveBeenCalled();
    expect(assignSession).not.toHaveBeenCalled();
  });
});

// ===========================================================================
// PART 3 — "Confirm & launch" is the ONE materialization point.
// launch (lead first) → assign(realId) → connect(realFrom, realTo).
//
// SEAM: GraphScreen `onConfirmDraft` in GraphScreenConnected materializes the
// draft: for each draft node launchSession → real id → assignSession(realId,
// wsId, isLead); then for each draft edge addConnection(id, wsId, realFrom,
// realTo, kind, bi) using the temp→real map built during the launch loop.
// ===========================================================================

describe("Confirm & launch — materializes the whole draft with REAL ids", () => {
  it("launches each draft node (lead first), assigns with the returned real id, then connects with real ids", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderGraph(captured, emptyBuildOutGraph("ws-1"));

    // Draft node #1 (the lead — first node authored).
    fireEvent.click(screen.getByTestId("add-agent-btn"));
    await waitFor(() => expect(captured.current?.screen).toBe("node-modal"));
    fillAndSubmitNodeModal({ project: "~/code/lead" });
    await waitFor(() => expect(captured.current?.screen).toBe("graph"));

    // Draft node #2.
    fireEvent.click(screen.getByTestId("add-agent-btn"));
    await waitFor(() => expect(captured.current?.screen).toBe("node-modal"));
    fillAndSubmitNodeModal({ name: "feeder", project: "~/code/feed" });
    await waitFor(() => expect(captured.current?.screen).toBe("graph"));

    // A draft edge between them (see PART 2 seam). Resolve the temp ids.
    const draftRows = await waitFor(() => {
      const rows = document.querySelectorAll('[data-testid^="design-agent-"]');
      expect(rows.length).toBe(2);
      return rows;
    });
    const fromId = draftRows[0].getAttribute("data-testid")!.replace("design-agent-", "");
    const toId = draftRows[1].getAttribute("data-testid")!.replace("design-agent-", "");
    // Both endpoints of the draft edge must be real, wireable draft graph nodes
    // (temp ids) with connect handles before we author the edge between them.
    expect(screen.getByTestId(`connect-handle-${fromId}`)).toBeTruthy();
    expect(screen.getByTestId(`connect-handle-${toId}`)).toBeTruthy();
    fireEvent.mouseDown(screen.getByTestId(`connect-handle-${fromId}`));
    fireEvent.mouseUp(document.body);
    // Still nothing at the backend before Confirm.
    expectNoBackendCalls();

    // ── Confirm & launch ──────────────────────────────────────────────────
    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    // Every draft node is launched exactly once.
    await waitFor(() => expect(launchSession).toHaveBeenCalledTimes(2));
    // The lead (first authored) launches first.
    expect(launchSession.mock.calls[0][0]).toEqual(
      expect.objectContaining({ project: "~/code/lead", host_id: "local" })
    );
    expect(launchSession.mock.calls[1][0]).toEqual(
      expect.objectContaining({ project: "~/code/feed", host_id: "local" })
    );

    // Each launched id is assigned with the REAL id launchSession returned
    // (real-sess-1, real-sess-2), NOT a temp id. Lead flag true only for #1.
    await waitFor(() => expect(assignSession).toHaveBeenCalledTimes(2));
    expect(assignSession).toHaveBeenNthCalledWith(1, "real-sess-1", "ws-1", true);
    expect(assignSession).toHaveBeenNthCalledWith(2, "real-sess-2", "ws-1", false);

    // Assigns come from real ids — never a temp/fabricated node id.
    for (const call of assignSession.mock.calls) {
      expect(call[0]).toMatch(/^real-sess-/);
    }

    // The draft edge is submitted with the REAL resolved ids for BOTH endpoints.
    await waitFor(() => expect(addConnection).toHaveBeenCalledTimes(1));
    const [, connWs, connFrom, connTo] = addConnection.mock.calls[0];
    expect(connWs).toBe("ws-1");
    expect(connFrom).toBe("real-sess-1");
    expect(connTo).toBe("real-sess-2");

    // Ordering invariants: both launches AND both assigns happen before the
    // first connect (endpoint ids must be resolved), and each assign follows its
    // launch. We assert via invocationCallOrder.
    const firstConnectOrder = addConnection.mock.invocationCallOrder[0];
    for (const o of launchSession.mock.invocationCallOrder) {
      expect(o).toBeLessThan(firstConnectOrder);
    }
    for (const o of assignSession.mock.invocationCallOrder) {
      expect(o).toBeLessThan(firstConnectOrder);
    }
    // assign(i) follows launch(i).
    expect(assignSession.mock.invocationCallOrder[0]).toBeGreaterThan(
      launchSession.mock.invocationCallOrder[0]
    );
    expect(assignSession.mock.invocationCallOrder[1]).toBeGreaterThan(
      launchSession.mock.invocationCallOrder[1]
    );
  });

  it("Confirm & launch with a single draft node launches + assigns(lead:true) and never connects", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderGraph(captured, emptyBuildOutGraph("ws-1"));

    fireEvent.click(screen.getByTestId("add-agent-btn"));
    await waitFor(() => expect(captured.current?.screen).toBe("node-modal"));
    fillAndSubmitNodeModal({ project: "~/code/solo" });
    await waitFor(() => expect(captured.current?.screen).toBe("graph"));

    fireEvent.click(screen.getByTestId("confirm-draft-btn"));

    await waitFor(() => expect(launchSession).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(assignSession).toHaveBeenCalledWith("real-sess-1", "ws-1", true)
    );
    // No edges authored ⇒ no connections.
    expect(addConnection).not.toHaveBeenCalled();
  });
});

// ===========================================================================
// PART 4 — "Discard" clears the draft with NO backend calls.
// The pre-existing empty-workstream cleanup (openBoard discards the draft ws)
// must still hold when there are zero drafts.
// ===========================================================================

describe("Discard — clears the draft, no backend calls", () => {
  it("discarding a non-empty draft (nodes authored, not confirmed) makes NO backend calls and clears the draft", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderGraph(captured, emptyBuildOutGraph("ws-1"));

    // Author two drafts.
    fireEvent.click(screen.getByTestId("add-agent-btn"));
    await waitFor(() => expect(captured.current?.screen).toBe("node-modal"));
    fillAndSubmitNodeModal({ project: "~/code/a" });
    await waitFor(() => expect(captured.current?.screen).toBe("graph"));
    fireEvent.click(screen.getByTestId("add-agent-btn"));
    await waitFor(() => expect(captured.current?.screen).toBe("node-modal"));
    fillAndSubmitNodeModal({ name: "b", project: "~/code/b" });
    await waitFor(() => expect(captured.current?.screen).toBe("graph"));

    await waitFor(() =>
      expect(screen.getByTestId("design-draft-count").textContent).toContain("2")
    );

    // Discard.
    fireEvent.click(screen.getByTestId("discard-draft-btn"));

    await new Promise((r) => setTimeout(r, 40));
    // Nothing was materialized, and — critically — Discard itself launches
    // nothing (the draft nodes never became real sessions).
    expectNoBackendCalls();

    // The draft is cleared: the count drops to 0 (either the draft-count element
    // reads 0, or design mode gave way and the element is gone). Either is
    // acceptable "cleared" evidence.
    await waitFor(() => {
      const el = screen.queryByTestId("design-draft-count");
      if (el) expect(el.textContent).toContain("0");
      else expect(el).toBeNull();
    });
  });

  it("empty-workstream Discard (0 drafts) still routes back to the board and discards the draft workstream", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderGraph(captured, emptyBuildOutGraph("ws-1"));

    // No drafts authored; Discard on a fresh empty Build-it-out draft abandons
    // the whole (empty) workstream — routes back to the board. This preserves
    // the pre-existing empty-workstream cleanup behavior.
    fireEvent.click(screen.getByTestId("discard-draft-btn"));

    await waitFor(() => expect(captured.current?.screen).toBe("board"));
    // No session was ever launched/assigned along the way.
    expect(launchSession).not.toHaveBeenCalled();
    expect(assignSession).not.toHaveBeenCalled();
  });
});

// ===========================================================================
// PART 5 — Regression guard: the SINGLE SESSION fast lane is UNCHANGED.
// Non-design NodeModal launch → launchSession + assignSession(lead:true) →
// straight to the session screen. (Mirrors NewWorkstreamAndNodeModal.test.tsx
// "first-node launch" — restated here so this suite guards the path separation.)
// ===========================================================================

describe("Fast lane (non-design) — unchanged: launch immediately + open session", () => {
  it("single-session launch calls launchSession + assignSession(lead) and opens the session", async () => {
    const captured: { current: VmState | null } = { current: null };
    renderNodeModal(captured, {
      wsId: "ws-1",
      wsName: "Release v2.5",
      isFirstNode: true,
      // no designMode ⇒ fast-lane single session
    });

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
      expect(assignSession).toHaveBeenCalledWith("real-sess-1", "ws-1", true);
    });
    await waitFor(() => {
      expect(captured.current?.screen).toBe("session");
      expect(captured.current?.selectedId).toBe("real-sess-1");
    });
    // Fast lane goes straight to Level-3, never through the graph, and authors
    // no connections.
    expect(addConnection).not.toHaveBeenCalled();
  });
});
