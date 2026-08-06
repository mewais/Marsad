/**
 * U2 — NodeModal resume picker consumes ONLY dynamic capabilities.
 *
 * Contract pinned (docs/specs/dynamic-capabilities.md §5, "Resume picker"):
 *   "reads capabilities[...].sessions filtered by chosen project (agy
 *   `project: None` entries render under 'unknown project'); fires
 *   `refreshCapabilities({host_id, driver, project})` on open and renders
 *   the cached list instantly with its `fetched_at_ms` ('as of …') —
 *   stale-then-fresh, never spinner-blocked."
 *
 * NodeModal's resume list comes ONLY from capabilities[(driver,host)].sessions
 * — never the kill-listed `listSessions()`. This suite proves the source and
 * the project filtering / unknown-project grouping / staleness rendering.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NodeModal } from "../src/screens/NodeModal";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot } from "../src/bridge/types";
import { capsMap, makeCapabilities, type CapabilitiesViewFixture } from "./fixtures/capabilities";

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    answerGate: vi.fn().mockResolvedValue(undefined),
    launchSession: vi.fn().mockResolvedValue("mock-session-new"),
    listSessions: vi.fn().mockResolvedValue([
      { id: "sess-aaaa-1111", snippet: "Add a login endpoint to the API", modified: 1719000000000 },
      { id: "sess-bbbb-2222", snippet: "Refactor the auth middleware", modified: 1718990000000 },
    ]),
    // Does not exist on api.ts today (§3.6) — added here so the spy is
    // observable the moment U2 wires NodeModal onto it.
    refreshCapabilities: vi.fn().mockResolvedValue(undefined),
  };
});

import * as api from "../src/bridge/api";

beforeEach(() => {
  vi.clearAllMocks();
});
afterEach(() => {
  vi.restoreAllMocks();
});

function renderNodeModal(caps: CapabilitiesViewFixture[]) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT };
  const preloaded = {
    snapshot,
    screen: "node-modal",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: capsMap(caps),
    nodeModalCtx: { wsId: "ws-1", wsName: "Test WS", isFirstNode: true },
    draftWsId: null,
  } as unknown as Partial<VmState>;
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NodeModal />
      </VmProvider>
    </ThemeProvider>
  );
}

function fillProjectAndResume(project: string) {
  fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: project } });
  fireEvent.click(screen.getByTestId("nm-resume-toggle"));
}

describe("NodeModal resume picker — capabilities-only source (U2 contract)", () => {
  it("lists sessions from capabilities[('claude','local')].sessions, and never calls the kill-listed listSessions()", () => {
    renderNodeModal([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        session_scope: { Projects: ["~/projects/api"] },
        sessions: [
          { id: "cap-sess-1", project: "~/projects/api", snippet: "Add caching layer", modified_ms: 1783100000000 },
        ],
        fetched_at_ms: 1783100000000,
      }),
    ]);

    fillProjectAndResume("~/projects/api");

    expect(screen.getByTestId("nm-resume-session-cap-sess-1").textContent).toContain("Add caching layer");
    // The old hardcoded mock entries must not leak in.
    expect(screen.queryByText(/Add a login endpoint to the API/)).toBeNull();
    expect(screen.queryByText(/Refactor the auth middleware/)).toBeNull();
    expect(api.listSessions as ReturnType<typeof vi.fn>).not.toHaveBeenCalled();
  });

  it("filters sessions to the chosen project — a session from a different project does not leak in", () => {
    renderNodeModal([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        session_scope: { Projects: ["~/projects/api", "~/projects/other"] },
        sessions: [
          { id: "sess-api", project: "~/projects/api", snippet: "api work", modified_ms: 100 },
          { id: "sess-other", project: "~/projects/other", snippet: "other work", modified_ms: 200 },
        ],
      }),
    ]);

    fillProjectAndResume("~/projects/api");

    expect(screen.getByTestId("nm-resume-session-sess-api")).toBeTruthy();
    expect(screen.queryByTestId("nm-resume-session-sess-other")).toBeNull();
  });

  it("agy-style sessions with project: null render under an 'unknown project' group, per SessionScope semantics", () => {
    // NodeModal keeps the selected driver (default "claude") and does a strict
    // (driver, host) capabilities lookup, so the fixture is keyed to the
    // selected driver on the switched host. The behavioral intent — agy-style
    // project:null sessions render under the "unknown project" group — is
    // unchanged.
    renderNodeModal([
      makeCapabilities({
        driver: "claude",
        host_id: "gpu",
        session_scope: "AllKnown",
        sessions: [
          { id: "agy-sess-1", project: null, snippet: "conversation with no mapped project", modified_ms: 300 },
        ],
      }),
    ]);

    fireEvent.click(screen.getByTestId("nm-host-gpu"));
    fireEvent.click(screen.getByTestId("nm-resume-toggle"));

    expect(screen.getByTestId("nm-resume-unknown-project")).toBeTruthy();
    expect(screen.getByTestId("nm-resume-session-agy-sess-1").textContent).toContain(
      "conversation with no mapped project"
    );
  });

  it("opening the resume picker fires refreshCapabilities({host_id, driver, project}) (debounced) and renders the cached list instantly (no spinner-blocked wait)", async () => {
    renderNodeModal([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        sessions: [{ id: "cached-sess", project: "~/projects/api", snippet: "cached", modified_ms: 1 }],
        fetched_at_ms: 1783100000000,
      }),
    ]);

    fillProjectAndResume("~/projects/api");

    // Cached data renders synchronously off fireEvent — no waitFor needed.
    expect(screen.getByTestId("nm-resume-session-cached-sess")).toBeTruthy();
    // The scoped re-probe is DEBOUNCED (~500ms) so per-keystroke typing doesn't
    // flood a slow remote host — assert it fires after the debounce settles.
    await waitFor(() =>
      expect(api.refreshCapabilities as ReturnType<typeof vi.fn>).toHaveBeenCalledWith(
        expect.objectContaining({ host_id: "local", driver: "claude", project: "~/projects/api" })
      )
    );
  });

  it("empty capabilities sessions ⇒ an honest discovering/hint state, never the mock-mode listSessions() fallback list", () => {
    renderNodeModal([makeCapabilities({ driver: "claude", host_id: "local" })]); // sessions: []

    fillProjectAndResume("~/projects/api");

    // Honest-loading: on open, with a project chosen and no sessions cached yet,
    // the picker shows discovery-in-progress ("Discovering…"); once the probe
    // lands empty it shows the "none found" hint. Either is honest — what must
    // NEVER appear is the old hardcoded mock listSessions() list, and
    // listSessions must never be called.
    const honest =
      screen.queryByTestId("nm-resume-loading") ?? screen.queryByTestId("nm-resume-hint");
    expect(honest).toBeTruthy();
    expect(screen.queryByText(/Add a login endpoint to the API/)).toBeNull();
    expect(screen.queryByText(/Refactor the auth middleware/)).toBeNull();
    expect(api.listSessions as ReturnType<typeof vi.fn>).not.toHaveBeenCalled();
  });
});
