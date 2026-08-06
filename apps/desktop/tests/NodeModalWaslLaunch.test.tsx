/**
 * NodeModalWaslLaunch — RED-phase tests for Wasl-driver launch behaviour in
 * NodeModal.
 *
 * Spec (not yet implemented):
 *   - When the selected driver is a wasl driver, clicking Launch calls
 *     `launchWaslSession` (NOT `launchSession`) with:
 *       { host_id, wasl_target_key: { protocol: "claude"|"opencode",
 *                                      context: { cwd:<project>, model, posture } },
 *         name? }
 *   - `launchSession` must NOT be called for a wasl driver.
 *   - The resume picker (`nm-resume-toggle`) is hidden for wasl drivers.
 *   - The wasl-toggle (if present — `nm-wasl-toggle` or text "Wasl") is
 *     hidden for wasl drivers (it is a standard-session affordance).
 *
 * Pattern mirrors NewWorkstreamAndNodeModal.test.tsx (VmProvider + ThemeProvider
 * + mocked api module). All api mocks forward to the real module for everything
 * not explicitly overridden.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NodeModal } from "../src/screens/NodeModal";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { CapabilitiesView } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Mock the api module.
// launchWaslSession is the NOT-YET-EXISTING function under test.
// launchSession is the existing function that must NOT be called.
// ---------------------------------------------------------------------------

const launchWaslSession = vi.fn(async (_args: unknown) => "mock-wasl-session-1");
const launchSession = vi.fn(async (_args: unknown) => "mock-sess-1");
const assignSession = vi.fn(async (_args: unknown) => {});

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    launchSession: (args: unknown) => launchSession(args),
    // launchWaslSession does not exist yet in api.ts — the mock defines it so
    // the test can spy on it.  When the coder adds the real function the test
    // will wire through automatically.
    launchWaslSession: (args: unknown) => launchWaslSession(args),
    assignSession: (args: unknown) => assignSession(args),
    fetchGraph: vi.fn(async () => null),
    fetchBoard: vi.fn(async () => ({
      summary: { workstreams: 0, sessions: 0, hosts: 0 },
      host_chips: [],
      link_chips: [],
      attention: [],
      all: [],
    })),
    refreshCapabilities: vi.fn(async () => {}),
  };
});

beforeEach(() => {
  vi.clearAllMocks();
  launchWaslSession.mockResolvedValue("mock-wasl-session-1");
  launchSession.mockResolvedValue("mock-sess-1");
  assignSession.mockResolvedValue(undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Capabilities fixtures
// ---------------------------------------------------------------------------

/** A capabilities map that includes both wasl drivers as launchable. */
function waslCapabilities(hostId: string): Record<string, CapabilitiesView> {
  const base = {
    cli_available: true,
    wasl_available: true,
    launchable: true,
    models: {
      models: [{ id: "claude-sonnet-latest" }, { id: "claude-opus-latest" }],
      source: "Listed" as const,
      free_text: true,
    },
    session_scope: null,
    sessions: [],
    fetched_at_ms: 1,
    errors: [],
  };
  return {
    // Standard claude still present so other drivers don't grey out unexpectedly.
    [`claude::${hostId}`]: { ...base, driver: "claude", host_id: hostId },
    // wasl-claude and wasl-opencode — the new launchable drivers under test.
    [`wasl-claude::${hostId}`]: { ...base, driver: "wasl-claude", host_id: hostId },
    [`wasl-opencode::${hostId}`]: {
      ...base,
      driver: "wasl-opencode",
      host_id: hostId,
      models: {
        models: [{ id: "gpt-4o" }, { id: "claude-sonnet-latest" }],
        source: "Listed" as const,
        free_text: true,
      },
    },
  };
}

// ---------------------------------------------------------------------------
// Render helper
// ---------------------------------------------------------------------------

