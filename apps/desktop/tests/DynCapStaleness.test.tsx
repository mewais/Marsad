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
 * There is no staleness-rendering surface in NewSession.tsx today (no
 * capabilities consumption at all) — this suite is RED until U2 lands it.
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
    refreshCapabilities: vi.fn().mockResolvedValue(undefined),
  };
});

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

describe("Capabilities staleness — 'as of <fetched_at_ms>' rendering", () => {
  it("a fresh fetched_at_ms renders an 'as of …' staleness label", () => {
    renderNewSession([
      makeCapabilities({
        driver: "claude",
        host_id: "local",
        sessions: [{ id: "s1", project: "~/projects/api", snippet: "x", modified_ms: 1 }],
        fetched_at_ms: 1783100000000, // 2026-06-30-ish, a real probe time
      }),
    ]);

    fireEvent.change(screen.getByTestId("ns-project-input"), { target: { value: "~/projects/api" } });
    fireEvent.click(screen.getByTestId("ns-resume-toggle"));

    const label = screen.getByTestId("ns-capabilities-staleness");
    expect(label.textContent).toMatch(/as of/i);
  });

  it("fetched_at_ms: 0 (never probed) renders an honest 'not fetched yet' state, not a formatted 1970 date", () => {
    renderNewSession([
      makeCapabilities({ driver: "claude", host_id: "local", fetched_at_ms: 0 }),
    ]);

    fireEvent.change(screen.getByTestId("ns-project-input"), { target: { value: "~/projects/api" } });
    fireEvent.click(screen.getByTestId("ns-resume-toggle"));

    const label = screen.getByTestId("ns-capabilities-staleness");
    expect(label.textContent).not.toMatch(/1970/);
    expect(label.textContent).toMatch(/never|not (yet )?fetched|no data yet/i);
  });

  it("two different (driver,host) capability snapshots render independent staleness labels", () => {
    renderNewSession([
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

    fireEvent.change(screen.getByTestId("ns-project-input"), { target: { value: "~/projects/api" } });
    fireEvent.click(screen.getByTestId("ns-resume-toggle"));

    expect(screen.getByTestId("ns-capabilities-staleness").textContent).toMatch(/as of/i);

    fireEvent.click(screen.getByTestId("ns-host-hetzner"));

    expect(screen.getByTestId("ns-capabilities-staleness").textContent).toMatch(/never|not (yet )?fetched|no data yet/i);
  });
});
