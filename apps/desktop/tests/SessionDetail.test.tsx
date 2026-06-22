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
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { SessionDetail } from "../src/screens/SessionDetail";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Helper — render SessionDetail with a seeded store
// ---------------------------------------------------------------------------

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
  };
});

import * as api from "../src/bridge/api";

beforeEach(() => {
  vi.clearAllMocks();
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
function getResumeSession() {
  return api.resumeSession as ReturnType<typeof vi.fn>;
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
    const matches = screen.getAllByText(/claude-sonnet-4-6/);
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

  it("renders Stop and Restart buttons", () => {
    renderWith({ selectedId: "s01" });
    expect(screen.getByTestId("stop-btn")).toBeTruthy();
    expect(screen.getByTestId("restart-btn")).toBeTruthy();
  });

  it("Stop button calls stopSession api", () => {
    renderWith({ selectedId: "s01" });
    fireEvent.click(screen.getByTestId("stop-btn"));
    expect(getStopSession()).toHaveBeenCalledWith("s01");
  });

  it("Restart button calls resumeSession api", () => {
    renderWith({ selectedId: "s01" });
    fireEvent.click(screen.getByTestId("restart-btn"));
    expect(getResumeSession()).toHaveBeenCalledWith("s01");
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
    expect(screen.getByText(/Escape hatch/i)).toBeTruthy();
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
      /Send a prompt, or \/ for commands/i
    );
    fireEvent.change(textarea, { target: { value: "Hello Claude" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });

    expect(getSendPrompt()).toHaveBeenCalledWith("s04", "Hello Claude");
  });

  it("clicking Send button also calls sendPrompt", () => {
    renderWith({ selectedId: "s04" });
    const textarea = screen.getByPlaceholderText(
      /Send a prompt, or \/ for commands/i
    );
    fireEvent.change(textarea, { target: { value: "Test message" } });

    const sendBtn = screen.getByText("Send ↵");
    fireEvent.click(sendBtn);

    expect(getSendPrompt()).toHaveBeenCalledWith("s04", "Test message");
  });

  it("empty message does NOT call sendPrompt", () => {
    renderWith({ selectedId: "s04" });
    const textarea = screen.getByPlaceholderText(
      /Send a prompt, or \/ for commands/i
    );
    fireEvent.change(textarea, { target: { value: "" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });
    expect(getSendPrompt()).not.toHaveBeenCalled();
  });

  it("Shift+Enter does NOT submit", () => {
    renderWith({ selectedId: "s04" });
    const textarea = screen.getByPlaceholderText(
      /Send a prompt, or \/ for commands/i
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

  it("renders tool chips", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Config"));

    expect(screen.getByTestId("tool-chip-bash")).toBeTruthy();
    expect(screen.getByTestId("tool-chip-git")).toBeTruthy();
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
    expect(screen.getByText(/Commit ✓/)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// 8. Terminal tab — escape hatch renders
// ---------------------------------------------------------------------------

describe("TerminalTab", () => {
  it("renders escape hatch header text", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Terminal"));
    expect(screen.getByText(/Escape hatch/i)).toBeTruthy();
  });

  it("renders terminal input", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Terminal"));
    expect(screen.getByTestId("terminal-input")).toBeTruthy();
  });

  it("typing a command + Enter calls sendPrompt", () => {
    renderWith({ selectedId: "s04" });
    fireEvent.click(screen.getByTestId("tab-Terminal"));

    const input = screen.getByTestId("terminal-input");
    fireEvent.change(input, { target: { value: "git status" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(getSendPrompt()).toHaveBeenCalledWith("s04", "git status");
  });
});
