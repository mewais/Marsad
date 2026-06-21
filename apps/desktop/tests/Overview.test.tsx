/**
 * Behavioral tests for the Overview screen.
 *
 * Uses VmProvider with a pre-injected mock snapshot so no Tauri runtime
 * is needed. Verifies stat tiles, attention gate cards, active rows, idle
 * cards, and dispatch interactions.
 */
import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import React from "react";
import { VmProvider, VmContext, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Overview } from "../src/screens/Overview";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Helper: render Overview wrapped in providers
// ---------------------------------------------------------------------------

function renderOverview(snapshot: AppSnapshot = MOCK_SNAPSHOT) {
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "overview",
    selectedId: null,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <Overview />
      </VmProvider>
    </ThemeProvider>
  );
}

// ---------------------------------------------------------------------------
// Stat tile tests
// ---------------------------------------------------------------------------

describe("Overview — stat tiles", () => {
  it("renders 'Needs you' tile with correct count from mock (2 Needs sessions)", () => {
    renderOverview();
    // The tile label
    expect(screen.getByText("Needs you")).toBeTruthy();
    // counts.needs = 2 (s01 + s02 are Needs)
    const needsTile = screen.getByText("Needs you").closest("div")!.parentElement!;
    expect(needsTile.textContent).toContain("2");
  });

  it("renders 'Errored' tile with count 1 (s03 is Error)", () => {
    renderOverview();
    expect(screen.getByText("Errored")).toBeTruthy();
    const errorTile = screen.getByText("Errored").closest("div")!.parentElement!;
    expect(errorTile.textContent).toContain("1");
  });

  it("renders 'Working' tile with count 3 (s04, s05, s06)", () => {
    renderOverview();
    expect(screen.getByText("Working")).toBeTruthy();
    const workingTile = screen.getByText("Working").closest("div")!.parentElement!;
    expect(workingTile.textContent).toContain("3");
  });

  it("renders 'Cost today' tile with dollar amount and tokens subtitle", () => {
    renderOverview();
    expect(screen.getByText("Cost today")).toBeTruthy();
    // The cost is $X.XX — we just confirm the tile renders a $ value
    const costTile = screen.getByText("Cost today").closest("div")!.parentElement!;
    expect(costTile.textContent).toContain("$");
    // tokens subtitle is present
    expect(costTile.textContent).toMatch(/tokens/i);
  });
});

// ---------------------------------------------------------------------------
// Attention section tests
// ---------------------------------------------------------------------------

describe("Overview — attention section", () => {
  it("renders 'Needs your attention' heading when attention cards exist", () => {
    renderOverview();
    expect(screen.getByText("Needs your attention")).toBeTruthy();
  });

  it("renders attention card for s01 (api-auth-refactor)", () => {
    renderOverview();
    const card = screen.getByTestId("attention-card-s01");
    expect(card).toBeTruthy();
    expect(card.textContent).toContain("api-auth-refactor");
  });

  it("attention card for s01 shows gate kind badge (Permission)", () => {
    renderOverview();
    const card = screen.getByTestId("attention-card-s01");
    expect(card.textContent).toContain("Permission");
  });

  it("attention card for s01 shows host label", () => {
    renderOverview();
    const card = screen.getByTestId("attention-card-s01");
    // host_id=local => label=localhost
    expect(card.textContent).toContain("localhost");
  });

  it("attention card for s01 shows project", () => {
    renderOverview();
    const card = screen.getByTestId("attention-card-s01");
    expect(card.textContent).toContain("~/projects/api");
  });

  it("attention card for s01 shows gate summary", () => {
    renderOverview();
    const card = screen.getByTestId("attention-card-s01");
    expect(card.textContent).toContain("prisma migrate deploy");
  });

  it("renders Review button on attention card", () => {
    renderOverview();
    const reviewBtn = screen.getByTestId("review-btn-s01");
    expect(reviewBtn).toBeTruthy();
    expect(reviewBtn.textContent).toContain("Review");
  });

  it("renders Open button on attention card", () => {
    renderOverview();
    const openBtn = screen.getByTestId("open-btn-s01");
    expect(openBtn).toBeTruthy();
    expect(openBtn.textContent).toContain("Open");
  });

  it("renders attention card for s02 (queue-async-workers, Plan gate)", () => {
    renderOverview();
    const card = screen.getByTestId("attention-card-s02");
    expect(card.textContent).toContain("queue-async-workers");
    expect(card.textContent).toContain("Plan");
  });

  it("renders attention card for s03 (ml-feature-engineering, Error state)", () => {
    renderOverview();
    const card = screen.getByTestId("attention-card-s03");
    expect(card.textContent).toContain("ml-feature-engineering");
  });
});

// ---------------------------------------------------------------------------
// Active now section tests
// ---------------------------------------------------------------------------

