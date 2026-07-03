/**
 * U3 (docs/UPGRADE_PLAN.md — "complete live-activity surfaces") — gate-kind
 * at-a-glance tests.
 *
 * Owner mandate (memory 'marsad-full-experience'): pending HITL gates must be
 * visible at a glance, not just inside the full GateOverlay modal — and a
 * pending *question* must never be indistinguishable from a generic gate.
 *
 * Wire-contract facts these tests pin against (verified by reading the
 * source, not assumed):
 *
 *  - `marsad_gates::Gate` has 5 variants: Permission, Plan, Edit, Question,
 *    Handoff (crates/marsad-gates/src/lib.rs). The desktop `Gate` TS mirror
 *    (src/bridge/types.ts) only carries Permission/Plan/Edit/Question — that
 *    is a separate, narrower gap (Handoff is the X1/Wasl connector's
 *    agent-to-agent gate, not part of U3's local-session live-activity
 *    scope) and is flagged in the report rather than tested here.
 *
 *  - `gateKind()` / `gateSummary()` in src/bridge/useVm.ts (the helpers that
 *    build every `SessionCard` consumed by Overview + the sidebar) handle
 *    Permission / Plan / Edit but silently fall through to `null` for
 *    Question:
 *
 *      function gateKind(sv: SessionView): string | null {
 *        if (!sv.gate) return null;
 *        if ("Permission" in sv.gate) return "permission";
 *        if ("Plan" in sv.gate) return "plan";
 *        if ("Edit" in sv.gate) return "edit";
 *        return null;                       // <-- Question falls through
 *      }
 *
 *    So a session waiting on `AskUserQuestion` renders identically to a
 *    generic "Needs" session on every at-a-glance surface: Overview's
 *    gateKindLabel()/gateLead() both fall back to the literal "Gate", and
 *    the summary box shows "—" instead of the actual question text. The
 *    only place the question is ever shown is inside the full GateOverlay
 *    modal (which a human has to already know to open).
 *
 *  - `SidebarRow` (src/components/AppShell.tsx) doesn't even receive
 *    `gate_kind` / `gate_summary` as props — despite `SessionCard` (which
 *    `sidebarList` is built from) already carrying both fields. So NO gate
 *    kind (Permission/Plan/Question/Edit) is ever visible in the session
 *    list without opening each session individually.
 */
import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import React from "react";
import { VmProvider, VmContext, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Overview } from "../src/screens/Overview";
import { AppShell } from "../src/components/AppShell";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const QUESTION_SESSION: SessionView = {
  ...MOCK_SNAPSHOT.sessions[0],
  id: "sQ1",
  name: "checkout-redesign",
  host_id: "local",
  project: "~/projects/checkout",
  branch: "feat/checkout-v2",
  state: "Needs",
  subagents: 0,
  running_subagents: 0,
  gate: {
    Question: {
      request_id: "req-q1",
      questions: [
        {
          question: "Use REST or GraphQL for the new checkout API?",
          header: "API design",
          multi_select: false,
          options: [
            { label: "REST", description: "Simple, matches existing services" },
            { label: "GraphQL", description: "More flexible client queries" },
          ],
        },
      ],
    },
  },
};

function snapshotWith(extra: SessionView[]): AppSnapshot {
  return {
    ...MOCK_SNAPSHOT,
    sessions: [...MOCK_SNAPSHOT.sessions, ...extra],
  };
}

function renderOverview(sessions: SessionView[] = []) {
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

function renderShell(sessions: SessionView[] = []) {
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
// 1. Overview attention card — a pending question must be surfaced distinctly
// ---------------------------------------------------------------------------

describe("U3 — Overview attention card surfaces a pending question distinctly", () => {
  it("shows a 'Question' kind badge, not the generic 'Gate' fallback", () => {
    renderOverview([QUESTION_SESSION]);
    const card = screen.getByTestId("attention-card-sQ1");
    expect(card.textContent).toContain("Question");
    // Regression guard on the literal fallback string the current code emits.
    expect(card.textContent).not.toContain("GATE");
  });

  it("shows the actual question text as the summary, not the '—' placeholder", () => {
    renderOverview([QUESTION_SESSION]);
    const card = screen.getByTestId("attention-card-sQ1");
    expect(card.textContent).toContain(
      "Use REST or GraphQL for the new checkout API?"
    );
  });
});

// ---------------------------------------------------------------------------
// 2. Sidebar — at-a-glance gate-kind indicator (beyond the state dot)
// ---------------------------------------------------------------------------

describe("U3 — sidebar surfaces the gate kind at a glance", () => {
  it("shows a Permission indicator for s01 (Permission gate) in the session list", () => {
    renderShell();
    const badge = screen.getByTestId("gate-badge-s01");
    expect(badge.textContent).toContain("Permission");
  });

  it("shows a Plan indicator for s02 (Plan gate) in the session list", () => {
    renderShell();
    const badge = screen.getByTestId("gate-badge-s02");
    expect(badge.textContent).toContain("Plan");
  });

  it("shows a Question indicator for a session with a pending question", () => {
    renderShell([QUESTION_SESSION]);
    const badge = screen.getByTestId("gate-badge-sQ1");
    expect(badge.textContent).toContain("Question");
  });

  it("shows NO gate indicator for a session with no gate (absent = nothing)", () => {
    // s04 is Working with gate: null.
    renderShell();
    expect(screen.queryByTestId("gate-badge-s04")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// 3. The answering affordance itself (GateOverlay) already works for
//    questions — pinned here as a regression guard so a future fix to the
//    at-a-glance labeling above doesn't accidentally break the modal.
// ---------------------------------------------------------------------------

describe("U3 — pending question is still a fully answerable affordance once opened", () => {
  it("Review button on the attention card routes to the gate screen", () => {
    let lastScreen: string | undefined;
    let lastSelectedId: string | null | undefined;

    function Capture({ children }: { children: React.ReactNode }) {
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
      snapshot: snapshotWith([QUESTION_SESSION]),
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
          <Capture>
            <Overview />
          </Capture>
        </VmProvider>
      </ThemeProvider>
    );

    const reviewBtn = screen.getByTestId("review-btn-sQ1");
    fireEvent.click(reviewBtn);

    expect(lastSelectedId).toBe("sQ1");
    expect(lastScreen).toBe("gate");
  });
});
