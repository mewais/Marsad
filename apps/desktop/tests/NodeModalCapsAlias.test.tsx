/**
 * NodeModalCapsAlias — NodeModal reads capabilities via alias for wasl-* drivers.
 *
 * Architecture (spec): capabilities are host+underlying-CLI facts; Wasl-ness is
 * only a launch-transport choice. wasl-claude → reads caps from "claude" slot,
 * wasl-opencode → "opencode", wasl-codex → "codex". The LAUNCH path stays as-is
 * (uses the real driver id). Only cap READS + the discovery TRIGGER use the alias.
 *
 * Mirrors NodeModalModes.test.tsx harness (tight, sync-first).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NodeModal } from "../src/screens/NodeModal";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { CapabilitiesView } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// API mock — spy on refreshCapabilities and launchWaslSession
// ---------------------------------------------------------------------------

const mockRefreshCapabilities = vi.fn(async (_args: unknown) => {});
const mockLaunchWaslSession = vi.fn(async (_args: unknown) => "mock-alias-session");
const mockLaunchSession = vi.fn(async (_args: unknown) => "mock-native-session");
const mockAssignSession = vi.fn(async (_args: unknown) => {});

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    launchSession: (args: unknown) => mockLaunchSession(args),
    launchWaslSession: (args: unknown) => mockLaunchWaslSession(args),
    assignSession: (args: unknown) => mockAssignSession(args),
    fetchGraph: vi.fn(async () => null),
    fetchBoard: vi.fn(async () => ({
      summary: { workstreams: 0, sessions: 0, hosts: 0 },
      host_chips: [],
      link_chips: [],
      attention: [],
      all: [],
    })),
    refreshCapabilities: (args: unknown) => mockRefreshCapabilities(args),
  };
});

beforeEach(() => {
  vi.clearAllMocks();
  mockRefreshCapabilities.mockResolvedValue(undefined);
  mockLaunchWaslSession.mockResolvedValue("mock-alias-session");
  mockLaunchSession.mockResolvedValue("mock-native-session");
  mockAssignSession.mockResolvedValue(undefined);
});
afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Capability fixtures — ONLY underlying protocol slots seeded (no wasl-* slots)
// ---------------------------------------------------------------------------

const HOST = "local";

/** Capabilities keyed ONLY by underlying protocol. wasl-* slots intentionally absent. */
function underlyingCaps(): Record<string, CapabilitiesView> {
  return {
    [`claude::${HOST}`]: {
      driver: "claude",
      host_id: HOST,
      cli_available: true,
      launchable: true,
      models: {
        models: [{ id: "alias-claude-model-a" }, { id: "alias-claude-model-b" }],
        source: "Listed" as const,
        free_text: false,
      },
      session_scope: null,
      sessions: [
        { id: "alias-sess-1", project: "~/proj", snippet: "aliased session", modified_ms: 1783200000000 },
      ],
      fetched_at_ms: 1783200000000,
      errors: [],
      available_modes: [
        { id: "plan", label: "Plan", description: "Plan first" },
      ],
    },
    [`opencode::${HOST}`]: {
      driver: "opencode",
      host_id: HOST,
      cli_available: true,
      launchable: true,
      models: {
        models: [{ id: "alias-oc-model" }],
        source: "Listed" as const,
        free_text: false,
      },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 1783200000000,
      errors: [],
      available_modes: [],
    },
    [`codex::${HOST}`]: {
      driver: "codex",
      host_id: HOST,
      cli_available: true,
      launchable: true,
      models: {
        models: [{ id: "alias-codex-model" }],
        source: "Listed" as const,
        free_text: false,
      },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 1783200000000,
      errors: [],
      available_modes: [],
    },
  };
}

// ---------------------------------------------------------------------------
// Render helper
// ---------------------------------------------------------------------------

function renderNodeModal(caps: Record<string, CapabilitiesView>) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "node-modal",
    capabilities: caps,
    nodeModalCtx: { wsId: "ws-alias", wsName: "Alias WS", isFirstNode: true },
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NodeModal />
      </VmProvider>
    </ThemeProvider>
  );
}

// ---------------------------------------------------------------------------
// Suite 1: wasl-claude reads models + modes from underlying claude slot (sync)
// ---------------------------------------------------------------------------

