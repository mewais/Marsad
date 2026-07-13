/**
 * SessionDetail screen + tabs — behavior tests.
 *
 * Uses the same seeded mock-store pattern as shell.test.tsx:
 *   - inject MOCK_SNAPSHOT with selectedId set
 *   - wrap in ThemeProvider + VmProvider
 *   - assert real behaviour (header, tab switching, composer, usage warn, config)
 */
import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  afterEach,
} from "vitest";
import type { ReactNode, ContextType } from "react";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { VmProvider, VmState, VmContext } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { SessionDetail } from "../src/screens/SessionDetail";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import { MOCK_BOARD } from "../src/bridge/mockBoard";
import type { AppSnapshot } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Helper — render SessionDetail with a seeded store
// ---------------------------------------------------------------------------

// Stand-in for a real capabilities probe (dynamic-capabilities.md §U2) —
// ConfigTab's model picker reads capabilities[(driver, host)].models for the
// selected session's host, never a static import. MOCK_SNAPSHOT's sessions
// all live on "local"/"hetzner"/"gpu", so cover all three.
function testModelCapabilities(hostId: string) {
  return {
    driver: "claude",
    host_id: hostId,
    cli_available: true,
    launchable: true,
    models: {
      models: [
        { id: "claude-haiku-4-5" },
        { id: "claude-sonnet-4-6" },
        { id: "claude-opus-4-8" },
      ],
      source: "Listed" as const,
      free_text: true,
    },
    session_scope: null,
    sessions: [],
    fetched_at_ms: 0,
    errors: [],
  };
}

function renderWith(
  overrides: Partial<AppSnapshot & { selectedId: string | null }> = {}
) {
  // Build an overridden snapshot; selectedId lives on both AppSnapshot and VmState
  const { selectedId, ...snapshotOverrides } = overrides;

  const snapshot: AppSnapshot = {
    ...MOCK_SNAPSHOT,
    ...snapshotOverrides,
    selected_id: selectedId !== undefined ? selectedId : MOCK_SNAPSHOT.selected_id,
  };

  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "session",
    selectedId: selectedId !== undefined ? selectedId : null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: {
      "claude::local": testModelCapabilities("local"),
      "claude::hetzner": testModelCapabilities("hetzner"),
      "claude::gpu": testModelCapabilities("gpu"),
    },
  };

  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <SessionDetail />
      </VmProvider>
    </ThemeProvider>
  );
}

// ---------------------------------------------------------------------------
// Mock the api module so we can spy on sendPrompt / stopSession / resumeSession
// ---------------------------------------------------------------------------

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    sendPrompt: vi.fn().mockResolvedValue(undefined),
    interrupt: vi.fn().mockResolvedValue(undefined),
    runSlash: vi.fn().mockResolvedValue(undefined),
    stopSession: vi.fn().mockResolvedValue(undefined),
    resumeSession: vi.fn().mockResolvedValue(undefined),
    removeSession: vi.fn().mockResolvedValue(undefined),
    // WAVE 3b (pty escape-hatch terminal) — spied so TerminalTab's wiring to
    // the real pty commands/events is exercised; in this happy-dom test
    // environment `isTauri()` is false so the REAL implementations are also
    // no-ops, but mocking lets tests assert the call shape.
    ptyStart: vi.fn().mockResolvedValue(undefined),
    ptyInput: vi.fn().mockResolvedValue(undefined),
    ptyResize: vi.fn().mockResolvedValue(undefined),
    listenPty: vi.fn().mockResolvedValue(() => {}),
  };
});

import * as api from "../src/bridge/api";