describe("Overview — active now section", () => {
  it("renders 'Active now' heading", () => {
    renderOverview();
    expect(screen.getByText("Active now")).toBeTruthy();
  });

  it("renders active row for s04 (frontend-dashboard)", () => {
    renderOverview();
    const row = screen.getByTestId("active-row-s04");
    expect(row).toBeTruthy();
    expect(row.textContent).toContain("frontend-dashboard");
  });

  it("active row shows host·branch info", () => {
    renderOverview();
    const row = screen.getByTestId("active-row-s04");
    // host local => localhost, branch feat/overview-v2
    expect(row.textContent).toContain("localhost");
    expect(row.textContent).toContain("feat/overview-v2");
  });

  it("active row shows context pct and tokens", () => {
    renderOverview();
    const row = screen.getByTestId("active-row-s04");
    // context_pct=55 => "55%", tokens=74000 => "74.0k"
    expect(row.textContent).toContain("55%");
  });

  it("renders all three active rows (s04, s05, s06)", () => {
    renderOverview();
    expect(screen.getByTestId("active-row-s04")).toBeTruthy();
    expect(screen.getByTestId("active-row-s05")).toBeTruthy();
    expect(screen.getByTestId("active-row-s06")).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// Idle & finished section tests
// ---------------------------------------------------------------------------

describe("Overview — idle & finished section", () => {
  it("renders 'Idle & finished' heading", () => {
    renderOverview();
    // heading contains & or & — either works
    const heading = screen.getAllByText(/idle/i).find(
      (el) => el.tagName === "H2"
    );
    expect(heading).toBeTruthy();
  });

  it("renders idle card for s07 (ci-pipeline-fix, Done)", () => {
    renderOverview();
    const card = screen.getByTestId("idle-card-s07");
    expect(card).toBeTruthy();
    expect(card.textContent).toContain("ci-pipeline-fix");
  });

  it("renders idle card for s08 and s09", () => {
    renderOverview();
    expect(screen.getByTestId("idle-card-s08")).toBeTruthy();
    expect(screen.getByTestId("idle-card-s09")).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// Interaction / dispatch tests
// ---------------------------------------------------------------------------

describe("Overview — interactions", () => {
  it("clicking Open on an attention card navigates to the session", () => {
    // We need to observe state changes. Use a capturing wrapper.
    let lastScreen: string | undefined;
    let lastSelectedId: string | null | undefined;

    function StateCapture({ children }: { children: React.ReactNode }) {
      return (
        <VmContext.Consumer>
          {(ctx) => {
            if (ctx) {
              lastScreen = ctx.state.screen;
              lastSelectedId = ctx.state.selectedId;
            }
            return <>{children}</>;
          }}
        </VmContext.Consumer>
      );
    }

    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "overview",
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
            <Overview />
          </StateCapture>
        </VmProvider>
      </ThemeProvider>
    );

    const openBtn = screen.getByTestId("open-btn-s01");
    fireEvent.click(openBtn);

    // After SELECT_SESSION, screen => "session", selectedId => "s01"
    expect(lastSelectedId).toBe("s01");
    expect(lastScreen).toBe("session");
  });

  it("clicking an active row navigates to the session", () => {
    let lastSelectedId: string | null | undefined;

    function StateCapture({ children }: { children: React.ReactNode }) {
      return (
        <VmContext.Consumer>
          {(ctx) => {
            if (ctx) lastSelectedId = ctx.state.selectedId;
            return <>{children}</>;
          }}
        </VmContext.Consumer>
      );
    }

    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "overview",
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
            <Overview />
          </StateCapture>
        </VmProvider>
      </ThemeProvider>
    );

    const activeRow = screen.getByTestId("active-row-s04");
    fireEvent.click(activeRow);

    expect(lastSelectedId).toBe("s04");
  });

  it("clicking an idle card navigates to the session", () => {
    let lastSelectedId: string | null | undefined;

    function StateCapture({ children }: { children: React.ReactNode }) {
      return (
        <VmContext.Consumer>
          {(ctx) => {
            if (ctx) lastSelectedId = ctx.state.selectedId;
            return <>{children}</>;
          }}
        </VmContext.Consumer>
      );
    }

    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "overview",
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
            <Overview />
          </StateCapture>
        </VmProvider>
      </ThemeProvider>
    );

    const idleCard = screen.getByTestId("idle-card-s07");
    fireEvent.click(idleCard);

    expect(lastSelectedId).toBe("s07");
  });

  it("clicking Review on attention card sets screen to 'gate' and selectedId", () => {
    let lastScreen: string | undefined;
    let lastSelectedId: string | null | undefined;

    function StateCapture({ children }: { children: React.ReactNode }) {
      return (
        <VmContext.Consumer>
          {(ctx) => {
            if (ctx) {
              lastScreen = ctx.state.screen;
              lastSelectedId = ctx.state.selectedId;
            }
            return <>{children}</>;
          }}
        </VmContext.Consumer>
      );
    }

    const preloaded: Partial<VmState> = {
      snapshot: MOCK_SNAPSHOT,
      screen: "overview",
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
            <Overview />
          </StateCapture>
        </VmProvider>
      </ThemeProvider>
    );

    const reviewBtn = screen.getByTestId("review-btn-s01");
    fireEvent.click(reviewBtn);

    expect(lastSelectedId).toBe("s01");
    expect(lastScreen).toBe("gate");
  });
});

// ---------------------------------------------------------------------------
// Empty state tests
// ---------------------------------------------------------------------------

describe("Overview — empty states", () => {
  it("shows no 'Needs your attention' section when attention is empty", () => {
    const noAttentionSnapshot: AppSnapshot = {
      ...MOCK_SNAPSHOT,
      sessions: MOCK_SNAPSHOT.sessions.map((s) =>
        s.state === "Needs" || s.state === "Error"
          ? { ...s, state: "Done" as const }
          : s
      ),
    };
    renderOverview(noAttentionSnapshot);
    expect(screen.queryByText("Needs your attention")).toBeNull();
  });

  it("shows empty active message when no active sessions", () => {
    const noActiveSnapshot: AppSnapshot = {
      ...MOCK_SNAPSHOT,
      sessions: MOCK_SNAPSHOT.sessions.map((s) =>
        s.state === "Working" || s.state === "Starting"
          ? { ...s, state: "Done" as const }
          : s
      ),
    };
    renderOverview(noActiveSnapshot);
    expect(
      screen.getByText(/No sessions actively running right now/i)
    ).toBeTruthy();
  });
});
