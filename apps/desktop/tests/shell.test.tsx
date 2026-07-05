/**
 * Smoke test: AppShell renders the Workstreams-redesign shell (SLICE A) —
 * a glass header (wordmark + breadcrumbs + new-workstream/settings/theme
 * controls) with NO left sidebar, NO Overview entry, NO Workspaces switcher.
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { AppShell } from "../src/components/AppShell";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

function renderWithMock(ui: React.ReactNode, overrides: Partial<VmState> = {}) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "board",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    ...overrides,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>{ui}</VmProvider>
    </ThemeProvider>
  );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("AppShell smoke test (Workstreams redesign SLICE A)", () => {
  it("renders the Marsad wordmark", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    const wordmark = screen.getByTestId("wordmark");
    expect(wordmark.textContent).toContain("Marsad");
  });

  it("renders NO left sidebar", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    expect(document.querySelector("aside")).toBeNull();
  });

  it("renders NO Overview nav entry and NO workspace switcher", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    expect(screen.queryByTestId("overview-entry")).toBeNull();
    expect(screen.queryByTestId("ws-switcher")).toBeNull();
    expect(screen.queryByText(/^Default$/)).toBeNull();
  });

  it("renders the '+ New workstream' button", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    const btn = screen.getByTestId("new-workstream-btn");
    expect(btn.textContent).toContain("New workstream");
  });

  it("renders the settings and theme-toggle buttons", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    expect(screen.getByTestId("open-settings-btn")).toBeTruthy();
    expect(screen.getByTitle("Toggle theme")).toBeTruthy();
  });

  it("renders no breadcrumbs while on the board (home) screen", () => {
    renderWithMock(<AppShell><div /></AppShell>, { screen: "board" });
    expect(screen.queryByTestId("crumb-0")).toBeNull();
  });

  it("renders the wallpaper layer behind the header/body", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    expect(screen.getByTestId("wallpaper")).toBeTruthy();
  });

  it("renders main slot content", () => {
    renderWithMock(
      <AppShell>
        <div data-testid="main-slot">hello world</div>
      </AppShell>
    );
    expect(screen.getByTestId("main-slot").textContent).toBe("hello world");
  });
});
