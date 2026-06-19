/**
 * Smoke test: AppShell renders from the mock store.
 * Asserts the wordmark and filter chips render.
 */
import { describe, it, expect } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { AppShell } from "../src/components/AppShell";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

function renderWithMock(ui: React.ReactNode) {
  const preloaded: Partial<VmState> = {
    snapshot: MOCK_SNAPSHOT,
    screen: "overview",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
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

describe("AppShell smoke test", () => {
  it("renders the Marsad wordmark", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    const wordmark = screen.getByTestId("wordmark");
    expect(wordmark.textContent).toContain("Marsad");
  });

  it("renders all four filter chips", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    // The filter chips are <button> elements inside the sidebar.
    // Query by role=button with accessible name to avoid collisions with
    // session badges (which are <span>, not <button>).
    const sidebar = document.querySelector("aside")!;
    const buttons = within(sidebar).getAllByRole("button");
    const buttonTexts = buttons.map((b) => b.textContent ?? "");
    expect(buttonTexts.some((t) => t.includes("All"))).toBe(true);
    expect(buttonTexts.some((t) => t.includes("Needs you"))).toBe(true);
    expect(buttonTexts.some((t) => t.includes("Working"))).toBe(true);
    expect(buttonTexts.some((t) => t.includes("Errors"))).toBe(true);
  });

  it("renders the Overview entry in sidebar", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    expect(screen.getByTestId("overview-entry")).toBeTruthy();
    expect(screen.getByTestId("overview-entry").textContent).toContain("Overview");
  });

  it("renders host health footer entries", () => {
    renderWithMock(<AppShell><div /></AppShell>);
    const sidebar = document.querySelector("aside")!;
    // The footer always has host labels in mono spans
    const allText = sidebar.textContent ?? "";
    expect(allText).toContain("localhost");
    expect(allText).toContain("hetzner-cx51");
    expect(allText).toContain("gpu-worker");
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
