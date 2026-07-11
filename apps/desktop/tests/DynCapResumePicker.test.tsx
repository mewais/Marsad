/**
 * U2 — NewSession resume picker consumes ONLY dynamic capabilities.
 *
 * Contract pinned (docs/specs/dynamic-capabilities.md §5, "Resume picker"):
 *   "reads capabilities[...].sessions filtered by chosen project (agy
 *   `project: None` entries render under 'unknown project'); fires
 *   `refreshCapabilities({host_id, driver, project})` on open and renders
 *   the cached list instantly with its `fetched_at_ms` ('as of …') —
 *   stale-then-fresh, never spinner-blocked."
 *
 * Today `NewSession.tsx:124` calls `listSessions(project, hostId)` — the
 * Tauri `list_sessions` command hardcoding claude's on-disk layout
 * (`src-tauri/bridge.rs:127-190`, kill-listed by §5/§"Kill list", deleted in
 * U2). In mock/test mode that wrapper unconditionally returns two fake
 * `SessionSummary`s regardless of the project/host — this suite proves the
 * resume list must come from capabilities instead, and is RED until U2
 * rewires it.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NewSession } from "../src/screens/NewSession";
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
    // observable the moment U2 wires NewSession onto it.
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

function renderNewSession(caps: CapabilitiesViewFixture[]) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT };
  const preloaded = {
    snapshot,
    screen: "new-session",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: capsMap(caps),
  } as unknown as Partial<VmState>;
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NewSession />
      </VmProvider>
    </ThemeProvider>
  );
}

function fillProjectAndResume(project: string) {
  fireEvent.change(screen.getByTestId("ns-project-input"), { target: { value: project } });
  fireEvent.click(screen.getByTestId("ns-resume-toggle"));
}

describe("NewSession resume picker — capabilities-only source (U2 contract)", () => {
  it("lists sessions from capabilities[('claude','local')].sessions, and never calls the kill-listed listSessions()", () => {
    renderNewSession([
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

    expect(screen.getByTestId("ns-resume-session-cap-sess-1").textContent).toContain("Add caching layer");
    // The old hardcoded mock entries must not leak in.
    expect(screen.queryByText(/Add a login endpoint to the API/)).toBeNull();
    expect(screen.queryByText(/Refactor the auth middleware/)).toBeNull();
    expect(api.listSessions as ReturnType<typeof vi.fn>).not.toHaveBeenCalled();
  });

  it("filters sessions to the chosen project — a session from a different project does not leak in", () => {
    renderNewSession([
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

    expect(screen.getByTestId("ns-resume-session-sess-api")).toBeTruthy();
    expect(screen.queryByTestId("ns-resume-session-sess-other")).toBeNull();
  });

  it("agy-style sessions with project: null render under an 'unknown project' group, per SessionScope semantics", () => {
    renderNewSession([
      makeCapabilities({
        driver: "gemini",
        host_id: "gpu",
        session_scope: "AllKnown",
        sessions: [
          { id: "agy-sess-1", project: null, snippet: "conversation with no mapped project", modified_ms: 300 },
        ],
      }),
    ]);

    fireEvent.click(screen.getByTestId("ns-host-gpu"));
    fireEvent.click(screen.getByTestId("ns-resume-toggle"));

    expect(screen.getByTestId("ns-resume-unknown-project")).toBeTruthy();
    expect(screen.getByTestId("ns-resume-session-agy-sess-1").textContent).toContain(
      "conversation with no mapped project"
    );
  });

  it("opening the resume picker fires refreshCapabilities({host_id, driver, project}) and renders the cached list instantly (no spinner-blocked wait)", () => {
    renderNewSession([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        sessions: [{ id: "cached-sess", project: "~/projects/api", snippet: "cached", modified_ms: 1 }],
        fetched_at_ms: 1783100000000,
      }),
    ]);

    fillProjectAndResume("~/projects/api");

    // Cached data renders synchronously off fireEvent — no waitFor needed.
    expect(screen.getByTestId("ns-resume-session-cached-sess")).toBeTruthy();
    expect(api.refreshCapabilities as ReturnType<typeof vi.fn>).toHaveBeenCalledWith(
      expect.objectContaining({ host_id: "local", driver: "claude", project: "~/projects/api" })
    );
  });

  it("empty capabilities sessions ⇒ an honest discovering/hint state, never the mock-mode listSessions() fallback list", () => {
    renderNewSession([makeCapabilities({ driver: "claude", host_id: "local" })]); // sessions: []

    fillProjectAndResume("~/projects/api");

    // Honest-loading: on open, with a project chosen and no sessions cached yet,
    // the picker shows discovery-in-progress ("Discovering…"); once the probe
    // lands empty it shows the "none found" hint. Either is honest — what must
    // NEVER appear is the old hardcoded mock listSessions() list, and
    // listSessions must never be called.
    const honest =
      screen.queryByTestId("ns-resume-loading") ?? screen.queryByTestId("ns-resume-hint");
    expect(honest).toBeTruthy();
    expect(screen.queryByText(/Add a login endpoint to the API/)).toBeNull();
    expect(screen.queryByText(/Refactor the auth middleware/)).toBeNull();
    expect(api.listSessions as ReturnType<typeof vi.fn>).not.toHaveBeenCalled();
  });
});
