/**
 * GateOverlay — multiple-question gate answered through the unified GateOptions
 * renderer. Guards the refactor that routes question options through the shared
 * stacked-row component: with >1 question, each renders its own option list,
 * selections accumulate (no immediate fire), and Submit sends every answer.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { GateOverlay } from "../src/screens/GateOverlay";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView } from "../src/bridge/types";

vi.mock("../src/bridge/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/bridge/api")>();
  return {
    ...original,
    answerGate: vi.fn().mockResolvedValue(undefined),
    answerQuestion: vi.fn().mockResolvedValue(undefined),
  };
});

import * as api from "../src/bridge/api";

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.restoreAllMocks());

const TWO_QUESTIONS: SessionView = {
  ...MOCK_SNAPSHOT.sessions[0],
  id: "sQ2",
  name: "multi-q",
  state: "Needs",
  gate: {
    Question: {
      request_id: "req-multi",
      questions: [
        {
          question: "Which database?",
          header: "Storage",
          multi_select: false,
          options: [
            { label: "Postgres", description: "relational" },
            { label: "MySQL", description: "relational" },
          ],
        },
        {
          question: "Which cache?",
          header: "Cache",
          multi_select: false,
          options: [
            { label: "Redis", description: "in-memory" },
            { label: "Memcached", description: "in-memory" },
          ],
        },
      ],
    },
  },
} as SessionView;

function renderGate(session: SessionView) {
  const snapshot: AppSnapshot = {
    ...MOCK_SNAPSHOT,
    sessions: [...MOCK_SNAPSHOT.sessions, session],
  };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "gate",
    selectedId: session.id,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <GateOverlay />
      </VmProvider>
    </ThemeProvider>
  );
}

describe("GateOverlay — multiple questions", () => {
  it("renders every question and its options", () => {
    renderGate(TWO_QUESTIONS);
    expect(screen.getByText("Which database?")).toBeTruthy();
    expect(screen.getByText("Which cache?")).toBeTruthy();
    expect(screen.getByText("Postgres")).toBeTruthy();
    expect(screen.getByText("Redis")).toBeTruthy();
  });

  it("accumulates a pick from each question and submits all answers together", async () => {
    renderGate(TWO_QUESTIONS);
    // With >1 question, clicking an option must NOT fire immediately.
    fireEvent.click(screen.getByText("Postgres"));
    fireEvent.click(screen.getByText("Redis"));
    expect(api.answerQuestion).not.toHaveBeenCalled();

    // Submit aggregates both answers.
    fireEvent.click(screen.getByTestId("question-submit-btn"));
    await vi.waitFor(() => {
      expect(api.answerQuestion).toHaveBeenCalledWith(
        expect.objectContaining({
          request_id: "req-multi",
          answers: { "Which database?": "Postgres", "Which cache?": "Redis" },
        })
      );
    });
  });
});
