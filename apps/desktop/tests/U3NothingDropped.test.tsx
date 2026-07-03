/**
 * U3 (docs/UPGRADE_PLAN.md — "complete live-activity surfaces") — the
 * "nothing silently dropped" test.
 *
 * Owner mandate (PLAN.md U3 + memory 'marsad-full-experience'): "everything
 * visible — running subagents, pending questions, HITL gates, plan mode,
 * compaction indicators, rate-limit indicators. Nothing silently dropped by
 * the reducer or UI."
 *
 * This file feeds ONE session carrying every live-activity signal that the
 * current wire contract (marsad_viewmodel::SessionView) is actually capable
 * of carrying at once — a pending question gate AND running/completed
 * subagents, on a session that Needs attention — and asserts each signal
 * has *some* visible surface on the two places a human scans first: the
 * Overview attention card and the sidebar session list. Today, per the
 * source (see U3GateKindSurfaces.test.tsx / U3RunningSubagents.test.tsx for
 * the individual gap citations), BOTH silently drop most of this:
 *
 *   - AttentionCard never references `sv.subagents` / `sv.running_subagents`
 *     at all.
 *   - gateKind()/gateSummary() (src/bridge/useVm.ts) return null for a
 *     Question gate, so the card's kind badge falls back to the literal
 *     "Gate" and its summary box falls back to "—" — the actual question
 *     text is dropped.
 *   - SidebarRow (src/components/AppShell.tsx) never receives gate_kind /
 *     gate_summary / subagent counts as props at all.
 *
 * ---------------------------------------------------------------------
 * EXCLUDED ON PURPOSE — backend gap, not faked here (see final report):
 * ---------------------------------------------------------------------
 * Compaction and rate-limit signals are NOT included in this fixture.
 * Verified by reading crates/marsad-proto/src/lib.rs and
 * crates/marsad-events/src/lib.rs: every `system` stream subtype other than
 * `system/init` (`compact_boundary`, rate-limit notices, thinking_tokens,
 * task_started/progress, etc.) parses to `StreamMsg::Unknown`, and
 * `reduce_stream`'s `StreamMsg::Unknown(_) => state` arm is a pure no-op —
 * SessionState has NO `compaction` or `rate_limit` field whatsoever (grep
 * confirms zero occurrences of either concept anywhere in
 * marsad-events/marsad-viewmodel/the desktop TS types). There is no wire
 * shape to pin a frontend test against yet; writing one would mean
 * inventing a field name/shape the backend doesn't emit, which is exactly
 * the kind of fake the task asked us not to produce. This is a genuine
 * backend gap spanning marsad-proto (parse the subtype instead of routing
 * to Unknown) → marsad-events (fold it into SessionState) →
 * marsad-viewmodel (surface it on SessionView) that needs to land before a
 * meaningful UI test can be written.
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Overview } from "../src/screens/Overview";
import { AppShell } from "../src/components/AppShell";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Fixture — every activity signal the wire contract can carry, at once
// ---------------------------------------------------------------------------

const EVERYTHING_SESSION: SessionView = {
  ...MOCK_SNAPSHOT.sessions[0],
  id: "sAll1",
  name: "payments-migration",
  host_id: "hetzner",
  project: "~/projects/payments",
  branch: "feat/stripe-v2",
  state: "Needs",
  subagents: 3,
  running_subagents: 1,
  gate: {
    Question: {
      request_id: "req-all-1",
      questions: [
        {
          question: "Roll out the new payment flow behind a feature flag?",
          header: "Rollout strategy",
          multi_select: false,
          options: [
            { label: "Yes, flag it", description: "Safer, staged rollout" },
            { label: "No, ship directly", description: "Faster, riskier" },
          ],
        },
      ],
    },
  },
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

function renderShell(sessions: SessionView[]) {
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
        <AppShell>
          <div />
        </AppShell>
      </VmProvider>
    </ThemeProvider>
  );
}

// ---------------------------------------------------------------------------
// Overview attention card — every signal visible on ONE card
// ---------------------------------------------------------------------------

describe("U3 — nothing silently dropped: Overview attention card", () => {
  it("surfaces the pending question kind + text", () => {
    renderOverview([EVERYTHING_SESSION]);
    const card = screen.getByTestId("attention-card-sAll1");
    expect(card.textContent).toContain("Question");
    expect(card.textContent).toContain(
      "Roll out the new payment flow behind a feature flag?"
    );
  });

  it("surfaces the running-subagent count", () => {
    renderOverview([EVERYTHING_SESSION]);
    const card = screen.getByTestId("attention-card-sAll1");
    expect(card.textContent).toMatch(/1/);
    expect(card.textContent?.toLowerCase()).toContain("sub");
  });

  it("surfaces the completed-subagent count (3 total, 1 running => 2 done)", () => {
    renderOverview([EVERYTHING_SESSION]);
    const card = screen.getByTestId("attention-card-sAll1");
    expect(card.textContent).toMatch(/2/);
  });
});

// ---------------------------------------------------------------------------
// Sidebar — every signal visible on ONE row
// ---------------------------------------------------------------------------

describe("U3 — nothing silently dropped: sidebar row", () => {
  it("surfaces the gate kind (Question) at a glance", () => {
    renderShell([EVERYTHING_SESSION]);
    const gateBadge = screen.getByTestId("gate-badge-sAll1");
    expect(gateBadge.textContent).toContain("Question");
  });

  it("surfaces the running-subagent count at a glance", () => {
    renderShell([EVERYTHING_SESSION]);
    const subBadge = screen.getByTestId("subagent-indicator-sAll1");
    expect(subBadge.textContent).toMatch(/1/);
  });
});