beforeEach(() => {
  vi.clearAllMocks();
  // The `afterEach` below (`vi.restoreAllMocks()`) resets bare `vi.fn()`
  // mocks (created via the `vi.mock` factory above, not `vi.spyOn`) back to
  // a plain no-implementation mock after the FIRST test in this file runs —
  // i.e. `mockResolvedValue` stops applying from the second test onward.
  // Every other command mock here is called fire-and-forget (never
  // `.then()`-chained), so a stray `undefined` return never surfaces;
  // `TerminalTab` DOES chain `.then()` on `listenPty(...)`'s return
  // (production code correctly assumes a real `Promise<UnlistenFn>`, which
  // it always is outside tests) — re-apply the resolved values before EVERY
  // test in this file (Terminal tab mounts are reachable from more than one
  // describe block below) so the mock keeps honoring that contract.
  vi.mocked(api.listenPty).mockResolvedValue(() => {});
  vi.mocked(api.ptyStart).mockResolvedValue(undefined);
  vi.mocked(api.ptyInput).mockResolvedValue(undefined);
  vi.mocked(api.ptyResize).mockResolvedValue(undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Helpers to get mock fns (typed)
// ---------------------------------------------------------------------------

function getSendPrompt() {
  return api.sendPrompt as ReturnType<typeof vi.fn>;
}
function getStopSession() {
  return api.stopSession as ReturnType<typeof vi.fn>;
}
function getRemoveSession() {
  return api.removeSession as ReturnType<typeof vi.fn>;
}
function getPtyStart() {
  return api.ptyStart as ReturnType<typeof vi.fn>;
}

// ---------------------------------------------------------------------------
// 1. No session selected — graceful empty state
// ---------------------------------------------------------------------------

describe("SessionDetail — no session selected", () => {
  it("shows empty state when selectedId is null", () => {
    renderWith({ selectedId: null });
    expect(screen.getByText(/No session selected/i)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// 2. Header
// ---------------------------------------------------------------------------

describe("SessionDetail — header", () => {
  it("renders the session name", () => {
    // s01 — "api-auth-refactor"
    renderWith({ selectedId: "s01" });
    const heading = screen.getByTestId("session-name");
    expect(heading.textContent).toBe("api-auth-refactor");
  });

  it("renders the state badge text", () => {
    renderWith({ selectedId: "s01" });
    // s01 is "Needs" — Badge renders "Needs you"
    expect(screen.getByText("Needs you")).toBeTruthy();
  });

  it("renders the branch in the meta row", () => {
    renderWith({ selectedId: "s01" });
    // branch: "feature/auth-v2" — may appear in both header meta and activity note
    const matches = screen.getAllByText(/feature\/auth-v2/);
    expect(matches.length).toBeGreaterThan(0);
  });

  it("renders model name in meta row", () => {
    renderWith({ selectedId: "s01" });
    // model appears in header meta row and possibly in activity system note
    // (s01.model, mockFixture.ts — "claude-sonnet-latest").
    const matches = screen.getAllByText(/claude-sonnet-latest/);
    expect(matches.length).toBeGreaterThan(0);
  });

  it("renders Answer gate button when session has a gate", () => {
    // s01 has a Permission gate
    renderWith({ selectedId: "s01" });
    expect(screen.getByTestId("answer-gate-btn")).toBeTruthy();
  });

  it("does NOT render Answer gate button when no gate", () => {
    // s04 — Working, no gate
    renderWith({ selectedId: "s04" });
    expect(screen.queryByTestId("answer-gate-btn")).toBeNull();
  });

  it("renders Pause and Delete buttons for a running session", () => {
    renderWith({ selectedId: "s01" });
    expect(screen.getByTestId("pause-btn")).toBeTruthy();
    expect(screen.getByTestId("remove-btn")).toBeTruthy();
  });

  it("Pause button calls stopSession api", () => {
    renderWith({ selectedId: "s01" });
    fireEvent.click(screen.getByTestId("pause-btn"));
    expect(getStopSession()).toHaveBeenCalledWith("s01");
  });

  it("Delete button calls removeSession api after confirm", () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    renderWith({ selectedId: "s01" });
    fireEvent.click(screen.getByTestId("remove-btn"));
    expect(getRemoveSession()).toHaveBeenCalledWith("s01");
    confirmSpy.mockRestore();
  });

  // Two-axis lifecycle (Gap A): the Pause↔Resume affordance follows LIVENESS,
  // not the activity phase.
  it("shows Resume (not Pause) for a Paused session even if its phase is Working", () => {
    // Same id so selectedDetail still resolves; flipped to not-live (Paused)
    // while its activity phase stays Working — the affordance must follow live.
    const sessions = MOCK_SNAPSHOT.sessions.map((s) =>
      s.id === "s01" ? { ...s, state: "Working" as const, live: false } : s
    );
    renderWith({ sessions, selectedId: "s01" });
    expect(screen.getByTestId("resume-btn")).toBeTruthy();
    expect(screen.queryByTestId("pause-btn")).toBeNull();
  });

  it("shows Pause (not Resume) for a finished-but-alive session (Done + live)", () => {
    const sessions = MOCK_SNAPSHOT.sessions.map((s) =>
      s.id === "s01" ? { ...s, state: "Done" as const, live: true } : s
    );
    renderWith({ sessions, selectedId: "s01" });
    expect(screen.getByTestId("pause-btn")).toBeTruthy();
    expect(screen.queryByTestId("resume-btn")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// 3. Tab bar — all 5 tabs render and switching works
// ---------------------------------------------------------------------------

describe("SessionDetail — tab bar", () => {
  const TABS = ["Activity", "Changes", "Config", "Usage", "Terminal"] as const;

  TABS.forEach((tab) => {
    it(`renders the ${tab} tab button`, () => {
      renderWith({ selectedId: "s04" });
      expect(screen.getByTestId(`tab-${tab}`)).toBeTruthy();
    });
  });

  it("starts on Activity tab (default)", () => {
    renderWith({ selectedId: "s04" });
    // Activity tab renders the transcript area
    expect(screen.getByText(/Session "frontend-dashboard"/i)).toBeTruthy();
  });

  it("clicking Changes tab renders Changes content", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Changes"));
    // ChangesTab renders "Changed files" header text
    expect(screen.getByText(/Changed files/i)).toBeTruthy();
  });

  it("clicking Config tab renders Config content", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Config"));
    expect(screen.getByText(/Permission posture/i)).toBeTruthy();
  });

  it("clicking Usage tab renders Usage content", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Usage"));
    expect(screen.getByText(/Context window/i)).toBeTruthy();
  });

  it("clicking Terminal tab renders Terminal content", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Terminal"));
    // The Terminal tab mounts the xterm output surface (the old "Escape hatch"
    // header was removed).
    expect(screen.getByTestId("terminal-output")).toBeTruthy();
  });

  it("Changes tab shows file count badge when changed_files > 0", () => {
    // s04 has changed_files: 12
    renderWith({ selectedId: "s04" });
    const changesTabBtn = screen.getByTestId("tab-Changes");
    expect(changesTabBtn.textContent).toContain("12");
  });
});

// ---------------------------------------------------------------------------
// 4. Activity tab — composer sends prompt + calls sendPrompt
// ---------------------------------------------------------------------------

describe("ActivityTab — composer", () => {
  it("typing a message and pressing Enter calls sendPrompt", () => {
    renderWith({ selectedId: "s04" });

    // The composer textarea has a placeholder text
    const textarea = screen.getByPlaceholderText(
      /Send a prompt/i
    );
    fireEvent.change(textarea, { target: { value: "Hello Claude" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });

    expect(getSendPrompt()).toHaveBeenCalledWith("s04", "Hello Claude");
  });

  it("clicking Send button also calls sendPrompt", () => {
    renderWith({ selectedId: "s04" });
    const textarea = screen.getByPlaceholderText(
      /Send a prompt/i
    );
    fireEvent.change(textarea, { target: { value: "Test message" } });

    const sendBtn = screen.getByText("Send ↵");
    fireEvent.click(sendBtn);

    expect(getSendPrompt()).toHaveBeenCalledWith("s04", "Test message");
  });

  it("empty message does NOT call sendPrompt", () => {
    renderWith({ selectedId: "s04" });
    const textarea = screen.getByPlaceholderText(
      /Send a prompt/i
    );
    fireEvent.change(textarea, { target: { value: "" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });
    expect(getSendPrompt()).not.toHaveBeenCalled();
  });

  it("Shift+Enter does NOT submit", () => {
    renderWith({ selectedId: "s04" });
    const textarea = screen.getByPlaceholderText(
      /Send a prompt/i
    );
    fireEvent.change(textarea, { target: { value: "No submit" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: true });
    expect(getSendPrompt()).not.toHaveBeenCalled();
  });

  it("interrupt button calls interrupt api", async () => {
    renderWith({ selectedId: "s04" });
    const interruptBtn = screen.getByTitle("Interrupt");
    fireEvent.click(interruptBtn);
    expect(api.interrupt).toHaveBeenCalledWith("s04");
  });
});

// ---------------------------------------------------------------------------
// 5. Usage tab — context bar warning at ≥75%
// ---------------------------------------------------------------------------

describe("UsageTab — context warning", () => {
  it("shows amber warning banner when context_pct >= 75", () => {
    // Patch s04 to have context_pct = 80
    const sessions = MOCK_SNAPSHOT.sessions.map((s) =>
      s.id === "s04" ? { ...s, context_pct: 80 } : s
    );
    renderWith({ selectedId: "s04", sessions });
    fireEvent.click(screen.getByTestId("tab-Usage"));

    const banner = screen.getByTestId("ctx-warn-banner");
    expect(banner).toBeTruthy();
    expect(banner.textContent).toContain("80% full");
  });

  it("does NOT show warning banner when context_pct < 75", () => {
    // s04 normally has 55% — below threshold
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Usage"));
    expect(screen.queryByTestId("ctx-warn-banner")).toBeNull();
  });

  it("renders tokens, cost, elapsed tiles", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Usage"));
    expect(screen.getByTestId("tokens-value")).toBeTruthy();
    expect(screen.getByTestId("cost-value")).toBeTruthy();
    expect(screen.getByTestId("elapsed-value")).toBeTruthy();
  });

  it("tokens tile shows formatted value from selectedDetail", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Usage"));
    // s04 has tokens: 74000 → "74.0k"
    const tile = screen.getByTestId("tokens-value");
    expect(tile.textContent).toContain("74.0k");
  });

  it("cost tile shows cost_label from selectedDetail", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Usage"));
    // s04 cost_usd: 0.89 → "$0.89"
    const tile = screen.getByTestId("cost-value");
    expect(tile.textContent).toContain("$0.89");
  });
});

// ---------------------------------------------------------------------------
// 6. Config tab — model buttons and posture radios render
// ---------------------------------------------------------------------------

describe("ConfigTab — model and posture", () => {
  it("renders Haiku, Sonnet, Opus model buttons", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Config"));

    expect(screen.getByTestId("model-btn-claude-haiku-4-5")).toBeTruthy();
    expect(screen.getByTestId("model-btn-claude-sonnet-4-6")).toBeTruthy();
    expect(screen.getByTestId("model-btn-claude-opus-4-8")).toBeTruthy();
  });

  it("renders all three posture options", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Config"));

    expect(screen.getByTestId("posture-ask")).toBeTruthy();
    expect(screen.getByTestId("posture-auto")).toBeTruthy();
    expect(screen.getByTestId("posture-full")).toBeTruthy();
  });

  it("clicking a model button changes selection (no api call yet)", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Config"));

    const opusBtn = screen.getByTestId("model-btn-claude-opus-4-8");
    fireEvent.click(opusBtn);
    // Visual change — border/bg change to active style; no api call expected yet
    expect(opusBtn.style.background).toBeTruthy();
  });

  it("renders Apply and Save-as-template buttons", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Config"));

    expect(screen.getByTestId("config-apply")).toBeTruthy();
    expect(screen.getByTestId("config-save-template")).toBeTruthy();
  });

  it("renders tool chips from live session tools", () => {
    // s01 has representative tools: ["Read", "Bash", "Edit", "Write"]
    renderWith({ selectedId: "s01" });
    fireEvent.click(screen.getByTestId("tab-Config"));

    // Real tool chips are rendered per tool in the session's tools array.
    expect(screen.getByTestId("tool-chip-Read")).toBeTruthy();
    expect(screen.getByTestId("tool-chip-Bash")).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// 7. Changes tab — file list and commit bar render
// ---------------------------------------------------------------------------

describe("ChangesTab", () => {
  it("renders file list with Changed files header", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Changes"));
    expect(screen.getByText(/Changed files/i)).toBeTruthy();
  });

  it("renders commit bar with Commit button", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Changes"));
    expect(screen.getByText(/^Commit$/)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// 8. Terminal tab — real xterm mounts + pty wiring
// ---------------------------------------------------------------------------

describe("TerminalTab", () => {
  it("mounts a REAL xterm.js terminal (WAVE 3b — no more synthetic lines)", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Terminal"));
    const container = screen.getByTestId("terminal-output");
    // xterm.js stamps its own `.xterm` root class into the mount point —
    // proves a real Terminal instance opened here, not the old stub <div>s.
    expect(container.querySelector(".xterm")).toBeTruthy();
  });

  it("starts the session's pty on mount", async () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Terminal"));
    // pty_start fires inside a requestAnimationFrame (multi-terminal v2), so let
    // it flush. It now carries the per-session terminal_id:
    // (sessionId, terminalId, cols, rows).
    await waitFor(() =>
      expect(getPtyStart()).toHaveBeenCalledWith(
        "s04",
        expect.any(String),
        expect.any(Number),
        expect.any(Number)
      )
    );
  });

  it("browser/mock mode renders an honestly idle terminal — no fake output", () => {
    // `ptyStart`/`listenPty` are mocked no-ops here (as they are for real in
    // browser/mock mode, `isTauri()` false) — the terminal must show nothing
    // synthetic, unlike the old stub which fabricated lines from SessionView.
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Terminal"));
    const container = screen.getByTestId("terminal-output");
    expect(container.textContent).not.toMatch(/Claude is working|Session complete/i);
  });

  it("switching away from the Terminal tab disposes the terminal without throwing", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Terminal"));
    expect(() =>
      fireEvent.click(screen.getByTestId("tab-Activity"))
    ).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// 9. "‹ Back" button (Workstreams redesign SLICE D) — label + nav target
//    depend on `state.fromGraph`, per docs/design/DESIGN_HANDOFF.md
//    §"LEVEL 3 — Session".
// ---------------------------------------------------------------------------

/** Renders <SessionDetail/> with full nav state control (board/wsId/fromGraph),
 *  capturing the live useVm() context so tests can assert on navigation. */
function renderWithNav(overrides: Partial<VmState>, onCtx: (ctx: NonNullable<ContextType<typeof VmContext>>) => void) {
  function Capture({ children }: { children: ReactNode }) {
    return (
      <VmContext.Consumer>
        {(ctx) => {
          if (ctx) onCtx(ctx);
          return <>{children}</>;
        }}
      </VmContext.Consumer>
    );
  }
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "session",
    selectedId: "s01",
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: {},
    board: MOCK_BOARD,
    ...overrides,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <Capture>
          <SessionDetail />
        </Capture>
      </VmProvider>
    </ThemeProvider>
  );
}

describe("SessionDetail — Back button", () => {
  it('labels the button "Workstreams" and routes to the board when fromGraph is false', () => {
    let lastScreen = "";
    renderWithNav({ fromGraph: false, wsId: "ws-build-pipeline" }, (ctx) => {
      lastScreen = ctx.state.screen;
    });
    const btn = screen.getByTestId("session-back-btn");
    expect(btn.textContent).toContain("Workstreams");

    fireEvent.click(btn);
    expect(lastScreen).toBe("board");
  });

  it("labels the button with the parent workstream's name when fromGraph is true", () => {
    renderWithNav({ fromGraph: true, wsId: "ws-build-pipeline" }, () => {});
    const btn = screen.getByTestId("session-back-btn");
    // MOCK_BOARD's "ws-build-pipeline" card name.
    expect(btn.textContent).toContain("Build pipeline handoff");
  });

  it("routes back to the graph (not the board) when fromGraph is true", async () => {
    let lastScreen = "";
    let lastWsId: string | null = null;
    renderWithNav({ fromGraph: true, wsId: "ws-build-pipeline" }, (ctx) => {
      lastScreen = ctx.state.screen;
      lastWsId = ctx.state.wsId;
    });
    fireEvent.click(screen.getByTestId("session-back-btn"));
    // goToGraph is async (fetchGraph then two dispatches) — wait for it to land.
    await waitFor(() => expect(lastScreen).toBe("graph"));
    expect(lastWsId).toBe("ws-build-pipeline");
  });

  it('falls back to "Workstreams" when fromGraph is true but no wsId is known', () => {
    renderWithNav({ fromGraph: true, wsId: null }, () => {});
    expect(screen.getByTestId("session-back-btn").textContent).toContain("Workstreams");
  });
});

// ---------------------------------------------------------------------------
// 10. Header meta row — driver pill (Workstreams redesign SLICE D)
// ---------------------------------------------------------------------------

describe("SessionDetail — header driver pill", () => {
  it("shows the driver reported by capabilities for the session's host", () => {
    // renderWith() seeds capabilities with driver: "claude" for local/hetzner/gpu.
    renderWith({ selectedId: "s01" });
    const pill = screen.getByTestId("session-driver-pill");
    expect(pill.textContent).toBe("claude");
  });
});
