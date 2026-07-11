/**
 * BUG #3 — Settings → Hosts row layout. Each host card (local machine, each
 * connected/added remote host, and the add-host form) lays out as distinct
 * labeled rows: Claude Code / Codex / Gemini command rows, a Setup command row,
 * then the larger Environment box. The three agent command rows edit
 * `agent_commands` (claude/codex/gemini), wired through updateHost/addHost.
 *
 * Harness mirrors LifecycleAndDefaults.test.tsx: mock the api module, inject
 * VmProvider initialState, click into the Hosts tab.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Settings } from "../src/screens/Settings";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { Settings as SettingsT } from "../src/bridge/types";
import { DEFAULT_SETTINGS } from "../src/bridge/api";

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
  };
});

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.restoreAllMocks());

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
  fireEvent.click(screen.getByTestId("settings-tab-hosts"));
}

describe("Settings Hosts — row layout (BUG #3)", () => {
  it("local machine card exposes Claude/Codex/Gemini/Setup command rows + Environment box", () => {
    renderSettings();
    expect(screen.getByTestId("local-claude-bin")).toBeTruthy();
    expect(screen.getByTestId("local-codex-cmd")).toBeTruthy();
    expect(screen.getByTestId("local-gemini-cmd")).toBeTruthy();
    expect(screen.getByTestId("local-setup")).toBeTruthy();
    expect(screen.getByTestId("local-env")).toBeTruthy();
    // The labels are present as distinct rows.
    expect(screen.getAllByText("Claude Code command").length).toBeGreaterThan(0);
  });

  it("existing remote host cards show actions only — NO config fields or Save", () => {
    // Per-host config is set once in the add-host form; an existing host card
    // shows only actions (Connect/Remove). The command/Setup/Environment rows and
    // the Save button must NOT render on an existing card.
    renderSettings();
    for (const id of ["hetzner", "gpu"]) {
      expect(screen.getByTestId(`host-row-${id}`)).toBeTruthy();
      expect(screen.queryByTestId(`host-claude-bin-${id}`)).toBeNull();
      expect(screen.queryByTestId(`host-codex-cmd-${id}`)).toBeNull();
      expect(screen.queryByTestId(`host-gemini-cmd-${id}`)).toBeNull();
      expect(screen.queryByTestId(`host-setup-${id}`)).toBeNull();
      expect(screen.queryByTestId(`host-env-${id}`)).toBeNull();
      expect(screen.queryByTestId(`host-save-${id}`)).toBeNull();
    }
  });

  it("add-host form lays out one command per row (Claude/Codex/Gemini/Setup) + Environment", () => {
    renderSettings();
    expect(screen.getByTestId("host-add-claude-bin")).toBeTruthy();
    expect(screen.getByTestId("host-add-codex-cmd")).toBeTruthy();
    expect(screen.getByTestId("host-add-gemini-cmd")).toBeTruthy();
    expect(screen.getByTestId("host-add-setup")).toBeTruthy();
    expect(screen.getByTestId("host-add-env")).toBeTruthy();
  });
});
