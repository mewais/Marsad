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
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
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

  it("each connected remote host card exposes the same command rows + Environment box", () => {
    renderSettings();
    for (const id of ["hetzner", "gpu"]) {
      expect(screen.getByTestId(`host-claude-bin-${id}`)).toBeTruthy();
      expect(screen.getByTestId(`host-codex-cmd-${id}`)).toBeTruthy();
      expect(screen.getByTestId(`host-gemini-cmd-${id}`)).toBeTruthy();
      expect(screen.getByTestId(`host-setup-${id}`)).toBeTruthy();
      expect(screen.getByTestId(`host-env-${id}`)).toBeTruthy();
      expect(screen.getByTestId(`host-save-${id}`)).toBeTruthy();
    }
  });

  it("saving a remote host wires the agent command rows through updateHost agentCommands", async () => {
    renderSettings();
    fireEvent.change(screen.getByTestId("host-codex-cmd-hetzner"), {
      target: { value: "codex --sandbox" },
    });
    fireEvent.change(screen.getByTestId("host-gemini-cmd-hetzner"), {
      target: { value: "/opt/gemini/bin/gemini" },
    });
    fireEvent.click(screen.getByTestId("host-save-hetzner"));

    await waitFor(() => expect(updateHost).toHaveBeenCalled());
    const call = updateHost.mock.calls[0] as unknown[];
    // updateHost(id, claudeBin, setup, env, target, agentCommands)
    expect(call[0]).toBe("hetzner");
    const agentCommands = call[5] as Array<[string, string]>;
    expect(agentCommands).toEqual(
      expect.arrayContaining([
        ["codex", "codex --sandbox"],
        ["gemini", "/opt/gemini/bin/gemini"],
      ])
    );
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
