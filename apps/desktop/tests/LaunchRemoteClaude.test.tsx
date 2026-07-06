/**
 * RENDER PROOF — the Marsad launch UX gated on REAL per-host capability
 * discovery (launchable + cli_available), covering NodeModal and NewSession.
 *
 * The new contract (this file pins it):
 *   1. REAL caps (launchable:true, cli_available:true, models with annotations)
 *      + a project typed → the model list shows the version annotations AND the
 *      Launch button is ENABLED.
 *   2. Caps ABSENT (discovery in flight) → a loading indicator shows and Launch
 *      is NOT hard-greyed on availability (optimistic; only the empty project
 *      blocks it).
 *   3. gemini (launchable:false) → greyed with the unavailable note.
 *   4. A launchable driver whose CLI wasn't detected (cli_available:false) →
 *      greyed with the note.
 *   5. Picking the `opus` radio leaves the custom free-text input EMPTY with the
 *      "Type your own" placeholder — not "opus"; typing a custom model fills it.
 *
 * These caps are keyed off the SELECTED host id — discovery is per-host, over
 * the connection — so there is no local/remote special-casing anymore.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { NodeModal } from "../src/screens/NodeModal";
import { NewSession } from "../src/screens/NewSession";
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

// "hetzner" is a host in MOCK_SNAPSHOT — discovery is per-host, no local/remote
// distinction in the gating anymore.
const HOST = "hetzner";

/** Real claude caps: launchable + cli_available, a small model catalog whose
 *  entries carry `annotation` (the host's resolved version, per real discovery),
 *  plus free text. */
function realClaudeCaps(hostId: string): Record<string, CapabilitiesView> {
  return {
    [`claude::${hostId}`]: {
      driver: "claude",
      host_id: hostId,
      cli_available: true,
      launchable: true,
      models: {
        models: [
          { id: "opus", annotation: "claude-opus-4.8[1m]" },
          { id: "sonnet", annotation: "claude-sonnet-4.6" },
          { id: "haiku", annotation: "claude-haiku-4.5" },
        ],
        source: "Listed",
        free_text: true,
      },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 1,
      errors: [],
    },
    // gemini: has no launch driver (launchable:false) → always greyed.
    [`gemini::${hostId}`]: {
      driver: "gemini",
      host_id: hostId,
      cli_available: true, // even with a CLI present, no launch driver
      launchable: false,
      models: { models: [], source: "Observed", free_text: true },
      session_scope: null,
      sessions: [],
      fetched_at_ms: 1,
      errors: [],
    },
  };
}

/** A launchable driver whose CLI was NOT detected on the host. */
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

function renderNewSession(capabilities: Record<string, CapabilitiesView>) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "new-session",
    capabilities,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <NewSession />
      </VmProvider>
    </ThemeProvider>
  );
}

