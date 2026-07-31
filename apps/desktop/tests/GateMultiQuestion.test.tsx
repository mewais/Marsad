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

const ONE_QUESTION: SessionView = {
  ...MOCK_SNAPSHOT.sessions[0],
  id: "sQ1",
  name: "single-q",
  state: "Needs",
  gate: {
    Question: {
      request_id: "req-single",
      questions: [
        {
          question: "Which approach?",
          header: "Approach",
          multi_select: false,
          options: [
            { label: "Approach A", description: "the safe one" },
            { label: "Approach B", description: "the fast one" },
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

describe("GateOverlay — lone single-select question", () => {
  // Regression guard: a lone single-select question still fires a preset option
  // immediately (no Submit needed) — the ergonomic tap-to-answer path.
  it("fires a preset option immediately", async () => {
    renderGate(ONE_QUESTION);
    fireEvent.click(screen.getByText("Approach A"));
    await vi.waitFor(() => {
      expect(api.answerQuestion).toHaveBeenCalledWith(
        expect.objectContaining({
          request_id: "req-single",
          answers: { "Which approach?": "Approach A" },
        })
      );
    });
  });

  // THE BUG: choosing "Other…" opens a custom-answer field but, because the
  // Submit button was suppressed for a lone single-select question, a typed
  // custom answer had no way to be sent. There must be a Submit that sends it.
  it("lets a custom 'Other' answer be submitted", async () => {
    renderGate(ONE_QUESTION);
    fireEvent.click(screen.getByText("Other…"));
    fireEvent.change(screen.getByPlaceholderText("Type a custom answer"), {
      target: { value: "My own approach" },
    });
    // Before the fix this button did not exist → getByTestId throws (RED).
    fireEvent.click(screen.getByTestId("question-submit-btn"));
    await vi.waitFor(() => {
      expect(api.answerQuestion).toHaveBeenCalledWith(
        expect.objectContaining({
          request_id: "req-single",
          answers: { "Which approach?": "My own approach" },
        })
      );
    });
  });
});
