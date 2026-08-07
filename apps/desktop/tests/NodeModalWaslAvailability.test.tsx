/**
 * NodeModalWaslAvailability — a "Build it out" (design-mode) workstream launches
 * every agent over the Wasl transport (#8), so a driver is available in design
 * mode only when BOTH its native CLI (`cli_available`) AND wasl itself
 * (`wasl_available`, i.e. wasl-mcpd on the host) are present. A single-session
 * (non-design) launch is native and needs only the CLI.
 *
 * This replaces the old NodeModalCapsAlias suite: there is no longer a "wasl-*"
 * driver identity to alias — the driver is always native; wasl-ness is a
 * workstream-TYPE decision, and that is what gates availability here.
 *
 * Observable: `nm-launch-btn` is disabled when the chosen driver is unavailable
 * (with the project filled, so the only remaining gate is driverAvailable).
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
  };
});

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.restoreAllMocks());

function claudeCaps(opts: { cli: boolean; wasl: boolean }): Record<string, CapabilitiesView> {
  return {
    "claude::local": {
      driver: "claude",
      host_id: "local",
      cli_available: opts.cli,
      wasl_available: opts.wasl,
      launchable: true,
      models: { models: [{ id: "claude-sonnet-latest" }], source: "Listed", free_text: true },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 1, // caps PRESENT (not discovering) so availability is authoritative
      errors: [],
    },
  };
}

function renderModal(caps: Record<string, CapabilitiesView>, designMode: boolean) {
  const preloaded: Partial<VmState> = {
    snapshot: { ...MOCK_SNAPSHOT },
    screen: "node-modal",
    capabilities: caps,
    nodeModalCtx: { wsId: "ws-1", wsName: "Test WS", isFirstNode: true, designMode },
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NodeModal />
      </VmProvider>
    </ThemeProvider>
  );
}

function launchBtn(): HTMLButtonElement {
  return screen.getByTestId("nm-launch-btn") as HTMLButtonElement;
}

describe("NodeModal — Wasl availability gating (#8)", () => {
  it("single-session (non-design): CLI present + wasl absent → driver AVAILABLE", () => {
    renderModal(claudeCaps({ cli: true, wasl: false }), false);
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/proj" } });
    expect(launchBtn().disabled).toBe(false);
  });

  it("build-it-out (design): CLI present but wasl ABSENT → driver UNAVAILABLE", () => {
    renderModal(claudeCaps({ cli: true, wasl: false }), true);
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/proj" } });
    expect(launchBtn().disabled).toBe(true);
  });

  it("build-it-out (design): CLI present AND wasl present → driver AVAILABLE", () => {
    renderModal(claudeCaps({ cli: true, wasl: true }), true);
    fireEvent.change(screen.getByTestId("nm-project-input"), { target: { value: "~/proj" } });
    expect(launchBtn().disabled).toBe(false);
  });
});
