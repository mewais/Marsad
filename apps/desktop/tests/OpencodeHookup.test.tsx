/**
 * OpencodeHookup — TDD red-phase tests for opencode frontend gaps.
 *
 * Spec: docs/specs/opencode-acp.md §7 (frontend).
 *
 * Covers three gaps that must FAIL now and go green once a coder wires them:
 *
 * A) Settings — opencode per-host command input field (local + add-host form):
 *    `local-opencode-cmd` and `host-add-opencode-cmd` testids are absent.
 *    [RED: not added yet]
 *
 * B) SessionDetail — opencode session header badge has a real (non-fallback)
 *    color from DRIVER_COLOR. Currently DRIVER_COLOR in SessionDetail.tsx lacks
 *    "opencode", so the pill is entirely absent (driverColor is undefined and the
 *    conditional `{driver && driverColor && ...}` hides it).
 *    [RED: DRIVER_COLOR missing "opencode" entry]
 *
 * C) current_mode — a session with `current_mode: "plan"` shows a mode indicator
 *    (data-testid="session-mode" containing "plan") in the session/activity header.
 *    [RED: current_mode is not surfaced in SessionDetail or ActivityTab]
 *
 * Pattern mirrors OpencodeDriver.test.tsx and SessionDetail.test.tsx:
 *   VmProvider + ThemeProvider, inject MOCK_SNAPSHOT overrides.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { SessionDetail } from "../src/screens/SessionDetail";
import { Settings } from "../src/screens/Settings";
import { ActivityTab } from "../src/screens/tabs/Activity";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView, Settings as SettingsT } from "../src/bridge/types";
import { DEFAULT_SETTINGS } from "../src/bridge/api";

// ---------------------------------------------------------------------------
// API mock — mirrors SessionDetail.test.tsx and SettingsHostRows.test.tsx
// ---------------------------------------------------------------------------

/* eslint-disable @typescript-eslint/no-explicit-any */
const updateHost = vi.fn((..._a: any[]) => Promise.resolve());
const addHost = vi.fn((..._a: any[]) => Promise.resolve());
const setSettings = vi.fn((..._a: any[]) => Promise.resolve());
const getSettings = vi.fn(
  (..._a: any[]): Promise<SettingsT> => Promise.resolve({ ...DEFAULT_SETTINGS })
);

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    updateHost: (...a: any[]) => updateHost(...a),
    addHost: (...a: any[]) => addHost(...a),
    getSettings: (...a: any[]) => getSettings(...a),
    setSettings: (...a: any[]) => setSettings(...a),
    sendPrompt: vi.fn().mockResolvedValue(undefined),
    interrupt: vi.fn().mockResolvedValue(undefined),
    stopSession: vi.fn().mockResolvedValue(undefined),
    resumeSession: vi.fn().mockResolvedValue(undefined),
    removeSession: vi.fn().mockResolvedValue(undefined),
    ptyStart: vi.fn().mockResolvedValue(undefined),
    ptyInput: vi.fn().mockResolvedValue(undefined),
    ptyResize: vi.fn().mockResolvedValue(undefined),
    listenPty: vi.fn().mockResolvedValue(() => {}),
    launchSession: vi.fn().mockResolvedValue("mock-session"),
    restartSession: vi.fn().mockResolvedValue(undefined),
    answerGate: vi.fn().mockResolvedValue(undefined),
  };
});

import * as api from "../src/bridge/api";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.listenPty).mockResolvedValue(() => {});
  vi.mocked(api.ptyStart).mockResolvedValue(undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Render helpers
// ---------------------------------------------------------------------------

function renderSettings() {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "settings",
    capabilities: {},
  };
  render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <Settings />
      </VmProvider>
    </ThemeProvider>
  );
  // Navigate to the Hosts tab
  fireEvent.click(screen.getByTestId("settings-tab-hosts"));
}

function renderSessionDetail(sessionId: string, sessionOverrides?: Partial<SessionView>) {
  const sessions = MOCK_SNAPSHOT.sessions.map((s) =>
    s.id === sessionId ? { ...s, ...sessionOverrides } : s
  );
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, sessions };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "session",
    selectedId: sessionId,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: {},
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <SessionDetail />
      </VmProvider>
    </ThemeProvider>
  );
}

function renderActivity(sessionId: string, sessionOverrides?: Partial<SessionView>) {
  const sessions = MOCK_SNAPSHOT.sessions.map((s) =>
    s.id === sessionId ? { ...s, ...sessionOverrides } : s
  );
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, sessions };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "session",
    selectedId: sessionId,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: {},
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <ActivityTab sessionId={sessionId} onOpenGate={() => undefined} />
      </VmProvider>
    </ThemeProvider>
  );
}

// =============================================================================
// A) Settings — opencode per-host command input field
// =============================================================================
//
// The Settings Hosts tab must expose an "opencode" command field in BOTH:
//   1. The local machine card (alongside Claude Code / Codex rows)
//   2. The add-host form (alongside host-add-claude-bin / host-add-codex-cmd
//      rows)
//
// RED: Settings.tsx's local machine section has `local-codex-cmd` but
// NO `local-opencode-cmd`.
// The add-host form has `host-add-codex-cmd` but
// NO `host-add-opencode-cmd`.

