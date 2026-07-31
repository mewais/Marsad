/**
 * NodeModalModes — session-mode picker driven by capabilities.
 *
 * Contract:
 *   - When capabilities for (driver, host) report available_modes, the NodeModal
 *     renders those modes (no hardcoded posture list).
 *   - Launching passes the chosen session_mode to launchSession.
 *   - When modeList is empty, no mode picker renders and session_mode is "".
 *   - Default selection follows Settings default_modes[driver] if present in
 *     modeList, else modeList[0]?.id, else "".
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NodeModal } from "../src/screens/NodeModal";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, CapabilitiesView, SessionMode } from "../src/bridge/types";

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    launchSession: vi.fn().mockResolvedValue("mock-session-new"),
    // Keep getSettings as the original (returns DEFAULT_SETTINGS synchronously in non-Tauri).
    assignSession: vi.fn().mockResolvedValue(undefined),
  };
});

import * as api from "../src/bridge/api";

beforeEach(() => {
  vi.clearAllMocks();
});
afterEach(() => {
  vi.restoreAllMocks();
});

function makeCapWithModes(modes: SessionMode[]): Record<string, CapabilitiesView> {
  return {
    "claude::local": {
      driver: "claude",
      host_id: "local",
      cli_available: true,
      launchable: true,
      models: {
        models: [{ id: "claude-sonnet-latest" }],
        source: "Listed",
        free_text: false,
      },
      session_scope: null,
      sessions: [],
      fetched_at_ms: Date.now(),
      errors: [],
      available_modes: modes,
    },
  };
}

function renderNodeModal(capabilities: Record<string, CapabilitiesView>) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "node-modal",
    capabilities,
    nodeModalCtx: { wsId: "ws-1", wsName: "Test WS", isFirstNode: true },
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NodeModal />
      </VmProvider>
    </ThemeProvider>
  );
}

const MODES: SessionMode[] = [
  { id: "plan", label: "Plan", description: "Plan before acting" },
  { id: "acceptEdits", label: "Accept edits", description: "Accept edits without prompting" },
];

// Sync render tests first — no async promises kept alive.
describe("NodeModal — session-mode picker render (sync)", () => {
  it("renders mode buttons from capabilities.available_modes", () => {
    renderNodeModal(makeCapWithModes(MODES));
    expect(screen.getByTestId("nm-mode-plan")).toBeTruthy();
    expect(screen.getByTestId("nm-mode-acceptEdits")).toBeTruthy();
  });

  it("shows mode label and description", () => {
    renderNodeModal(makeCapWithModes(MODES));
    expect(screen.getByText("Plan")).toBeTruthy();
    expect(screen.getByText("Plan before acting")).toBeTruthy();
  });

  it("no mode picker when modeList is empty", () => {
    renderNodeModal(makeCapWithModes([]));
    expect(screen.queryByTestId(/nm-mode-/)).toBeNull();
  });
});

// Async launch tests last — vi.waitFor completes and no subsequent render() is called.
describe("NodeModal — session-mode picker launch (async)", () => {
  it("defaults to first mode and passes it as session_mode to launchSession", async () => {
    renderNodeModal(makeCapWithModes(MODES));
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/proj" } });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));
    await vi.waitFor(() => {
      expect(api.launchSession as ReturnType<typeof vi.fn>).toHaveBeenCalledWith(
        expect.objectContaining({ session_mode: "plan" })
      );
    });
  });

  it("choosing a mode passes it to launchSession as session_mode", async () => {
    renderNodeModal(makeCapWithModes(MODES));
    fireEvent.click(screen.getByTestId("nm-mode-acceptEdits"));
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/proj" } });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));
    await vi.waitFor(() => {
      expect(api.launchSession as ReturnType<typeof vi.fn>).toHaveBeenCalledWith(
        expect.objectContaining({ session_mode: "acceptEdits" })
      );
    });
  });

  it("session_mode is empty string when no modes in catalog", async () => {
    renderNodeModal(makeCapWithModes([]));
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/proj" } });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));
    await vi.waitFor(() => {
      expect(api.launchSession as ReturnType<typeof vi.fn>).toHaveBeenCalledWith(
        expect.objectContaining({ session_mode: "" })
      );
    });
  });
});
