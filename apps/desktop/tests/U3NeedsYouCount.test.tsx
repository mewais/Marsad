/**
 * U3 (docs/UPGRADE_PLAN.md — "complete live-activity surfaces") — the
 * Overview "Needs you" stat tile must count every flavor of session that is
 * actually waiting on the human: a permission gate, a plan-approval gate, a
 * pending question gate, AND a bare Notification (no gate object at all —
 * `HookEvent::Notification` in crates/marsad-events/src/lib.rs sets
 * `Phase::Needs` without touching `state.gate`).
 *
 * `counts.needs` (src/bridge/useVm.ts `computeCounts`) is a single
 * `sessions.filter(s => s.state === "Needs").length` — since every one of
 * these cases sets `Phase::Needs` in the reducer, this SHOULD already be
 * correct by construction. This suite exists to pin that correctness
 * explicitly with a mix that was never exercised before (MOCK_SNAPSHOT has
 * no Question-gated session, and no gate-less Notification-style Needs
 * session), rather than assume it from the Permission/Plan-only coverage in
 * the pre-existing Overview.test.tsx.
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Overview } from "../src/screens/Overview";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

// ---------------------------------------------------------------------------
// Fixture — a clean slate (no gates) plus one of each "needs you" flavor.
// ---------------------------------------------------------------------------

function baseSession(overrides: Partial<SessionView>): SessionView {
  return {
    ...MOCK_SNAPSHOT.sessions[3], // Working, gate: null template
    ...overrides,
  };
}

const PERMISSION_NEEDS = baseSession({
  id: "n1",
  name: "perm-needs",
  state: "Needs",
  gate: {
    Permission: {
      request_id: "r1",
      command: "rm -rf build/",
      cwd: "/app",
      risk: "",
      explanation: "Clean build output",
    },
  },
});

const PLAN_NEEDS = baseSession({
  id: "n2",
  name: "plan-needs",
  state: "Needs",
  gate: { Plan: { summary: "Refactor the auth module", steps: [] } },
});

const QUESTION_NEEDS = baseSession({
  id: "n3",
  name: "question-needs",
  state: "Needs",
  gate: {
    Question: {
      request_id: "r3",
      questions: [
        {
          question: "Which environment?",
          header: "",
          multi_select: false,
          options: [],
        },
      ],
    },
  },
});

// A bare Notification — HookEvent::Notification sets Phase::Needs without
// ever touching `gate`, so this session Needs attention with `gate: null`.
const NOTIFICATION_NEEDS = baseSession({
  id: "n4",
  name: "notification-needs",
  state: "Needs",
  gate: null,
});

const WORKING_NOT_NEEDS = baseSession({
  id: "n5",
  name: "still-working",
  state: "Working",
  gate: null,
});

function renderOverview(sessions: SessionView[]) {
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

describe("U3 — Overview 'Needs you' count covers every waiting flavor", () => {
  it("counts permission + plan + question + bare-notification sessions, not just permission/plan", () => {
    renderOverview([
      PERMISSION_NEEDS,
      PLAN_NEEDS,
      QUESTION_NEEDS,
      NOTIFICATION_NEEDS,
      WORKING_NOT_NEEDS,
    ]);
    expect(screen.getByText("Needs you")).toBeTruthy();
    const needsTile = screen.getByText("Needs you").closest("div")!.parentElement!;
    expect(needsTile.textContent).toContain("4");
  });

  it("every needs-you session gets its own attention card, including the question and bare-notification ones", () => {
    renderOverview([
      PERMISSION_NEEDS,
      PLAN_NEEDS,
      QUESTION_NEEDS,
      NOTIFICATION_NEEDS,
      WORKING_NOT_NEEDS,
    ]);
    expect(screen.getByTestId("attention-card-n1")).toBeTruthy();
    expect(screen.getByTestId("attention-card-n2")).toBeTruthy();
    expect(screen.getByTestId("attention-card-n3")).toBeTruthy();
    expect(screen.getByTestId("attention-card-n4")).toBeTruthy();
    expect(screen.queryByTestId("attention-card-n5")).toBeNull();
  });

  it("the bare-notification card (no gate) still surfaces SOME reason it needs attention, not a blank gate box", () => {
    renderOverview([NOTIFICATION_NEEDS]);
    const card = screen.getByTestId("attention-card-n4");
    // Today gateKindLabel()/gateLead() fall back to the literal "Gate" for a
    // null kind on a non-Error Needs session, and the summary box renders
    // the bare placeholder "—" — neither tells the human anything about
    // *why* the session needs them (a claude Notification, not a gate).
    expect(card.textContent).not.toContain("—");
  });
});
