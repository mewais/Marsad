/**
 * U2 — capabilities staleness rendering.
 *
 * Contract pinned (docs/specs/dynamic-capabilities.md §3.3/§5):
 *   "The cache is ALWAYS served instantly; TTL only decides whether a
 *   background re-probe is queued. `fetched_at_ms` rides the wire so the UI
 *   can render 'as of …' honestly; a persisted-only entry has
 *   `fetched_at_ms` from its last real probe (or 0 if none ever succeeded)."
 *   §5: resume picker "renders the cached list instantly with its
 *   `fetched_at_ms` ('as of …')".
 *
 * `fetched_at_ms: 0` is the "never successfully probed" sentinel (§1
 * `Capabilities.fetched_at_ms` doc comment: "0 = never"), so the UI must
 * render an honest "not fetched yet" state for it — NOT a formatted 1970
 * timestamp, which would misrepresent an epoch-0 sentinel as real data.
 *
 * NodeModal renders the capabilities staleness label ("as of …" / "Not
 * fetched yet") next to the Resume toggle, sourced from the selected
 * (driver, host) capabilities' `fetched_at_ms`.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
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
    refreshCapabilities: vi.fn().mockResolvedValue(undefined),
  };
});

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

describe("Capabilities staleness — 'as of <fetched_at_ms>' rendering", () => {
  it("a fresh fetched_at_ms renders an 'as of …' staleness label", () => {
    renderNodeModal([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        sessions: [{ id: "s1", project: "~/projects/api", snippet: "x", modified_ms: 1 }],
        fetched_at_ms: 1783100000000, // 2026-06-30-ish, a real probe time
      }),
    ]);

    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/projects/api" } });
    fireEvent.click(screen.getByTestId("nm-resume-toggle"));

    const label = screen.getByTestId("nm-capabilities-staleness");
    expect(label.textContent).toMatch(/as of/i);
  });

  it("fetched_at_ms: 0 (never probed) renders an honest 'not fetched yet' state, not a formatted 1970 date", () => {
    renderNodeModal([
      makeCapabilities({ driver: "claude", host_id: "local", fetched_at_ms: 0 }),
    ]);

    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/projects/api" } });
    fireEvent.click(screen.getByTestId("nm-resume-toggle"));

    const label = screen.getByTestId("nm-capabilities-staleness");
    expect(label.textContent).not.toMatch(/1970/);
    expect(label.textContent).toMatch(/never|not (yet )?fetched|no data yet/i);
  });

  it("two different (driver,host) capability snapshots render independent staleness labels", () => {
    renderNodeModal([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        sessions: [{ id: "s-local", project: "~/projects/api", snippet: "x", modified_ms: 1 }],
        fetched_at_ms: 1783100000000,
      }),
      makeCapabilities({
        driver: "claude",
        host_id: "hetzner",
        sessions: [{ id: "s-hetzner", project: "~/projects/api", snippet: "x", modified_ms: 1 }],
        fetched_at_ms: 0,
      }),
    ]);

    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/projects/api" } });
    fireEvent.click(screen.getByTestId("nm-resume-toggle"));

    expect(screen.getByTestId("nm-capabilities-staleness").textContent).toMatch(/as of/i);

    fireEvent.click(screen.getByTestId("nm-host-hetzner"));

    expect(screen.getByTestId("nm-capabilities-staleness").textContent).toMatch(/never|not (yet )?fetched|no data yet/i);
  });
});