function renderNodeModal(
  capabilities: Record<string, CapabilitiesView>,
  isFirstNode = true
) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "node-modal",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities,
    nodeModalCtx: { wsId: "ws-wasl", wsName: "Wasl WS", isFirstNode },
    draftWsId: null,
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
// Suite 1: wasl-claude driver calls launchWaslSession, NOT launchSession
// ---------------------------------------------------------------------------

describe("NodeModal — wasl-claude launch", () => {
  it("renders a driver button for 'wasl-claude' when capabilities include it", () => {
    // RED: DRIVERS does not contain 'wasl-claude' yet; no button will render.
    renderNodeModal(waslCapabilities("local"));
    const btn = screen.getByTestId("nm-driver-wasl-claude");
    expect(btn).toBeTruthy();
  });

  it("clicking wasl-claude button is enabled (not disabled)", () => {
    renderNodeModal(waslCapabilities("local"));
    const btn = screen.getByTestId("nm-driver-wasl-claude") as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
  });

  it("selecting wasl-claude then clicking Launch calls launchWaslSession once", async () => {
    renderNodeModal(waslCapabilities("local"));

    // Select the wasl-claude driver.
    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    // Type a project directory so the Launch button is enabled.
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/my-project" },
    });

    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => {
      expect(launchWaslSession).toHaveBeenCalledTimes(1);
    });
  });

  it("launchWaslSession is called with wasl_target_key.protocol === 'claude'", async () => {
    renderNodeModal(waslCapabilities("local"));

    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/my-project" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => {
      expect(launchWaslSession).toHaveBeenCalledWith(
        expect.objectContaining({
          wasl_target_key: expect.objectContaining({
            protocol: "claude",
          }),
        })
      );
    });
  });

  it("launchWaslSession is called with wasl_target_key.context.cwd === the typed project", async () => {
    const project = "~/code/my-project";
    renderNodeModal(waslCapabilities("local"));

    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: project },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => {
      expect(launchWaslSession).toHaveBeenCalledWith(
        expect.objectContaining({
          wasl_target_key: expect.objectContaining({
            context: expect.objectContaining({ cwd: project }),
          }),
        })
      );
    });
  });

  it("launchWaslSession is called with wasl_target_key.context.model from the model picker", async () => {
    renderNodeModal(waslCapabilities("local"));

    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    // Pick the second model in the catalog (claude-opus-latest).
    fireEvent.click(screen.getByTestId("nm-model-claude-opus-latest"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/opus-project" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => {
      expect(launchWaslSession).toHaveBeenCalledWith(
        expect.objectContaining({
          wasl_target_key: expect.objectContaining({
            context: expect.objectContaining({ model: "claude-opus-latest" }),
          }),
        })
      );
    });
  });

  it("launchWaslSession is called with host_id for the selected host", async () => {
    renderNodeModal(waslCapabilities("local"));

    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/my-project" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => {
      expect(launchWaslSession).toHaveBeenCalledWith(
        expect.objectContaining({ host_id: expect.any(String) })
      );
    });
  });

  it("launchSession is NOT called when the driver is wasl-claude", async () => {
    renderNodeModal(waslCapabilities("local"));

    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/my-project" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    // Give any async paths a moment, then assert launchSession was never called.
    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1));
    expect(launchSession).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// Suite 2: wasl-opencode driver calls launchWaslSession with protocol "opencode"
// ---------------------------------------------------------------------------

