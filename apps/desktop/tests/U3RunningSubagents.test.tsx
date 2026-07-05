/**
 * U3 (docs/UPGRADE_PLAN.md — "complete live-activity surfaces") — running
 * subagents pinned at the at-a-glance surfaces (Overview's Active-now row
 * and attention cards, plus the sidebar).
 *
 * Wire-contract facts (verified by reading the source):
 *
 *  - `marsad_events::SessionState` (crates/marsad-events/src/lib.rs) folds a
 *    rich `Vec<Subagent>` with per-agent `name` / `model` / `status`
 *    (Running | Done) from Task/Agent tool_use blocks.
 *
 *  - `marsad_viewmodel::SessionView` (crates/marsad-viewmodel/src/lib.rs)
 *    — the struct that actually crosses the wire to the desktop — collapses
 *    that list down to two integers before it ever reaches the frontend:
 *
 *      pub subagents: u32,           // completed count
 *      pub running_subagents: u32,   // running count
 *
 *    So "per-subagent state" (which agent, which model, is THIS one running
 *    or done) is a real backend/viewmodel gap — flagged in the report, not
 *    faked here. These tests pin what the wire format actually allows:
 *    aggregate running vs. completed counts.
 *
 *  - `apps/desktop/src/screens/tabs/Activity.tsx` (`deriveTranscript`)
 *    ALREADY renders both counts as feed entries once you've opened a
 *    session's Activity tab — pinned below as a regression guard (expected
 *    green).
 *
 *  - `apps/desktop/src/screens/Overview.tsx`'s `ActiveRow` has a dead code
 *    path for exactly this: a subagent chip gated on
 *    `card.last_action?.startsWith("⊟")` — but `toCard()` in
 *    src/bridge/useVm.ts hardcodes `last_action: null` for every card, so
 *    that branch can never fire. Subagent activity is invisible on the
 *    Overview at-a-glance surface no matter how many are running.
 *    `AttentionCard` doesn't reference subagents at all.
 *
 *  - The former sidebar-row assertions in this file were removed: the
 *    Workstreams redesign (SLICE A) deletes the sidebar entirely, so there
 *    is no more session-list row to surface a subagent indicator on.
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Overview } from "../src/screens/Overview";
import { SessionDetail } from "../src/screens/SessionDetail";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const RUNNING_SUBAGENT_SESSION: SessionView = {
  ...MOCK_SNAPSHOT.sessions[3], // s04 shape (Working, no gate)
  id: "sSub1",
  name: "batch-import-tool",
  host_id: "local",
  project: "~/projects/batch-import",
  branch: "feat/parallel-import",
  state: "Working",
  gate: null,
  subagents: 3,
  running_subagents: 2,
};

const NO_SUBAGENT_SESSION: SessionView = {
  ...MOCK_SNAPSHOT.sessions[3],
  id: "sSub0",
  name: "solo-task",
  host_id: "local",
  project: "~/projects/solo",
  branch: "main",
  state: "Working",
  gate: null,
  subagents: 0,
  running_subagents: 0,
};

function snapshotWith(extra: SessionView[]): AppSnapshot {
  return { ...MOCK_SNAPSHOT, sessions: [...MOCK_SNAPSHOT.sessions, ...extra] };
}

function renderOverview(sessions: SessionView[]) {
  const preloaded: Partial<VmState> = {
    snapshot: snapshotWith(sessions),
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
// 1. Overview "Active now" row — running subagent count, at a glance
// ---------------------------------------------------------------------------

describe("U3 — Overview active row surfaces running subagent count", () => {
  it("shows the running-subagent count on the active row when running_subagents > 0", () => {
    renderOverview([RUNNING_SUBAGENT_SESSION]);
    const row = screen.getByTestId("active-row-sSub1");
    expect(row.textContent).toMatch(/2/);
    expect(row.textContent?.toLowerCase()).toContain("sub");
  });

  it("shows NO subagent indicator on the active row when running_subagents === 0 (absent = nothing)", () => {
    renderOverview([NO_SUBAGENT_SESSION]);
    const row = screen.getByTestId("active-row-sSub0");
    expect(row.textContent?.toLowerCase()).not.toContain("subagent");
    expect(row.textContent?.toLowerCase()).not.toContain("sub-agent");
  });
});

// ---------------------------------------------------------------------------
// 2. SessionDetail's Activity tab — regression guard for the ALREADY WORKING
//    aggregate-count rendering, so any future fix to the surfaces above
//    doesn't regress this path (expected GREEN today).
// ---------------------------------------------------------------------------

describe("U3 — Activity tab already renders subagent counts (regression guard)", () => {
  it("shows a 'running' entry with the running count once the session is open", () => {
    const snapshot = snapshotWith([RUNNING_SUBAGENT_SESSION]);
    const preloaded: Partial<VmState> = {
      snapshot,
      screen: "session",
      selectedId: "sSub1",
      tab: "Activity",
      filter: "All",
      search: "",
      wsMenuOpen: false,
      capabilities: {},
    };
    render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <SessionDetail />
        </VmProvider>
      </ThemeProvider>
    );
    expect(screen.getByText(/2 sub-agents running/i)).toBeTruthy();
  });

  it("shows a 'done' entry for completed subagents (total minus running)", () => {
    const snapshot = snapshotWith([RUNNING_SUBAGENT_SESSION]);
    const preloaded: Partial<VmState> = {
      snapshot,
      screen: "session",
      selectedId: "sSub1",
      tab: "Activity",
      filter: "All",
      search: "",
      wsMenuOpen: false,
      capabilities: {},
    };
    render(
      <ThemeProvider initialTheme="dark">
        <VmProvider initialState={preloaded}>
          <SessionDetail />
        </VmProvider>
      </ThemeProvider>
    );
    // subagents=3, running_subagents=2 => 1 completed.
    expect(screen.getByText(/1 sub-agent$/i)).toBeTruthy();
  });
});
