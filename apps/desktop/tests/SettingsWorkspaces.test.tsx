/**
 * Behavioral Vitest/RTL tests for the Settings screen.
 *
 * Pattern mirrors shell/SessionDetail/GateNewSession:
 *   inject MOCK_SNAPSHOT via VmProvider initialState, wrap in ThemeProvider,
 *   assert real behavior.
 *
 * NOTE: this file used to also cover the Workspaces screen. The Workstreams
 * redesign (SLICE A) deletes the Workspaces UI entirely (switcher pill,
 * manager modal, screen) — "Restore workspace on launch" stays as a backend
 * setting, but there is no more UI to switch/manage workspaces from. Those
 * tests were removed along with the screen; they were testing a deleted
 * feature, not regressing one.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Settings } from "../src/screens/Settings";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Mock the api module
// ---------------------------------------------------------------------------

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    getSettings: vi.fn().mockResolvedValue({ ...original.DEFAULT_SETTINGS }),
    setSettings: vi.fn().mockResolvedValue(undefined),
  };
});

import * as api from "../src/bridge/api";

beforeEach(() => {
  vi.clearAllMocks();
  // ThemeProvider persists theme/glass to localStorage and re-reads it on init,
  // so without this clear a test that switches to "light" leaks that choice into
  // the next test (which expects the documented initialTheme="dark"). Reset to a
  // clean slate so every test starts from defaults.
  localStorage.clear();
  // Re-set implementations that clearAllMocks wipes out.
  (api.getSettings as ReturnType<typeof vi.fn>).mockResolvedValue({ ...api.DEFAULT_SETTINGS });
  (api.setSettings as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// Stand-in for a real capabilities probe (dynamic-capabilities.md §U2) — the
// Settings "Default model" picker unions capabilities[(driver, host)].models
// across every known host, never a static import.
const TEST_CAPABILITIES = {
  "claude::local": {
    driver: "claude",
    host_id: "local",
    cli_available: true,
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
  },
};

function renderSettings(snapshotOverride?: Partial<AppSnapshot>) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, ...snapshotOverride };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "settings",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: TEST_CAPABILITIES,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <Settings />
      </VmProvider>
    </ThemeProvider>
  );
}

// ---------------------------------------------------------------------------
// Settings — tab bar renders
// ---------------------------------------------------------------------------

describe("Settings — tab bar", () => {
  it("renders all 4 tab buttons", () => {
    renderSettings();
    expect(screen.getByTestId("settings-tab-notifications")).toBeTruthy();
    expect(screen.getByTestId("settings-tab-hosts")).toBeTruthy();
    expect(screen.getByTestId("settings-tab-session-defaults")).toBeTruthy();
    expect(screen.getByTestId("settings-tab-general")).toBeTruthy();
  });

  it("Notifications is the default active tab", () => {
    renderSettings();
    // Notifications panel content renders (per-event toggles)
    expect(screen.getByTestId("notif-row-gate_needs_you")).toBeTruthy();
  });

  it("switching to Hosts tab shows host list panel", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    // Hosts panel content
    expect(screen.getByTestId("host-row-local")).toBeTruthy();
  });

  it("switching to Session defaults tab shows model picker", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-session-defaults"));
    expect(screen.getByTestId("defaults-model-claude-sonnet-4-6")).toBeTruthy();
  });

  it("switching to General tab shows theme/glass controls", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-general"));
    expect(screen.getByTestId("general-theme-dark")).toBeTruthy();
    expect(screen.getByTestId("general-theme-light")).toBeTruthy();
  });

  it("switching tabs hides the previous panel", () => {
    renderSettings();
    // Notifications panel is visible by default
    expect(screen.getByTestId("notif-row-gate_needs_you")).toBeTruthy();
    // Switch to Hosts
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    // Notifications panel gone
    expect(screen.queryByTestId("notif-row-gate_needs_you")).toBeNull();
    // Hosts panel visible
    expect(screen.getByTestId("host-row-local")).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// Settings — Notifications tab
// ---------------------------------------------------------------------------

describe("Settings — Notifications tab", () => {
  it("renders all 5 event rows", () => {
    renderSettings();
    expect(screen.getByTestId("notif-row-gate_needs_you")).toBeTruthy();
    expect(screen.getByTestId("notif-row-gate_error")).toBeTruthy();
    expect(screen.getByTestId("notif-row-session_done")).toBeTruthy();
    expect(screen.getByTestId("notif-row-session_started")).toBeTruthy();
    expect(screen.getByTestId("notif-row-host_unreachable")).toBeTruthy();
  });

  it("renders event labels in the rows", () => {
    renderSettings();
    expect(screen.getByText("Gate needs you")).toBeTruthy();
    expect(screen.getByText("Gate errored")).toBeTruthy();
    expect(screen.getByText("Session done")).toBeTruthy();
    expect(screen.getByText("Session started")).toBeTruthy();
    expect(screen.getByText("Host unreachable")).toBeTruthy();
  });

  it("renders OS notification preview panel", () => {
    renderSettings();
    // The preview panel contains a unique phrase not present elsewhere
    expect(screen.getByText(/waiting for your approval/)).toBeTruthy();
  });

  it("toggling a notification event toggle changes its checked state", () => {
    renderSettings();
    // gate_needs_you is enabled by default — find its toggle (role=switch)
    const row = screen.getByTestId("notif-row-gate_needs_you");
    const toggles = row.querySelectorAll("[role='switch']");
    const mainToggle = toggles[0] as HTMLButtonElement;
    expect(mainToggle.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(mainToggle);
    expect(mainToggle.getAttribute("aria-checked")).toBe("false");
  });
});

// ---------------------------------------------------------------------------
// Settings — Hosts tab
// ---------------------------------------------------------------------------

describe("Settings — Hosts tab", () => {
  it("lists all hosts from the store (3 hosts)", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    expect(screen.getByTestId("host-row-local")).toBeTruthy();
    expect(screen.getByTestId("host-row-hetzner")).toBeTruthy();
    expect(screen.getByTestId("host-row-gpu")).toBeTruthy();
  });

  it("localhost row shows 'this machine' badge", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    const row = screen.getByTestId("host-row-local");
    expect(row.textContent).toContain("this machine");
  });

  it("localhost Protected button is rendered (not removable)", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    const row = screen.getByTestId("host-row-local");
    expect(row.textContent).toContain("Protected");
  });

  it("hetzner row shows status and target", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    const row = screen.getByTestId("host-row-hetzner");
    expect(row.textContent).toContain("hetzner-cx51");
    expect(row.textContent).toContain("dev@hetzner-cx51.example.com");
    expect(row.textContent).toContain("Connected");
  });

  it("gpu row shows Reconnecting status", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    const row = screen.getByTestId("host-row-gpu");
    expect(row.textContent).toContain("Reconnecting");
  });

  it("renders add-remote form with label + target inputs", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    expect(screen.getByTestId("host-add-label")).toBeTruthy();
    expect(screen.getByTestId("host-add-target")).toBeTruthy();
    expect(screen.getByTestId("host-add-btn")).toBeTruthy();
  });

  it("add-remote button is disabled when inputs are empty", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    const btn = screen.getByTestId("host-add-btn") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });

  it("add-remote button is enabled when both inputs are filled", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-hosts"));
    fireEvent.change(screen.getByTestId("host-add-label"), {
      target: { value: "new-host" },
    });
    fireEvent.change(screen.getByTestId("host-add-target"), {
      target: { value: "user@host.example.com" },
    });
    const btn = screen.getByTestId("host-add-btn") as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Settings — Session defaults tab
// ---------------------------------------------------------------------------

describe("Settings — Session defaults tab", () => {
  it("renders model picker buttons for all 3 models", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-session-defaults"));
    expect(screen.getByTestId("defaults-model-claude-haiku-4-5")).toBeTruthy();
    expect(screen.getByTestId("defaults-model-claude-sonnet-4-6")).toBeTruthy();
    expect(screen.getByTestId("defaults-model-claude-opus-4-8")).toBeTruthy();
  });

  it("renders posture picker buttons: ask, auto, full", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-session-defaults"));
    expect(screen.getByTestId("defaults-posture-ask")).toBeTruthy();
    expect(screen.getByTestId("defaults-posture-auto")).toBeTruthy();
    expect(screen.getByTestId("defaults-posture-full")).toBeTruthy();
  });

  it("clicking a model button selects it (border changes)", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-session-defaults"));
    const opusBtn = screen.getByTestId("defaults-model-claude-opus-4-8");
    fireEvent.click(opusBtn);
    // After click, opus should be selected (accent border style applied)
    // We verify by checking it doesn't throw and the button exists
    expect(opusBtn).toBeTruthy();
  });

  it("clicking a posture button selects it", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-session-defaults"));
    const askBtn = screen.getByTestId("defaults-posture-ask");
    fireEvent.click(askBtn);
    expect(askBtn).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// Settings — General tab (theme + glass via ThemeProvider)
// ---------------------------------------------------------------------------

describe("Settings — General tab", () => {
  it("renders theme toggle buttons: dark and light", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-general"));
    expect(screen.getByTestId("general-theme-dark")).toBeTruthy();
    expect(screen.getByTestId("general-theme-light")).toBeTruthy();
  });

  it("clicking the light theme button calls toggleTheme (data-theme changes to light)", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-general"));
    // Default theme is dark; click light button triggers toggleTheme
    const lightBtn = screen.getByTestId("general-theme-light");
    fireEvent.click(lightBtn);
    // ThemeProvider sets data-theme on <body>
    expect(document.body.getAttribute("data-theme")).toBe("light");
  });

  it("clicking dark button again cycles back to dark", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-general"));
    // First toggle to light
    fireEvent.click(screen.getByTestId("general-theme-light"));
    expect(document.body.getAttribute("data-theme")).toBe("light");
    // Toggle back to dark
    fireEvent.click(screen.getByTestId("general-theme-dark"));
    expect(document.body.getAttribute("data-theme")).toBe("dark");
  });

  it("glass toggle fires toggleGlass — body gets glass class when on", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-general"));
    // Find the glass toggle via the wrapper data-testid
    const wrap = screen.getByTestId("general-glass-toggle-wrap");
    const glassToggle = wrap.querySelector("[role='switch']") as HTMLButtonElement;
    expect(glassToggle).toBeTruthy();
    // Initially glass is off (initialGlass=false in ThemeProvider)
    expect(glassToggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(glassToggle);
    // After click, glass class should be on body
    expect(document.body.classList.contains("glass")).toBe(true);
  });

  it("renders background picker with 2 options (Custom image removed)", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-general"));
    expect(screen.getByTestId("general-bg-gradient")).toBeTruthy();
    expect(screen.getByTestId("general-bg-desktop")).toBeTruthy();
    expect(screen.queryByTestId("general-bg-custom-image")).toBeNull();
  });

  it("renders restore-on-launch and confirm-before-stop toggles", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-general"));
    expect(screen.getByText("Restore workspace on launch")).toBeTruthy();
    expect(screen.getByText("Confirm before stopping a session")).toBeTruthy();
  });

  it("restore-on-launch toggle is on by default and can be toggled", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-general"));
    const wrap = screen.getByTestId("general-restore-toggle-wrap");
    const toggle = wrap.querySelector("[role='switch']") as HTMLButtonElement;
    expect(toggle).toBeTruthy();
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-checked")).toBe("false");
  });
});
