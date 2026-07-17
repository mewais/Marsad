/**
 * Discovery-in-flight optimism + real gating.
 *
 * While capabilities for a (driver, host) are ABSENT (real per-host discovery
 * in flight), a driver must be treated as LAUNCHABLE (optimistic) — not greyed —
 * and the model area shows a loading indicator (not the empty state). Once caps
 * are PRESENT, gating is authoritative: available iff launchable && cli_available.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NodeModal } from "../src/screens/NodeModal";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { CapabilitiesView } from "../src/bridge/types";

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    launchSession: vi.fn().mockResolvedValue("mock-session-new"),
    assignSession: vi.fn().mockResolvedValue(undefined),
    refreshCapabilities: vi.fn().mockResolvedValue(undefined),
  };
});

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.restoreAllMocks());

/** launchable claude whose CLI was NOT detected (real known-false). */
function cliMissingClaude(hostId: string): Record<string, CapabilitiesView> {
  return {
    [`claude::${hostId}`]: {
      driver: "claude",
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

describe("NodeModal — discovery-in-flight optimism", () => {
  it("ABSENT capabilities: claude driver is NOT greyed and has no unavailable note", () => {
    renderNodeModal({});
    const claudeBtn = screen.getByTestId("nm-driver-claude") as HTMLButtonElement;
    expect(claudeBtn.disabled).toBe(false);
    expect(screen.queryByTestId("nm-driver-unavailable-note")).toBeNull();
  });

  it("ABSENT capabilities: a loading indicator shows in the model area (not the empty hint)", () => {
    renderNodeModal({});
    expect(screen.getByTestId("nm-model-loading")).toBeTruthy();
    expect(screen.queryByTestId("nm-model-empty-hint")).toBeNull();
  });

  it("ABSENT capabilities: Launch is enabled once a project is set", () => {
    renderNodeModal({});
    expect((screen.getByTestId("nm-launch-btn") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/release" },
    });
    expect((screen.getByTestId("nm-launch-btn") as HTMLButtonElement).disabled).toBe(false);
  });

  it("PRESENT caps (launchable:true, cli_available:false): claude is greyed and Launch stays disabled", () => {
    renderNodeModal(cliMissingClaude("local"));
    expect((screen.getByTestId("nm-driver-claude") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId("nm-driver-unavailable-note")).toBeTruthy();
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/release" },
    });
    expect((screen.getByTestId("nm-launch-btn") as HTMLButtonElement).disabled).toBe(true);
  });
});
