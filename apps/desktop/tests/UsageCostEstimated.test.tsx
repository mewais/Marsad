/**
 * U4 — cost truthfulness, desktop UI (PLAN.md U4 + memory
 * 'marsad-full-experience').
 *
 * Owner mandate: agent-reported cost (claude stream `total_cost_usd` /
 * `modelUsage`) is the source of truth end-to-end; the self-computed
 * litellm/hand pricing table used ONLY for historical transcript backfill
 * MUST be labeled "estimated" in the UI.
 *
 * Today NEITHER the `ModelUsageRow` type (bridge/types.ts) NOR the Usage
 * tab (screens/tabs/Usage.tsx) carries or renders any live-vs-estimated
 * provenance at all — every per-model cost figure renders identically
 * regardless of whether it came from the claude stream's own `result` event
 * or was recomputed by the local pricing table during a transcript backfill.
 * This file pins the missing UI contract:
 *
 *   - a `model_usage[]` row carrying `estimated: true` must render a visible
 *     "estimated"/"~" affordance in the per-model breakdown table;
 *   - a row carrying `estimated: false` (live, agent-reported) must render
 *     plainly, with no estimate marker;
 *   - a session whose displayed cost is backfill-only (no live
 *     agent-reported total — modeled here as a `cost_estimated: true` flag
 *     on the session) must show a visible marker on the top Cost tile;
 *   - a session with a live agent-reported cost shows no such marker.
 *
 * The exact field names (`estimated` / `cost_estimated`) are this test
 * file's proposal for the wire contract, not a requirement already agreed
 * upon elsewhere — see the companion `marsad-core` U4 test files
 * (crates/marsad-core/tests/u4_cost_estimated_tag.rs) for the matching
 * backend-side pin using the same names. Mock data here intentionally
 * carries these fields even though `SessionView`/`ModelUsageRow`'s TS types
 * don't declare them yet — Vitest's esbuild transform doesn't type-check,
 * so this compiles and runs fine, exercising exactly what the component
 * does (or doesn't) render for that data today.
 *
 * ALL "must render a marker" assertions below are expected RED today — this
 * is net-new frontend work, not a partially-implemented feature.
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { UsageTab } from "../src/screens/tabs/Usage";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Harness — render UsageTab directly with a single overridden session
// (mirrors the lighter-weight direct-tab-render pattern used by
// tests/Changes.test.tsx, rather than the full SessionDetail wrapper).
// ---------------------------------------------------------------------------

function renderUsageTab(sessionOverrides: Partial<SessionView>) {
  const base = MOCK_SNAPSHOT.sessions.find((s) => s.id === "s04")!;
  const session = { ...base, ...sessionOverrides } as SessionView;
  const sessions = MOCK_SNAPSHOT.sessions.map((s) =>
    s.id === "s04" ? session : s
  );
  const snapshot: AppSnapshot = { ...MOCK_SNAPSHOT, sessions };

  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "session",
    selectedId: "s04",
    tab: "Usage",
    filter: "All",
    search: "",
    wsMenuOpen: false,
  };

  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <UsageTab />
      </VmProvider>
    </ThemeProvider>
  );
}

// ---------------------------------------------------------------------------
// Per-model breakdown table — live vs estimated rows
// ---------------------------------------------------------------------------

describe("UsageTab model breakdown — cost provenance (U4)", () => {
  it("RED: an estimated (backfill-only) row renders a visible estimated/~ affordance", () => {
    renderUsageTab({
      model_usage: [
        {
          model: "claude-opus-4-5",
          input_tokens: 1000,
          output_tokens: 200,
          cost_usd: 0.0175,
          // Not yet part of the ModelUsageRow TS type — this is the pinned
          // contract this test requires the type/component to grow.
          estimated: true,
        } as unknown as SessionView["model_usage"][number],
      ],
    });

    const row = screen.getByTestId("model-usage-row-claude-opus-4-5");
    expect(
      row.textContent,
      `estimated model_usage row must render a visible estimated/~ marker, got: "${row.textContent}"`
    ).toMatch(/estimated|~/i);
  });

  it("a live (agent-reported) row renders plainly with no estimate marker", () => {
    renderUsageTab({
      model_usage: [
        {
          model: "claude-sonnet-4-6",
          input_tokens: 11,
          output_tokens: 155,
          cost_usd: 0.268,
          estimated: false,
        } as unknown as SessionView["model_usage"][number],
      ],
    });

    const row = screen.getByTestId("model-usage-row-claude-sonnet-4-6");
    expect(
      row.textContent,
      `live model_usage row must NOT render an estimated/~ marker, got: "${row.textContent}"`
    ).not.toMatch(/estimated|~/i);
    // The live cost must still render plainly.
    expect(row.textContent).toContain("0.2680");
  });

  it("RED: a session with only estimated rows and no live rows still marks each row estimated individually (mixed table)", () => {
    renderUsageTab({
      model_usage: [
        {
          model: "claude-haiku-4-5",
          input_tokens: 500,
          output_tokens: 100,
          cost_usd: 0.001,
          estimated: true,
        } as unknown as SessionView["model_usage"][number],
        {
          model: "claude-sonnet-4-6",
          input_tokens: 11,
          output_tokens: 155,
          cost_usd: 0.268,
          estimated: false,
        } as unknown as SessionView["model_usage"][number],
      ],
    });

    const estimatedRow = screen.getByTestId("model-usage-row-claude-haiku-4-5");
    const liveRow = screen.getByTestId("model-usage-row-claude-sonnet-4-6");
    expect(
      estimatedRow.textContent,
      `estimated row in a mixed table must be marked, got: "${estimatedRow.textContent}"`
    ).toMatch(/estimated|~/i);
    expect(
      liveRow.textContent,
      `live row in a mixed table must stay unmarked, got: "${liveRow.textContent}"`
    ).not.toMatch(/estimated|~/i);
  });
});

// ---------------------------------------------------------------------------
// Top Cost tile — session-level provenance
// ---------------------------------------------------------------------------

describe("UsageTab Cost tile — session-level cost provenance (U4)", () => {
  it("RED: a session whose cost is backfill-only (no live agent-reported total) shows a visible estimated affordance on the Cost tile", () => {
    renderUsageTab({
      cost_usd: 0.0175,
      // Not yet part of the SessionView TS type — pinned contract.
      cost_estimated: true,
      compactions: 0,
      last_compaction: null,
      rate_limit: null,
    } as unknown as Partial<SessionView>);

    const tile = screen.getByTestId("cost-value");
    expect(
      tile.textContent,
      `backfill-only session cost must render a visible estimated/~ marker on the Cost tile, got: "${tile.textContent}"`
    ).toMatch(/estimated|~/i);
  });

  it("a session with a live agent-reported cost shows the Cost tile plainly, with no estimate marker", () => {
    renderUsageTab({
      cost_usd: 0.89,
      cost_estimated: false,
      compactions: 0,
      last_compaction: null,
      rate_limit: null,
    } as unknown as Partial<SessionView>);

    const tile = screen.getByTestId("cost-value");
    expect(
      tile.textContent,
      `live-cost session must NOT render an estimated/~ marker on the Cost tile, got: "${tile.textContent}"`
    ).not.toMatch(/estimated|~/i);
    expect(tile.textContent).toContain("$0.89");
  });
});
