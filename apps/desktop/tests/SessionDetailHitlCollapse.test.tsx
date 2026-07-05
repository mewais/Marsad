/**
 * Activity tab — the HITL "needs-you → resolved" collapse pattern
 * (Workstreams redesign SLICE D, docs/design/DESIGN_HANDOFF.md §"LEVEL 3 —
 * Session" / Activity tab bullet list).
 *
 * A pending question-batch/plan/permission renders as an amber glowing card
 * with a Review/Answer CTA that opens the gate overlay; once resolved it
 * collapses into a muted `▸`-expandable summary row in the transcript. This
 * is uniform across all three HITL types (this codebase also has a 4th gate
 * kind, Edit, not covered by these tests but exercised by the same code
 * path — see src/screens/tabs/Activity.tsx's EditHitl).
 *
 * The backend has no history of past gate decisions (it just clears
 * `session.gate` once answered) — so the "what was decided" half of the
 * resolved row is reconstructed from bridge/api.ts's `resolvedGateLog`,
 * which answerGate/answerQuestion append to (see api.ts's doc comment).
 * These tests call those real functions directly (not mocked) to populate
 * it, then simulate the backend clearing `session.gate` via a VM_UPDATE
 * "session" event dispatched on the live store — the same dataflow a real
 * gate resolution takes.
 */
import { describe, it, expect, beforeEach } from "vitest";
import type { ReactNode, ContextType } from "react";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { VmProvider, VmState, VmContext } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { SessionDetail } from "../src/screens/SessionDetail";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import { _resetResolvedGates, answerGate, answerQuestion } from "../src/bridge/api";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

beforeEach(() => {
  _resetResolvedGates();
});

// ---------------------------------------------------------------------------
// Render helper — captures the live VmContext so a test can dispatch a
// VM_UPDATE "session" event (simulating the backend clearing `gate` once
// answered) without remounting the provider (which would just re-seed the
// PENDING gate from initialState again).
// ---------------------------------------------------------------------------

let capturedCtx: NonNullable<ContextType<typeof VmContext>> | null = null;

function Capture({ children }: { children: ReactNode }) {
  return (
    <VmContext.Consumer>
      {(ctx) => {
        capturedCtx = ctx;
        return <>{children}</>;
      }}
    </VmContext.Consumer>
  );
}

function renderSession(selectedId: string, extraSessions: SessionView[] = []) {
  capturedCtx = null;
  const snapshot: AppSnapshot = {
    ...MOCK_SNAPSHOT,
    sessions: [...MOCK_SNAPSHOT.sessions, ...extraSessions],
    selected_id: selectedId,
  };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "session",
    selectedId,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: {},
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <Capture>
          <SessionDetail />
        </Capture>
      </VmProvider>
    </ThemeProvider>
  );
}

/** Simulate the backend resolving `session`'s gate (clearing it to null). */
function resolveGate(session: SessionView) {
  act(() => {
    capturedCtx!.dispatch({
      type: "VM_UPDATE",
      payload: { kind: "session", session: { ...session, gate: null } },
    });
  });
}

// ---------------------------------------------------------------------------
// 1. Permission gate — s01 (mockFixture): "prisma migrate deploy" in /app
// ---------------------------------------------------------------------------

