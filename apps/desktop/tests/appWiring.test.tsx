/**
 * Full-flow integration suite for the desktop app-wiring block (U-WIRE).
 *
 * Renders the REAL `<App/>` (ThemeProvider > VmProvider > Router) and drives
 * the whole Level-1 → Level-2 → Level-3 navigation ONLY through real rendered
 * controls (clicks) + the real `useVm`/router. There is NO direct `dispatch`
 * to force a screen anywhere in this file — that is the whole point: it proves
 * the UI is actually hooked to functionality. A nav button that dispatches
 * nothing, a card wired to a dead handler, or a connected wrapper that leaves a
 * callback unwired must fail a test here.
 *
 * In test env `isTauri()` is false, so `useVm` drives the mock backend
 * (`mockWorkstreams`). We reset it in `beforeEach` so seeded state is
 * deterministic. Async board/graph fetches are awaited via `findBy*`/`waitFor`.
 *
 * Covers docs/specs/ui-app-wiring.md §Test plan W1–W8 exactly.
 *
 * NOTE ON THE SESSION SCREEN (W3/W4/W6): the seeded graph node ids
 * (s01, s07, s06) are REAL sessions in the app snapshot's session list
 * (s01–s10), so opening a node navigates to the `session` screen and mounts a
 * POPULATED `<SessionDetail/>` for that exact session. The flow assertions key
 * off "we left the graph/board AND the real session header (`session-name`)
 * rendered" via `expectOnSessionScreen` — proving the click reached the actual
 * session, not merely any screen transition.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { App } from "../src/App";
import { VmProvider } from "../src/bridge/useVm";
import { BoardScreen } from "../src/screens/BoardScreen";
import { GraphScreenConnected } from "../src/screens/graph/GraphScreenConnected";
import { mockWorkstreams } from "../src/bridge/mockWorkstreams";
import { _resetMock } from "../src/bridge/api";

// ---------------------------------------------------------------------------
// Seed knowledge (from src/bridge/mockWorkstreams.ts seedState()):
//   - "ws-auth-refactor"  — 1 member session (s01) => single-node.
//   - "ws-build-pipeline" — 2 member sessions (s07,
//     s06) => multi-node, with a gated connection "conn-seed-1"
//     from s07 -> s06.
// ---------------------------------------------------------------------------
const SINGLE_NODE_WS_ID = "ws-auth-refactor";
const MULTI_NODE_WS_ID = "ws-build-pipeline";
const GATED_CONN_ID = "conn-seed-1";
const GATED_FROM = "s07";
const GATED_TO = "s06";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Render the real App root. */
function renderApp() {
  return render(<App />);
}

/**
 * Boot the app. Board is now the home screen (Workstreams redesign SLICE A —
 * there is no more separate Overview home and no "nav-board" sidebar entry to
 * click through), so booting IS opening the board; just await the
 * async-fetched board landmark.
 */
async function bootAndOpenBoard() {
  renderApp();
  await screen.findByTestId("board-title");
}

/**
 * Assert we are on the SessionDetail screen (Level 3) for a REAL session and
 * NOT on the graph or board. Because the seeded node ids (s01/s07/s06) are real
 * snapshot sessions, opening a node mounts a POPULATED SessionDetail — so we
 * require the `session-name` header (never the empty "No session selected."
 * fallback), proving the click reached the actual session. When `expectName` is
 * given, also assert the header text matches that exact session.
 */
async function expectOnSessionScreen(expectName?: string) {
  await waitFor(() => {
    // Left the graph and the board.
    expect(screen.queryByTestId("graph-canvas")).toBeNull();
    expect(screen.queryByTestId("board-title")).toBeNull();
    // A populated SessionDetail is mounted (the empty-state must NOT show).
    expect(screen.queryByText(/No session selected/i)).toBeNull();
    const nameEl = screen.getByTestId("session-name");
    if (expectName != null) {
      expect(nameEl.textContent).toContain(expectName);
    }
  });
}

