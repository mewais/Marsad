/**
 * Changes tab — per-file diff fetching behavior.
 *
 * Verifies that selecting a changed file (with no pending Edit gate) fetches its
 * diff via getFileDiff and renders it, shows "No diff" for an empty result, and
 * surfaces an error without crashing.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { ChangesTab } from "../src/screens/tabs/Changes";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SideRow } from "../src/bridge/types";
import * as api from "../src/bridge/api";

// A single styled diff row as getFileDiff would return.
const FAKE_ROW: SideRow = {
  l: { ln: "1", sig: "-", sigColor: "#e0594a", text: "old", fg: "#e0594a", bg: "rgba(224,89,74,0.12)" },
  r: { ln: "1", sig: "+", sigColor: "#46c08a", text: "new", fg: "#46c08a", bg: "rgba(70,192,138,0.12)" },
};

function snapshotWithChange(): AppSnapshot {
  const sessions = MOCK_SNAPSHOT.sessions.map((s) =>
    s.id === "s01"
      ? {
          ...s,
          gate: null, // drop the edit/permission gate so the fetch path runs
          changes: [{ path: "src/foo.ts", status: "M", added: 1, removed: 1 }],
        }
      : s
  );
  return { ...MOCK_SNAPSHOT, sessions, selected_id: "s01" };
}

function renderTab() {
  const preloaded: Partial<VmState> = {
    snapshot: snapshotWithChange(),
    screen: "session",
    selectedId: "s01",
    tab: "Changes",
    filter: "All",
    search: "",
    wsMenuOpen: false,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <ChangesTab />
      </VmProvider>
    </ThemeProvider>
  );
}

describe("ChangesTab per-file diff", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("fetches and renders the active file's diff", async () => {
    const spy = vi.spyOn(api, "getFileDiff").mockResolvedValue([FAKE_ROW]);
    renderTab();
    await waitFor(() => expect(spy).toHaveBeenCalledWith("s01", "src/foo.ts"));
    // The diff text from the fetched row should render.
    await waitFor(() => expect(screen.getByText("new")).toBeTruthy());
    expect(screen.getByText("old")).toBeTruthy();
  });

  it("shows 'No diff' when the fetch returns nothing", async () => {
    vi.spyOn(api, "getFileDiff").mockResolvedValue([]);
    renderTab();
    await waitFor(() => expect(screen.getByText("No diff")).toBeTruthy());
  });

  it("surfaces an error without crashing", async () => {
    vi.spyOn(api, "getFileDiff").mockRejectedValue(new Error("boom"));
    renderTab();
    await waitFor(() =>
      expect(screen.getByText(/Failed to load diff: boom/)).toBeTruthy()
    );
  });
});