describe("Activity — HITL collapse: permission", () => {
  it("shows a pending amber card with the command preview and a Review CTA", () => {
    renderSession("s01");
    expect(screen.getByText("Permission needed")).toBeTruthy();
    expect(screen.getByText(/\$ prisma migrate deploy/)).toBeTruthy();
    expect(screen.getByText("Review →")).toBeTruthy();
  });

  it("Review CTA opens the gate screen", () => {
    renderSession("s01");
    fireEvent.click(screen.getByText("Review →"));
    expect(capturedCtx!.state.screen).toBe("gate");
  });

  it("collapses into a resolved summary row (✓ approved) once approved", async () => {
    renderSession("s01");
    const s01 = MOCK_SNAPSHOT.sessions.find((s) => s.id === "s01")!;
    await answerGate({ session_id: "s01", decision_id: "req-mock-s01", allow: true });
    resolveGate(s01);

    expect(screen.queryByText("Permission needed")).toBeNull();
    expect(screen.getByText("Permission")).toBeTruthy();
    expect(screen.getByText(/prisma migrate deploy/)).toBeTruthy();
    expect(screen.getByText(/approved/)).toBeTruthy();
  });

  it("collapses into a rejected mark when denied", async () => {
    renderSession("s01");
    const s01 = MOCK_SNAPSHOT.sessions.find((s) => s.id === "s01")!;
    await answerGate({ session_id: "s01", decision_id: "req-mock-s01", allow: false });
    resolveGate(s01);

    expect(screen.getByText(/rejected/)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// 2. Plan gate — s02 (mockFixture): 3-step async-workers refactor plan
// ---------------------------------------------------------------------------

describe("Activity — HITL collapse: plan", () => {
  it("shows a pending amber card with the step count and a Review-plan CTA", () => {
    renderSession("s02");
    expect(screen.getByText("Plan mode — needs you")).toBeTruthy();
    expect(screen.getByText("3 steps")).toBeTruthy();
    expect(screen.getByText("Review plan →")).toBeTruthy();
  });

  it("collapses to 'Kept in plan mode' when not approved, expandable to the steps", async () => {
    renderSession("s02");
    const s02 = MOCK_SNAPSHOT.sessions.find((s) => s.id === "s02")!;
    await answerGate({ session_id: "s02", decision_id: "s02", allow: false });
    resolveGate(s02);

    expect(screen.queryByText("Plan mode — needs you")).toBeNull();
    expect(screen.getByText(/Kept in plan mode/)).toBeTruthy();

    // Expandable — clicking the collapsed row reveals the numbered steps.
    fireEvent.click(screen.getByText("Plan"));
    expect(screen.getByText("Extract queue interface")).toBeTruthy();
  });

  it("collapses to 'Approved — exiting plan mode' when approved", async () => {
    renderSession("s02");
    const s02 = MOCK_SNAPSHOT.sessions.find((s) => s.id === "s02")!;
    await answerGate({ session_id: "s02", decision_id: "s02", allow: true });
    resolveGate(s02);

    expect(screen.getByText(/Approved — exiting plan mode/)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// 3. Question gate — no mockFixture example carries one, so a fixture is
//    built inline (mirrors tests/U3GateKindSurfaces.test.tsx's QUESTION_SESSION).
// ---------------------------------------------------------------------------

const QUESTION_SESSION: SessionView = {
  ...MOCK_SNAPSHOT.sessions[0],
  id: "sQ1",
  name: "checkout-redesign",
  host_id: "local",
  project: "~/projects/checkout",
  branch: "feat/checkout-v2",
  state: "Needs",
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
        {
          question: "Which auth scheme?",
          header: "",
          multi_select: false,
          options: [
            { label: "OAuth2", description: "" },
            { label: "API keys", description: "" },
          ],
        },
      ],
    },
  },
};

describe("Activity — HITL collapse: questions", () => {
  it("shows a pending amber card with the question count and first question text", () => {
    renderSession("sQ1", [QUESTION_SESSION]);
    expect(screen.getByText("2 questions need you")).toBeTruthy();
    expect(screen.getByText("Use REST or GraphQL for the new checkout API?")).toBeTruthy();
    expect(screen.getByText("Answer →")).toBeTruthy();
  });

  it("Answer CTA opens the gate screen", () => {
    renderSession("sQ1", [QUESTION_SESSION]);
    fireEvent.click(screen.getByText("Answer →"));
    expect(capturedCtx!.state.screen).toBe("gate");
  });

  it("collapses to an expandable 'N answered' row with ✓ on each picked answer", async () => {
    renderSession("sQ1", [QUESTION_SESSION]);
    await answerQuestion({
      session_id: "sQ1",
      request_id: "req-q1",
      answers: {
        "Use REST or GraphQL for the new checkout API?": "GraphQL",
        "Which auth scheme?": "A custom scheme",
      },
    });
    resolveGate(QUESTION_SESSION);

    expect(screen.queryByText("2 questions need you")).toBeNull();
    expect(screen.getByText(/2 of 2 answered/)).toBeTruthy();

    // Expand — reveals every question with its picked answer marked ✓,
    // including a custom (non-listed) answer appended as its own chip.
    fireEvent.click(screen.getByText("Questions"));
    expect(screen.getByText("Use REST or GraphQL for the new checkout API?")).toBeTruthy();
    expect(screen.getByText("✓ GraphQL")).toBeTruthy();
    expect(screen.getByText("✓ A custom scheme")).toBeTruthy();
  });

  it("shows an unanswered question with no ✓ mark on any option", async () => {
    renderSession("sQ1", [QUESTION_SESSION]);
    await answerQuestion({
      session_id: "sQ1",
      request_id: "req-q1",
      answers: { "Use REST or GraphQL for the new checkout API?": "REST" },
    });
    resolveGate(QUESTION_SESSION);

    fireEvent.click(screen.getByText("Questions"));
    expect(screen.getByText("✓ REST")).toBeTruthy();
    // The second question was never answered — no picked mark anywhere for it.
    expect(screen.queryByText("✓ OAuth2")).toBeNull();
    expect(screen.queryByText("✓ API keys")).toBeNull();
  });
});