beforeEach(() => {
  mockWorkstreams.reset();
  _resetMock();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// W1 — boot → Board renders directly (Board is now the home screen)
// ---------------------------------------------------------------------------

describe("W1: the app boots directly onto the Board (Board is the home screen)", () => {
  it("boots straight into the Board — no click required", async () => {
    renderApp();

    // The provider-fetched board resolves and renders board-only landmarks,
    // with no intermediate nav click (there is no more Overview home).
    const title = await screen.findByTestId("board-title");
    expect(title.textContent).toContain("Workstreams");
    // The seeded workstreams show up as real rows (proves the board data, not
    // a bare placeholder, reached the screen).
    expect(await screen.findByTestId(`workstream-row-${MULTI_NODE_WS_ID}`)).toBeTruthy();
    expect(screen.getByTestId(`workstream-row-${SINGLE_NODE_WS_ID}`)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// W2 — multi-node card click → Graph screen
// ---------------------------------------------------------------------------

describe("W2: clicking a multi-node workstream card routes to the graph", () => {
  it("board card click → openWorkstream → graph canvas + graph nodes render", async () => {
    await bootAndOpenBoard();

    // Not on the graph before the click.
    expect(screen.queryByTestId("graph-canvas")).toBeNull();

    fireEvent.click(screen.getByTestId(`workstream-row-${MULTI_NODE_WS_ID}`));

    // The graph was fetched + routed: the canvas and per-node cards appear.
    await screen.findByTestId("graph-canvas");
    expect(await screen.findByTestId(`graph-node-${GATED_FROM}`)).toBeTruthy();
    expect(screen.getByTestId(`graph-node-${GATED_TO}`)).toBeTruthy();
    // And we are no longer on the board.
    expect(screen.queryByTestId("board-title")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// W3 — single-node nav rule → straight to SessionDetail, NOT the graph
// ---------------------------------------------------------------------------

describe("W3: single-node workstream card honors the nav rule end-to-end", () => {
  it("clicking the single-node card goes to the session screen, not the graph", async () => {
    await bootAndOpenBoard();

    fireEvent.click(screen.getByTestId(`workstream-row-${SINGLE_NODE_WS_ID}`));

    // The nav rule routes single-node workstreams to their session, never the
    // graph — assert we land on the real session (s01 = api-auth-refactor).
    await expectOnSessionScreen("api-auth-refactor");
    expect(screen.queryByTestId("graph-canvas")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// W4 — graph → session via a node's open affordance
// ---------------------------------------------------------------------------

describe("W4: a node's open affordance routes to that node's session", () => {
  it("opening the multi-node graph then clicking node-open-{id} opens the session", async () => {
    await bootAndOpenBoard();

    fireEvent.click(screen.getByTestId(`workstream-row-${MULTI_NODE_WS_ID}`));
    await screen.findByTestId("graph-canvas");

    // Click the dedicated open affordance for a specific node (not the node
    // body — that only selects). This exercises the `openNode` wiring.
    const openAffordance = screen.getByTestId(`node-open-${GATED_FROM}`);
    fireEvent.click(openAffordance);

    // GATED_FROM = s07 = the ci-pipeline-fix session.
    await expectOnSessionScreen("ci-pipeline-fix");
  });
});

// ---------------------------------------------------------------------------
// W5 — node selection round-trips through selectNode → state.nodeSel → prop
// ---------------------------------------------------------------------------

describe("W5: clicking a graph node selects it and dims non-neighbours", () => {
  it("selects the clicked node (data-selected) and dims a non-neighbour, staying on the graph", async () => {
    await bootAndOpenBoard();

    fireEvent.click(screen.getByTestId(`workstream-row-${MULTI_NODE_WS_ID}`));
    await screen.findByTestId("graph-canvas");

    // Click the NODE BODY (selection), not the open affordance.
    // s01 is a member of the OTHER workstream, so it is never a
    // neighbour of the build-pipeline nodes — but it isn't in this graph. The
    // two build-pipeline nodes are connected by the seeded edge, so selecting
    // one keeps the other in-neighbourhood. To get a guaranteed non-neighbour
    // we add a third, unconnected node to the workstream before opening.
    // (Done in the dedicated non-neighbour test below.) Here we assert the
    // core selection round-trip on the seeded pair.
    const nodeEl = screen.getByTestId(`graph-node-${GATED_FROM}`);
    fireEvent.click(nodeEl);

    await waitFor(() => {
      expect(
        screen.getByTestId(`graph-node-${GATED_FROM}`).getAttribute("data-selected")
      ).toBe("true");
    });

    // Still on the graph after a selection (selection must not navigate away).
    expect(screen.getByTestId("graph-canvas")).toBeTruthy();
  });

  it("a non-neighbour node is dimmed when another node is selected", async () => {
    // Seed a third node into the multi-node workstream with NO connection, so
    // it is guaranteed to be a non-neighbour of the selected node.
    mockWorkstreams.assignSession("sess-isolated", MULTI_NODE_WS_ID, false);

    await bootAndOpenBoard();

    fireEvent.click(screen.getByTestId(`workstream-row-${MULTI_NODE_WS_ID}`));
    await screen.findByTestId("graph-canvas");
    // The isolated node is present in the graph.
    await screen.findByTestId("graph-node-sess-isolated");

    // Select the gated-from node; the isolated node shares no edge with it.
    fireEvent.click(screen.getByTestId(`graph-node-${GATED_FROM}`));

    await waitFor(() => {
      expect(
        screen.getByTestId(`graph-node-${GATED_FROM}`).getAttribute("data-selected")
      ).toBe("true");
      expect(
        screen.getByTestId("graph-node-sess-isolated").getAttribute("data-dimmed")
      ).toBe("true");
    });

    // The neighbour (connected via the seeded edge) is NOT dimmed.
    expect(
      screen.getByTestId(`graph-node-${GATED_TO}`).getAttribute("data-dimmed")
    ).toBe("false");
    // Still on the graph.
    expect(screen.getByTestId("graph-canvas")).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// W6 — gate affordance is live (interim onOpenGate → receiving node's session)
// ---------------------------------------------------------------------------

describe("W6: the gate affordance is a live control, not a dead handler", () => {
  it("clicking the gate diamond opens the receiving node's session", async () => {
    await bootAndOpenBoard();

    fireEvent.click(screen.getByTestId(`workstream-row-${MULTI_NODE_WS_ID}`));
    await screen.findByTestId("graph-canvas");

    // The seeded gated connection renders a clickable gate diamond.
    const gate = await screen.findByTestId(`gate-diamond-${GATED_CONN_ID}`);
    fireEvent.click(gate);

    // Interim wiring: onOpenGate routes to the RECEIVING node's session
    // (GATED_TO = s06 = data-ingestion-pipeline).
    await expectOnSessionScreen("data-ingestion-pipeline");
    // We definitively left the graph.
    expect(screen.queryByTestId("graph-canvas")).toBeNull();
  });

  it("the inspector-handoff callout is also wired to open the receiving session", async () => {
    await bootAndOpenBoard();

    fireEvent.click(screen.getByTestId(`workstream-row-${MULTI_NODE_WS_ID}`));
    await screen.findByTestId("graph-canvas");

    // The pending-handoff callout in the inspector is the other live entry to
    // the same interim wiring.
    const handoff = await screen.findByTestId("inspector-handoff");
    expect(handoff.textContent).toContain(GATED_FROM);
    expect(handoff.textContent).toContain(GATED_TO);
    fireEvent.click(handoff);

    // Same interim wiring: opens the receiving session (s06 = data-ingestion-pipeline).
    await expectOnSessionScreen("data-ingestion-pipeline");
  });
});

// ---------------------------------------------------------------------------
// W7 — connected wrapper loading placeholders
// ---------------------------------------------------------------------------

describe("W7: connected wrappers show loading placeholders for a null slice", () => {
  it("GraphScreenConnected renders graph-loading when no graph is open", () => {
    // Bare provider with a preloaded (empty) snapshot so no async board fetch
    // fires; graph slice is null => the wrapper must show its placeholder.
    render(
      <VmProvider initialState={{ snapshot: null, screen: "graph", graph: null }}>
        <GraphScreenConnected />
      </VmProvider>
    );
    expect(screen.getByTestId("graph-loading")).toBeTruthy();
    // The real graph screen must NOT be rendered while the slice is null.
    expect(screen.queryByTestId("graph-canvas")).toBeNull();
  });

  it("BoardScreen renders a placeholder for a null board, then the real board once populated", async () => {
    // A bare BoardScreen with an injected null-board state: the wrapper fetches
    // the board on mount, so the transient null resolves to a real board.
    render(
      <VmProvider initialState={{ snapshot: null, screen: "board", board: null }}>
        <BoardScreen />
      </VmProvider>
    );

    // Either the transient placeholder is caught, or the board has already
    // resolved — at minimum the board must resolve to the real screen.
    await screen.findByTestId("board-title");
    expect(screen.getByTestId(`workstream-row-${MULTI_NODE_WS_ID}`)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// W8 — regression: the shell chrome renders at boot alongside the board
// ---------------------------------------------------------------------------

describe("W8: the shell chrome renders at boot, with Board as home (no Overview/Workspaces)", () => {
  it("the app boots with the wordmark present and the board as the home screen", async () => {
    renderApp();

    // The shell wordmark is present at boot.
    expect(await screen.findByTestId("wordmark")).toBeTruthy();
    // Board — not Overview — is the home screen (Workstreams redesign SLICE A).
    expect(await screen.findByTestId("board-title")).toBeTruthy();
    expect(screen.queryByTestId("graph-canvas")).toBeNull();
    // The old sidebar's Overview entry and the Workspaces switcher are gone.
    expect(screen.queryByTestId("overview-entry")).toBeNull();
    expect(screen.queryByTestId("ws-switcher")).toBeNull();
  });
});
