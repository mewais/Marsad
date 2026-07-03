/**
 * Review finding #4 (UI layer): the Overview "Cost today" tile must INCLUDE
 * an estimated-only session's cost.
 *
 * Before the fix, a backfill-only session (no live agent-reported `result`)
 * had `cost_usd: 0.0` while its `model_usage` rows carried real, nonzero
 * estimated dollars. `computeCounts` (bridge/useVm.ts) sums/filters on
 * `cost_usd`, so such a session contributed $0.00 and was silently excluded
 * from "Cost today" — even though the Usage tab's own per-model breakdown
 * showed nonzero estimated dollars for it.
 *
 * The fix lives in Rust (`marsad-core::make_session_view`): an estimated
 * session's `SessionView.cost_usd` now reflects the summed per-model
 * estimated cost, so the value the desktop receives is already consistent
 * with its `model_usage` breakdown. This test pins the downstream UI
 * contract: given such a (consistent) SessionView, the "Cost today" tile
 * shows that summed cost (not $0.00) and marks it estimated.
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Overview } from "../src/screens/Overview";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

/**
 * A single estimated-only session whose headline `cost_usd` equals the sum
 * of its model-breakdown rows (0.30 + 0.20 = 0.50) — exactly the consistent
 * shape `make_session_view` now produces for a backfill-only session.
 */
function estimatedOnlySession(): SessionView {
  const base = MOCK_SNAPSHOT.sessions[0];
  return {
    ...base,
    id: "s-estimated",
    name: "backfilled-session",
    state: "Done",
    gate: null,
    tokens: 1_500_000,
    cost_usd: 0.5,
    cost_estimated: true,
    model_usage: [
      {
        model: "claude-sonnet-4-6",
        input_tokens: 1_000_000,
        output_tokens: 500_000,
        cost_usd: 0.3,
        estimated: true,
      },
      {
        model: "claude-haiku-4-5",
        input_tokens: 200_000,
        output_tokens: 100_000,
        cost_usd: 0.2,
        estimated: true,
      },
    ],
  } as SessionView;
}

function renderOverviewWith(sessions: SessionView[]) {
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, sessions };
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

describe("Overview 'Cost today' — estimated-only session inclusion (review #4)", () => {
  it("includes an estimated-only session's cost in the 'Cost today' total", () => {
    renderOverviewWith([estimatedOnlySession()]);

    const costValue = screen.getByTestId("overview-cost-value");
    // The tile must show the summed estimated cost ($0.50), NOT the stale
    // $0.00 that the pre-fix `cost_usd: 0` passthrough would have produced.
    expect(costValue.textContent).toContain("$0.50");
    expect(costValue.textContent).not.toContain("$0.00");
    // And it must be marked estimated (every costing session is backfill-only).
    expect(costValue.textContent).toMatch(/estimated/i);
  });

  it("the tile total equals the sum of that session's model-usage breakdown", () => {
    const session = estimatedOnlySession();
    const breakdownSum = session.model_usage.reduce((a, r) => a + r.cost_usd, 0);
    expect(breakdownSum).toBeCloseTo(0.5, 9);

    renderOverviewWith([session]);
    const costValue = screen.getByTestId("overview-cost-value");
    expect(costValue.textContent).toContain(`$${breakdownSum.toFixed(2)}`);
  });
});