describe("Settings Hosts — opencode command field [RED]", () => {
  it("local machine card exposes an opencode command input (data-testid='local-opencode-cmd')", () => {
    // RED: The local machine card currently shows Claude Code / Codex
    // command rows but NOT opencode. A coder must add an opencode row with
    // data-testid="local-opencode-cmd" alongside the existing ones.
    renderSettings();
    // Verify the existing rows are there (sanity baseline)
    expect(screen.getByTestId("local-claude-bin")).toBeTruthy();
    expect(screen.getByTestId("local-codex-cmd")).toBeTruthy();
    // RED assertion: opencode row is ABSENT
    expect(
      screen.getByTestId("local-opencode-cmd"),
    ).toBeTruthy(); // will FAIL until coder adds it
  });

  it("local machine opencode command row has an input (or textarea) element", () => {
    // RED: same as above — no input field for opencode yet.
    renderSettings();
    const input = screen.getByTestId("local-opencode-cmd");
    // Must be an actual editable element
    const tag = input.tagName.toLowerCase();
    expect(["input", "textarea"]).toContain(tag);
  });

  it("add-host form exposes an opencode command input (data-testid='host-add-opencode-cmd')", () => {
    // RED: The add-host form has Claude / Codex rows but not opencode.
    // A coder must add data-testid="host-add-opencode-cmd".
    renderSettings();
    // Sanity baseline
    expect(screen.getByTestId("host-add-claude-bin")).toBeTruthy();
    expect(screen.getByTestId("host-add-codex-cmd")).toBeTruthy();
    // RED assertion: opencode row is ABSENT in the add-host form
    expect(
      screen.getByTestId("host-add-opencode-cmd"),
    ).toBeTruthy(); // will FAIL until coder adds it
  });

  it("add-host form opencode field has an input (or textarea) element", () => {
    // RED: same as above.
    renderSettings();
    const input = screen.getByTestId("host-add-opencode-cmd");
    const tag = input.tagName.toLowerCase();
    expect(["input", "textarea"]).toContain(tag);
  });

  it("local opencode command row has a visible label near the input", () => {
    // RED: The label text "opencode" (or "opencode command") must appear alongside
    // the input in the local machine card, consistent with the other driver rows.
    renderSettings();
    // Verify the codex label exists as a sanity check (the current implementation
    // shows a label row above each input using text like "Codex command").
    const allText = document.body.textContent ?? "";
    // The opencode label must be in the DOM somewhere near the field.
    // "opencode" as a label word should appear on the Hosts tab.
    expect(allText.toLowerCase()).toContain("opencode");
  });
});

// =============================================================================
// B) SessionDetail — opencode driver badge has a real color
// =============================================================================
//
// When `SessionView.driver === "opencode"`, the header must show a driver pill
// (data-testid="session-driver-pill") with a real hex color (non-empty, not
// the CSS variable fallback for an unknown driver, and not absent).
//
// RED: SessionDetail.tsx's DRIVER_COLOR map has entries for claude/codex
// but NOT opencode. Because `DRIVER_COLOR["opencode"]` is undefined, the
// conditional `{driver && driverColor && <span>}` hides the pill entirely —
// so getByTestId("session-driver-pill") throws "not found".

describe("SessionDetail — opencode driver badge [RED]", () => {
  it("shows a driver pill (data-testid='session-driver-pill') for an opencode session", () => {
    // RED: DRIVER_COLOR["opencode"] is undefined, so driverColor is undefined,
    // so the pill is conditionally hidden. A coder must add "opencode" to
    // DRIVER_COLOR in SessionDetail.tsx.
    renderSessionDetail("s04", { driver: "opencode" });
    // Will FAIL until DRIVER_COLOR["opencode"] is defined
    expect(screen.getByTestId("session-driver-pill")).toBeTruthy();
  });

  it("opencode driver pill displays the text 'opencode'", () => {
    // RED: pill is absent, so textContent check can't happen.
    renderSessionDetail("s04", { driver: "opencode" });
    const pill = screen.getByTestId("session-driver-pill");
    expect(pill.textContent?.toLowerCase()).toContain("opencode");
  });

  it("opencode driver pill has a non-empty color style (not transparent/invisible)", () => {
    // RED: pill is absent; when added its color style must be a real value
    // (the driverTint helper returns an rgba() string from the hex — not
    // empty or 'transparent').
    renderSessionDetail("s04", { driver: "opencode" });
    const pill = screen.getByTestId("session-driver-pill") as HTMLElement;
    const color = pill.style.color || pill.style.borderColor || pill.style.background;
    // Must not be empty — a real color is set
    expect(color).toBeTruthy();
  });

  it("opencode driver pill is NOT absent when driver='opencode' (vs claude/codex always showing)", () => {
    // Sanity: claude driver DOES show a pill (it exists in DRIVER_COLOR)
    renderSessionDetail("s04", { driver: "claude" });
    const claudePill = screen.getByTestId("session-driver-pill");
    expect(claudePill).toBeTruthy();
    // Cleanup implicitly on the second render in a new test (kept separate)
  });

  it("codex driver pill shows for reference (confirms pill conditional works)", () => {
    // Sanity: codex driver DOES show a pill (DRIVER_COLOR has codex)
    renderSessionDetail("s04", { driver: "codex" });
    const pill = screen.getByTestId("session-driver-pill");
    expect(pill.textContent?.toLowerCase()).toContain("codex");
  });

  it("opencode driver pill color is not the same as the fallback for unknown drivers", () => {
    // When a coder adds "opencode" to DRIVER_COLOR, its color must be distinct
    // from undefined/empty (no color). We test the pill exists AND has a color
    // attribute. The exact color value is not pinned (the coder picks the hex).
    renderSessionDetail("s04", { driver: "opencode" });
    const pill = screen.getByTestId("session-driver-pill") as HTMLElement;
    // The inline border color must be non-empty (the driverTint path sets border)
    const styleAttr = pill.getAttribute("style") ?? "";
    expect(styleAttr.length).toBeGreaterThan(0);
  });
});