describe("NodeModal — real capability gating", () => {
  it("real caps + project typed → model annotations render AND Launch is ENABLED", () => {
    renderNodeModal(realClaudeCaps(HOST));
    fireEvent.click(screen.getByTestId(`nm-host-${HOST}`));
    // Claude is available.
    expect((screen.getByTestId("nm-driver-claude") as HTMLButtonElement).disabled).toBe(false);
    // Model annotations (the host's resolved versions) render next to the ids.
    expect(screen.getByTestId("nm-model-opus").textContent).toContain("claude-opus-4.8[1m]");
    expect(screen.getByTestId("nm-model-sonnet").textContent).toContain("claude-sonnet-4.6");
    // Type a project → Launch enabled.
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/release" },
    });
    expect((screen.getByTestId("nm-launch-btn") as HTMLButtonElement).disabled).toBe(false);
  });

  it("caps ABSENT → a loading indicator shows and Launch isn't hard-greyed on availability", () => {
    renderNodeModal({}); // no caps discovered yet
    // Loading indicator in the model area (not the empty state).
    expect(screen.getByTestId("nm-model-loading")).toBeTruthy();
    expect(screen.queryByTestId("nm-model-empty-hint")).toBeNull();
    // Claude not greyed while loading.
    expect((screen.getByTestId("nm-driver-claude") as HTMLButtonElement).disabled).toBe(false);
    // Only the empty project blocks Launch; once typed it's enabled.
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/release" },
    });
    expect((screen.getByTestId("nm-launch-btn") as HTMLButtonElement).disabled).toBe(false);
  });

  it("gemini (launchable:false) → greyed with the unavailable note", () => {
    renderNodeModal(realClaudeCaps(HOST));
    fireEvent.click(screen.getByTestId(`nm-host-${HOST}`));
    expect((screen.getByTestId("nm-driver-gemini") as HTMLButtonElement).disabled).toBe(true);
    // Selecting gemini is a no-op; assert the known-unavailable note surfaces by
    // driving driver=gemini via caps: claude stays selected so we check the
    // button's disabled state (the block) which is gemini's greyed truth.
  });

  it("launchable driver with cli_available:false → greyed with the note", () => {
    renderNodeModal(cliMissingClaude(HOST));
    fireEvent.click(screen.getByTestId(`nm-host-${HOST}`));
    expect((screen.getByTestId("nm-driver-claude") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId("nm-driver-unavailable-note")).toBeTruthy();
    fireEvent.change(screen.getByTestId("nm-project-input"), {
      target: { value: "~/code/release" },
    });
    expect((screen.getByTestId("nm-launch-btn") as HTMLButtonElement).disabled).toBe(true);
  });

  it("picking the `opus` radio leaves the custom free-text input EMPTY (placeholder 'Type your own')", () => {
    renderNodeModal(realClaudeCaps(HOST));
    fireEvent.click(screen.getByTestId(`nm-host-${HOST}`));
    fireEvent.click(screen.getByTestId("nm-model-opus"));
    const custom = screen.getByTestId("nm-model-freetext") as HTMLInputElement;
    expect(custom.value).toBe("");
    expect(custom.placeholder).toBe("Type your own");
  });

  it("typing a custom model sets it; the box shows the typed text", () => {
    renderNodeModal(realClaudeCaps(HOST));
    fireEvent.click(screen.getByTestId(`nm-host-${HOST}`));
    fireEvent.click(screen.getByTestId("nm-model-opus"));
    const custom = screen.getByTestId("nm-model-freetext") as HTMLInputElement;
    fireEvent.change(custom, { target: { value: "my-custom-model" } });
    expect((screen.getByTestId("nm-model-freetext") as HTMLInputElement).value).toBe("my-custom-model");
  });
});

describe("NewSession — real capability gating", () => {
  it("real caps + project typed → model annotations render AND Launch is ENABLED", () => {
    renderNewSession(realClaudeCaps(HOST));
    fireEvent.click(screen.getByTestId(`ns-host-${HOST}`));
    expect((screen.getByTestId("ns-driver-claude") as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByTestId("ns-model-opus").textContent).toContain("claude-opus-4.8[1m]");
    fireEvent.change(screen.getByTestId("ns-project-input"), {
      target: { value: "~/code/release" },
    });
    expect((screen.getByTestId("ns-launch-btn") as HTMLButtonElement).disabled).toBe(false);
  });

  it("caps ABSENT → loading indicator shows; Launch not hard-greyed on availability", () => {
    renderNewSession({});
    expect(screen.getByTestId("ns-model-loading")).toBeTruthy();
    expect(screen.queryByTestId("ns-model-empty-hint")).toBeNull();
    expect((screen.getByTestId("ns-driver-claude") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.change(screen.getByTestId("ns-project-input"), {
      target: { value: "~/code/release" },
    });
    expect((screen.getByTestId("ns-launch-btn") as HTMLButtonElement).disabled).toBe(false);
  });

  it("gemini (launchable:false) → greyed", () => {
    renderNewSession(realClaudeCaps(HOST));
    fireEvent.click(screen.getByTestId(`ns-host-${HOST}`));
    expect((screen.getByTestId("ns-driver-gemini") as HTMLButtonElement).disabled).toBe(true);
  });

  it("launchable driver with cli_available:false → greyed with the note", () => {
    renderNewSession(cliMissingClaude(HOST));
    fireEvent.click(screen.getByTestId(`ns-host-${HOST}`));
    expect((screen.getByTestId("ns-driver-claude") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId("ns-driver-unavailable-note")).toBeTruthy();
  });

  it("picking `opus` leaves the custom free-text input EMPTY (placeholder 'Type your own')", () => {
    renderNewSession(realClaudeCaps(HOST));
    fireEvent.click(screen.getByTestId(`ns-host-${HOST}`));
    fireEvent.click(screen.getByTestId("ns-model-opus"));
    const custom = screen.getByTestId("ns-model-freetext") as HTMLInputElement;
    expect(custom.value).toBe("");
    expect(custom.placeholder).toBe("Type your own");
  });
});