describe("NodeModal — wasl-claude caps alias (sync)", () => {
  it("wasl-claude driver button exists", () => {
    renderNodeModal(underlyingCaps());
    expect(screen.getByTestId("nm-driver-wasl-claude")).toBeTruthy();
  });

  it("selecting wasl-claude shows models from underlying claude slot (not 'discovering models')", () => {
    renderNodeModal(underlyingCaps());
    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    // Models from claude::local appear via alias.
    expect(screen.getByTestId("nm-model-alias-claude-model-a")).toBeTruthy();
    expect(screen.getByTestId("nm-model-alias-claude-model-b")).toBeTruthy();
    // No discovering spinner — caps found.
    expect(screen.queryByTestId("nm-model-loading")).toBeNull();
  });

  it("selecting wasl-claude shows modes from underlying claude slot", () => {
    renderNodeModal(underlyingCaps());
    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    expect(screen.getByTestId("nm-mode-plan")).toBeTruthy();
  });

  it("resume toggle is NOT rendered for wasl-claude (wasl sessions don't resume)", () => {
    renderNodeModal(underlyingCaps());
    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    expect(screen.queryByTestId("nm-resume-toggle")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Suite 2: wasl-opencode reads from underlying opencode slot (sync)
// ---------------------------------------------------------------------------

describe("NodeModal — wasl-opencode caps alias (sync)", () => {
  it("selecting wasl-opencode shows models from underlying opencode slot", () => {
    renderNodeModal(underlyingCaps());
    fireEvent.click(screen.getByTestId("nm-driver-wasl-opencode"));
    expect(screen.getByTestId("nm-model-alias-oc-model")).toBeTruthy();
    expect(screen.queryByTestId("nm-model-loading")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Suite 3: wasl-codex reads from underlying codex slot (sync)
// ---------------------------------------------------------------------------

describe("NodeModal — wasl-codex caps alias (sync)", () => {
  it("wasl-codex driver button exists", () => {
    renderNodeModal(underlyingCaps());
    expect(screen.getByTestId("nm-driver-wasl-codex")).toBeTruthy();
  });

  it("wasl-codex driver button is enabled when codex caps exist", () => {
    renderNodeModal(underlyingCaps());
    const btn = screen.getByTestId("nm-driver-wasl-codex") as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
  });

  it("selecting wasl-codex shows models from underlying codex slot", () => {
    renderNodeModal(underlyingCaps());
    fireEvent.click(screen.getByTestId("nm-driver-wasl-codex"));
    expect(screen.getByTestId("nm-model-alias-codex-model")).toBeTruthy();
    expect(screen.queryByTestId("nm-model-loading")).toBeNull();
  });

  it("resume toggle is NOT rendered for wasl-codex", () => {
    renderNodeModal(underlyingCaps());
    fireEvent.click(screen.getByTestId("nm-driver-wasl-codex"));
    expect(screen.queryByTestId("nm-resume-toggle")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Suite 4: native claude still reads its own slot (regression, sync)
// ---------------------------------------------------------------------------

describe("NodeModal — native drivers still read own slot (regression, sync)", () => {
  it("native claude (default) shows claude::local models", () => {
    renderNodeModal(underlyingCaps());
    // Default driver is claude — reads claude::local directly.
    expect(screen.getByTestId("nm-model-alias-claude-model-a")).toBeTruthy();
  });

  it("native opencode shows opencode::local models", () => {
    renderNodeModal(underlyingCaps());
    fireEvent.click(screen.getByTestId("nm-driver-opencode"));
    expect(screen.getByTestId("nm-model-alias-oc-model")).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// Suite 5: refreshCapabilities discovery trigger uses UNDERLYING protocol (async)
//
// The discovery effect fires when caps for (capDriver, host) are ABSENT.
// We seed an EMPTY caps map so all drivers trigger the probe on open.
// ---------------------------------------------------------------------------

describe("NodeModal — discovery trigger uses underlying protocol (async)", () => {
  it("when wasl-claude is selected (no caps), refreshCapabilities fires with driver:'claude' not 'wasl-claude'", async () => {
    // Empty caps → all drivers trigger the discovery probe on open.
    renderNodeModal({});
    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    await waitFor(() => {
      // Must be called with the UNDERLYING protocol, not the wasl variant.
      expect(mockRefreshCapabilities).toHaveBeenCalledWith(
        expect.objectContaining({ driver: "claude" })
      );
    });
    // The wasl driver id must NEVER be passed to refreshCapabilities.
    const calls = mockRefreshCapabilities.mock.calls.map((c) => (c[0] as { driver?: string }).driver);
    expect(calls).not.toContain("wasl-claude");
  });

  it("when wasl-codex is selected (no caps), refreshCapabilities fires with driver:'codex' not 'wasl-codex'", async () => {
    // Open with default claude driver first (seeded with claude caps to suppress its probe),
    // then switch to wasl-codex with empty codex caps to trigger a probe.
    const capsWithClaudeOnly: Record<string, CapabilitiesView> = {
      [`claude::${HOST}`]: underlyingCaps()[`claude::${HOST}`],
    };
    renderNodeModal(capsWithClaudeOnly);
    // Switch to wasl-codex — codex slot is absent → probe fires.
    fireEvent.click(screen.getByTestId("nm-driver-wasl-codex"));
    await waitFor(() => {
      expect(mockRefreshCapabilities).toHaveBeenCalledWith(
        expect.objectContaining({ driver: "codex" })
      );
    });
    const calls = mockRefreshCapabilities.mock.calls.map((c) => (c[0] as { driver?: string }).driver);
    expect(calls).not.toContain("wasl-codex");
  });
});

// ---------------------------------------------------------------------------
// Suite 6: launch path uses REAL driver id (not the alias)
// ---------------------------------------------------------------------------

describe("NodeModal — launch path uses real driver id (async)", () => {
  it("wasl-codex launch calls launchWaslSession with protocol:'codex'", async () => {
    renderNodeModal(underlyingCaps());
    fireEvent.click(screen.getByTestId("nm-driver-wasl-codex"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/cdx" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));
    await waitFor(() => {
      expect(mockLaunchWaslSession).toHaveBeenCalledWith(
        expect.objectContaining({
          wasl_target_key: expect.objectContaining({ protocol: "codex" }),
        })
      );
    });
    expect(mockLaunchSession).not.toHaveBeenCalled();
  });

  it("wasl-claude launch calls launchWaslSession with protocol:'claude'", async () => {
    renderNodeModal(underlyingCaps());
    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/cl" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));
    await waitFor(() => {
      expect(mockLaunchWaslSession).toHaveBeenCalledWith(
        expect.objectContaining({
          wasl_target_key: expect.objectContaining({ protocol: "claude" }),
        })
      );
    });
    expect(mockLaunchSession).not.toHaveBeenCalled();
  });
});