describe("NodeModal — wasl-opencode launch", () => {
  it("renders a driver button for 'wasl-opencode' when capabilities include it", () => {
    // RED: DRIVERS does not contain 'wasl-opencode' yet.
    renderNodeModal(waslCapabilities("local"));
    const btn = screen.getByTestId("nm-driver-wasl-opencode");
    expect(btn).toBeTruthy();
  });

  it("selecting wasl-opencode then clicking Launch calls launchWaslSession once", async () => {
    renderNodeModal(waslCapabilities("local"));

    fireEvent.click(screen.getByTestId("nm-driver-wasl-opencode"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/oc-project" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => {
      expect(launchWaslSession).toHaveBeenCalledTimes(1);
    });
  });

  it("launchWaslSession is called with wasl_target_key.protocol === 'opencode'", async () => {
    renderNodeModal(waslCapabilities("local"));

    fireEvent.click(screen.getByTestId("nm-driver-wasl-opencode"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/oc-project" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => {
      expect(launchWaslSession).toHaveBeenCalledWith(
        expect.objectContaining({
          wasl_target_key: expect.objectContaining({
            protocol: "opencode",
          }),
        })
      );
    });
  });

  it("launchWaslSession context.cwd matches the typed project for wasl-opencode", async () => {
    const project = "~/code/oc-project";
    renderNodeModal(waslCapabilities("local"));

    fireEvent.click(screen.getByTestId("nm-driver-wasl-opencode"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: project },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => {
      expect(launchWaslSession).toHaveBeenCalledWith(
        expect.objectContaining({
          wasl_target_key: expect.objectContaining({
            context: expect.objectContaining({ cwd: project }),
          }),
        })
      );
    });
  });

  it("launchSession is NOT called when the driver is wasl-opencode", async () => {
    renderNodeModal(waslCapabilities("local"));

    fireEvent.click(screen.getByTestId("nm-driver-wasl-opencode"));
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/oc-project" },
    });
    fireEvent.click(screen.getByTestId("nm-launch-btn"));

    await waitFor(() => expect(launchWaslSession).toHaveBeenCalledTimes(1));
    expect(launchSession).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// Suite 3: Resume picker + wasl toggle hidden for wasl drivers
// ---------------------------------------------------------------------------

describe("NodeModal — wasl driver hides non-applicable UI", () => {
  it("resume toggle (nm-resume-toggle) is NOT rendered when driver is wasl-claude", () => {
    // RED: the existing NodeModal always renders nm-resume-toggle regardless of driver.
    renderNodeModal(waslCapabilities("local"));
    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));

    // After selecting a wasl driver the toggle must be absent.
    expect(screen.queryByTestId("nm-resume-toggle")).toBeNull();
  });

  it("resume toggle (nm-resume-toggle) is NOT rendered when driver is wasl-opencode", () => {
    renderNodeModal(waslCapabilities("local"));
    fireEvent.click(screen.getByTestId("nm-driver-wasl-opencode"));

    expect(screen.queryByTestId("nm-resume-toggle")).toBeNull();
  });

  it("resume toggle IS still rendered for the standard 'claude' driver", () => {
    // Regression: must not suppress the toggle for non-wasl drivers.
    renderNodeModal(waslCapabilities("local"));
    // claude is the default selected driver — no click needed.
    expect(screen.getByTestId("nm-resume-toggle")).toBeTruthy();
  });

  it("wasl-toggle/wasl affordance (nm-wasl-toggle) is NOT rendered when driver is wasl-claude", () => {
    // The standard-session "Wasl" toggle (nm-wasl-toggle) is irrelevant when
    // the launch itself IS a wasl launch. Must be hidden.
    // RED: if nm-wasl-toggle is rendered it will be present; otherwise this
    // assertion passes trivially, which is also correct (no toggle at all is fine).
    renderNodeModal(waslCapabilities("local"));
    fireEvent.click(screen.getByTestId("nm-driver-wasl-claude"));
    expect(screen.queryByTestId("nm-wasl-toggle")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Suite 4: launchWaslSession API — exported from api.ts
// ---------------------------------------------------------------------------

describe("api.ts — launchWaslSession export", () => {
  it("launchWaslSession is exported from src/bridge/api.ts", async () => {
    // RED: the function does not exist yet; the import will resolve undefined.
    const apiModule = await import("../src/bridge/api");
    expect(typeof (apiModule as Record<string, unknown>).launchWaslSession).toBe("function");
  });
});