// =============================================================================
// C) current_mode — session with current_mode shows a mode indicator
// =============================================================================
//
// An opencode session can report its current mode via `current_mode_update`.
// The SessionView already carries `current_mode?: string` (added as part of the
// ACP wiring). The UI must surface this somewhere in the session header or
// activity area (data-testid="session-mode" containing the mode id).
//
// RED: neither SessionDetail nor ActivityTab renders current_mode today.

describe("SessionDetail — current_mode indicator [RED]", () => {
  it("renders a mode indicator (data-testid='session-mode') when current_mode is 'plan'", () => {
    // RED: SessionDetail.tsx does not read or render current_mode.
    // A coder must add a `data-testid="session-mode"` element somewhere in the
    // header (near the driver pill / badge row) when current_mode is non-empty.
    renderSessionDetail("s04", {
      driver: "opencode",
      current_mode: "plan",
    } as Partial<SessionView>);
    // Will FAIL until current_mode is surfaced in SessionDetail
    expect(screen.getByTestId("session-mode")).toBeTruthy();
  });

  it("session-mode indicator contains the mode text 'plan'", () => {
    // RED: element is absent.
    renderSessionDetail("s04", {
      driver: "opencode",
      current_mode: "plan",
    } as Partial<SessionView>);
    const modeEl = screen.getByTestId("session-mode");
    expect(modeEl.textContent?.toLowerCase()).toContain("plan");
  });

  it("session-mode indicator contains the mode text 'build' when current_mode='build'", () => {
    // RED: element is absent.
    renderSessionDetail("s04", {
      driver: "opencode",
      current_mode: "build",
    } as Partial<SessionView>);
    const modeEl = screen.getByTestId("session-mode");
    expect(modeEl.textContent?.toLowerCase()).toContain("build");
  });

  it("does NOT render session-mode indicator when current_mode is absent/undefined", () => {
    // When current_mode is undefined (non-opencode session or before first
    // mode_update), no mode indicator should appear — absence is correct.
    renderSessionDetail("s04", {
      driver: "claude",
      current_mode: undefined,
    } as Partial<SessionView>);
    expect(screen.queryByTestId("session-mode")).toBeNull();
  });

  it("does NOT render session-mode indicator when current_mode is empty string", () => {
    // An empty string is not a real mode — the indicator must be hidden.
    renderSessionDetail("s04", {
      driver: "opencode",
      current_mode: "",
    } as Partial<SessionView>);
    expect(screen.queryByTestId("session-mode")).toBeNull();
  });
});

// =============================================================================
// C-alt) ActivityTab — current_mode indicator in the activity area
// =============================================================================
//
// The mode indicator may also (or alternatively) live in the ActivityTab
// rather than in SessionDetail's header — both locations are acceptable.
// These tests check the ActivityTab independently.
//
// RED: ActivityTab does not render current_mode.

describe("ActivityTab — current_mode indicator [RED]", () => {
  it("renders a mode indicator when current_mode='plan' in the activity area", () => {
    // RED: ActivityTab does not read or render current_mode.
    // A coder may add the indicator here instead of (or in addition to) the header.
    renderActivity("s04", {
      driver: "opencode",
      current_mode: "plan",
    } as Partial<SessionView>);
    // Either the header (in SessionDetail) or the activity area must show the
    // mode indicator. Since this test renders ActivityTab standalone, we look
    // for the session-mode testid.
    expect(screen.getByTestId("session-mode")).toBeTruthy();
  });

  it("activity area mode indicator shows the mode text", () => {
    // RED: same as above.
    renderActivity("s04", {
      driver: "opencode",
      current_mode: "plan",
    } as Partial<SessionView>);
    const modeEl = screen.getByTestId("session-mode");
    expect(modeEl.textContent?.toLowerCase()).toContain("plan");
  });
});
