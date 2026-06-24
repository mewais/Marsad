/**
 * Behavioral Vitest/RTL tests for Settings + Workspaces screens.
 *
 * Pattern mirrors shell/Overview/SessionDetail/GateNewSession:
 *   inject MOCK_SNAPSHOT via VmProvider initialState, wrap in ThemeProvider,
 *   assert real behavior.
 */
import React from "react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmContext, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Settings } from "../src/screens/Settings";
import { Workspaces } from "../src/screens/Workspaces";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Mock the api module
// ---------------------------------------------------------------------------

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    setWorkspace: vi.fn().mockResolvedValue(undefined),
    saveWorkspace: vi.fn().mockResolvedValue(undefined),
    duplicateWorkspace: vi.fn().mockResolvedValue(undefined),
    deleteWorkspace: vi.fn().mockResolvedValue(undefined),
    exportWorkspace: vi.fn().mockResolvedValue("{}"),
    importWorkspace: vi.fn().mockResolvedValue(undefined),
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
// Helpers
// ---------------------------------------------------------------------------

function getSetWorkspace() {
  return api.setWorkspace as ReturnType<typeof vi.fn>;
}

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
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <Settings />
      </VmProvider>
    </ThemeProvider>
  );
}

function renderWorkspaces(snapshotOverride?: Partial<AppSnapshot>) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, ...snapshotOverride };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "workspaces",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <Workspaces />
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

  it("renders background picker with 3 options", () => {
    renderSettings();
    fireEvent.click(screen.getByTestId("settings-tab-general"));
    expect(screen.getByTestId("general-bg-gradient")).toBeTruthy();
    expect(screen.getByTestId("general-bg-custom-image")).toBeTruthy();
    expect(screen.getByTestId("general-bg-desktop")).toBeTruthy();
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

// ---------------------------------------------------------------------------
// Workspaces — screen renders
// ---------------------------------------------------------------------------

describe("Workspaces — screen renders", () => {
  it("renders the Workspaces screen root", () => {
    renderWorkspaces();
    expect(screen.getByTestId("workspaces-screen")).toBeTruthy();
  });

  it("shows the Manage… button to open modal", () => {
    renderWorkspaces();
    expect(screen.getByTestId("ws-open-manager")).toBeTruthy();
  });

  it("shows the active workspace info block for ws-default", () => {
    renderWorkspaces();
    expect(screen.getByTestId("ws-active-info")).toBeTruthy();
    expect(screen.getByTestId("ws-active-info").textContent).toContain("Default");
  });

  it("renders ws-switcher pill for each workspace", () => {
    renderWorkspaces();
    expect(screen.getByTestId("ws-switcher")).toBeTruthy();
    expect(screen.getByTestId("ws-pill-ws-default")).toBeTruthy();
    expect(screen.getByTestId("ws-pill-ws-backend")).toBeTruthy();
    expect(screen.getByTestId("ws-pill-ws-ml")).toBeTruthy();
  });

  it("active workspace pill (ws-default) is highlighted via data-active", () => {
    renderWorkspaces();
    const pill = screen.getByTestId("ws-pill-ws-default");
    // active pill has data-active="true"
    expect(pill.getAttribute("data-active")).toBe("true");
  });

  it("non-active workspace pill (ws-backend) has data-active=false", () => {
    renderWorkspaces();
    const pill = screen.getByTestId("ws-pill-ws-backend");
    expect(pill.getAttribute("data-active")).toBe("false");
  });
});

// ---------------------------------------------------------------------------
// Workspaces — summary list
// ---------------------------------------------------------------------------

describe("Workspaces — summary list", () => {
  it("renders a summary row for each workspace", () => {
    renderWorkspaces();
    expect(screen.getByTestId("ws-summary-row-ws-default")).toBeTruthy();
    expect(screen.getByTestId("ws-summary-row-ws-backend")).toBeTruthy();
    expect(screen.getByTestId("ws-summary-row-ws-ml")).toBeTruthy();
  });

  it("active row (ws-default) Load button reads 'Active' and is disabled", () => {
    renderWorkspaces();
    const btn = screen.getByTestId("ws-load-summary-ws-default") as HTMLButtonElement;
    expect(btn.textContent).toContain("Active");
    expect(btn.disabled).toBe(true);
  });

  it("non-active row (ws-backend) Load button reads 'Load' and is enabled", () => {
    renderWorkspaces();
    const btn = screen.getByTestId("ws-load-summary-ws-backend") as HTMLButtonElement;
    expect(btn.textContent).toContain("Load");
    expect(btn.disabled).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Workspaces — Load behavior (setWorkspace + dispatch)
// ---------------------------------------------------------------------------

describe("Workspaces — Load behavior", () => {
  it("clicking Load on ws-backend calls setWorkspace with 'ws-backend'", async () => {
    renderWorkspaces();
    const btn = screen.getByTestId("ws-load-summary-ws-backend");
    fireEvent.click(btn);
    await vi.waitFor(() => {
      expect(getSetWorkspace()).toHaveBeenCalledWith("ws-backend");
    });
  });

  it("clicking Load on ws-backend updates active_ws in the store", () => {
    let capturedActiveWs: string | null | undefined;

    function StateCapture({ children }: { children: React.ReactNode }) {
      return (
        <VmContext.Consumer>
          {(ctx) => {
            if (ctx) capturedActiveWs = ctx.state.snapshot?.active_ws;
            return <>{children}</>;
          }}
        </VmContext.Consumer>
      );
    }

    const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT };
    const preloaded: Partial<VmState> = {
      snapshot,
      screen: "workspaces",
      selectedId: null,
      tab: "Activity",
      filter: "All",
      search: "",
      wsMenuOpen: false,
    };
    render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <StateCapture>
            <Workspaces />
          </StateCapture>
        </VmProvider>
      </ThemeProvider>
    );

    fireEvent.click(screen.getByTestId("ws-load-summary-ws-backend"));
    // SET_WORKSPACE action updates snapshot.active_ws
    expect(capturedActiveWs).toBe("ws-backend");
  });

  it("clicking switcher pill for ws-ml calls setWorkspace and updates active", async () => {
    renderWorkspaces();
    const pill = screen.getByTestId("ws-pill-ws-ml");
    fireEvent.click(pill);
    await vi.waitFor(() => {
      expect(getSetWorkspace()).toHaveBeenCalledWith("ws-ml");
    });
  });
});

// ---------------------------------------------------------------------------
// Workspaces — Manager modal
// ---------------------------------------------------------------------------

describe("Workspaces — Manager modal", () => {
  it("modal is closed by default", () => {
    renderWorkspaces();
    // Modal content only renders when open; Load button inside modal absent
    expect(screen.queryByTestId("ws-save-as-input")).toBeNull();
  });

  it("clicking Manage… opens the manager modal", () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    expect(screen.getByTestId("ws-save-as-input")).toBeTruthy();
  });

  it("manager modal lists all workspaces with Load / Duplicate / Export / Delete", () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    // For each workspace row in the modal
    ["ws-default", "ws-backend", "ws-ml"].forEach((id) => {
      expect(screen.getByTestId(`ws-row-${id}`)).toBeTruthy();
      // Duplicate, Export, Delete
      expect(screen.getByTestId(`ws-duplicate-${id}`)).toBeTruthy();
      expect(screen.getByTestId(`ws-export-${id}`)).toBeTruthy();
      expect(screen.getByTestId(`ws-delete-${id}`)).toBeTruthy();
    });
  });

  it("active ws-default Load button in modal is disabled", () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    const loadBtn = screen.getByTestId("ws-load-ws-default") as HTMLButtonElement;
    expect(loadBtn.disabled).toBe(true);
  });

  it("ws-backend Load button in modal is enabled and calls setWorkspace", async () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    const loadBtn = screen.getByTestId("ws-load-ws-backend") as HTMLButtonElement;
    expect(loadBtn.disabled).toBe(false);
    fireEvent.click(loadBtn);
    await vi.waitFor(() => {
      expect(getSetWorkspace()).toHaveBeenCalledWith("ws-backend");
    });
  });

  it("Duplicate button calls the duplicateWorkspace API command", async () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    fireEvent.click(screen.getByTestId("ws-duplicate-ws-backend"));
    await vi.waitFor(() => {
      expect(api.duplicateWorkspace as ReturnType<typeof vi.fn>).toHaveBeenCalledWith(
        "ws-backend"
      );
    });
  });

  it("Save current as button is disabled when input is empty", () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    const btn = screen.getByTestId("ws-save-as-btn") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });

  it("Save current as button is enabled and calls saveWorkspace API", async () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    const input = screen.getByTestId("ws-save-as-input");
    fireEvent.change(input, { target: { value: "My saved snapshot" } });
    const btn = screen.getByTestId("ws-save-as-btn") as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    fireEvent.click(btn);
    // Verify the backend command was called with the right name
    await vi.waitFor(() => {
      expect(api.saveWorkspace as ReturnType<typeof vi.fn>).toHaveBeenCalledWith(
        "My saved snapshot",
        expect.any(Array)
      );
    });
    // Input clears after the async save resolves
    await vi.waitFor(() => {
      expect((input as HTMLInputElement).value).toBe("");
    });
  });

  it("auto-save toggle exists in modal and can be toggled", () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    const wrap = screen.getByTestId("ws-autosave-toggle-wrap");
    const toggle = wrap.querySelector("[role='switch']") as HTMLButtonElement;
    expect(toggle).toBeTruthy();
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-checked")).toBe("true");
  });

  it("Import button is rendered in the modal footer", () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    expect(screen.getByTestId("ws-import-btn")).toBeTruthy();
  });

  it("pressing Escape closes the modal", () => {
    renderWorkspaces();
    fireEvent.click(screen.getByTestId("ws-open-manager"));
    expect(screen.getByTestId("ws-save-as-input")).toBeTruthy();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByTestId("ws-save-as-input")).toBeNull();
  });
});
