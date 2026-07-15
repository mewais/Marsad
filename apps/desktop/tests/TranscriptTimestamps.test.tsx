/**
 * Transcript timestamps — day separators, idle-gap markers, and per-turn times.
 *
 * Per-entry `at` (epoch ms) drives: a day divider when the calendar day changes,
 * a faint time marker on a long idle gap, and a muted time below a user prompt /
 * below the last entry of an assistant turn. No timestamps → none of these (so
 * older sessions render exactly as before).
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VmProvider, VmState } from "../src/bridge/useVm";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { SessionDetail } from "../src/screens/SessionDetail";
import { MOCK_SNAPSHOT } from "../src/bridge/mockFixture";
import type { AppSnapshot, SessionView, TranscriptEntry } from "../src/bridge/types";

function renderTranscript(transcript: TranscriptEntry[]) {
  const base = MOCK_SNAPSHOT.sessions[0];
  const session: SessionView = { ...base, transcript };
  const snapshot: AppSnapshot = {
    ...MOCK_SNAPSHOT,
    sessions: [session, ...MOCK_SNAPSHOT.sessions.slice(1)],
    selected_id: session.id,
  };
  const preloaded: Partial<VmState> = {
    snapshot,
    screen: "session",
    selectedId: session.id,
    tab: "Activity",
    filter: "All",
    search: "",
    wsMenuOpen: false,
    capabilities: {},
  };
  return render(
    <ThemeProvider initialTheme="dark">
      <VmProvider initialState={preloaded}>
        <SessionDetail />
      </VmProvider>
    </ThemeProvider>
  );
}

const DAY1 = Date.UTC(2026, 6, 9, 12, 0, 0); // Thu Jul 9 2026, midday UTC

describe("transcript timestamps", () => {
  it("draws day separators + per-turn times across a multi-day conversation", () => {
    renderTranscript([
      { role: "user", text: "q1", at: DAY1 },
      { role: "assistant", text: "a1", at: DAY1 + 5 * 60 * 1000 },
      { role: "user", text: "q2", at: DAY1 + 26 * 60 * 60 * 1000 }, // next local day
    ]);
    // A leading day header + one when the day changes → at least two dividers.
    expect(
      screen.getAllByTestId("transcript-day-separator").length
    ).toBeGreaterThanOrEqual(2);
    // A time below: user q1, the assistant turn (a1), and user q2 → ≥ 3.
    expect(
      screen.getAllByTestId("transcript-turn-time").length
    ).toBeGreaterThanOrEqual(3);
  });

  it("shows an idle-gap marker for a long gap within the same day", () => {
    renderTranscript([
      { role: "user", text: "q1", at: DAY1 },
      { role: "assistant", text: "a1", at: DAY1 + 45 * 60 * 1000 }, // 45 min gap
    ]);
    expect(screen.getByTestId("transcript-gap-marker")).toBeTruthy();
  });

  it("collapses an assistant run to ONE trailing time (not per fragment)", () => {
    renderTranscript([
      { role: "user", text: "go", at: DAY1 },
      { role: "assistant", text: "step 1", at: DAY1 + 1000 },
      { role: "tool", name: "Bash", input: "{}", at: DAY1 + 2000 },
      { role: "assistant", text: "step 2", at: DAY1 + 3000 },
    ]);
    // One time for the user prompt + one for the whole assistant run = 2 (not 4).
    expect(screen.getAllByTestId("transcript-turn-time")).toHaveLength(2);
  });

  it("renders no separators/times when entries have no timestamps", () => {
    renderTranscript([
      { role: "user", text: "q1" },
      { role: "assistant", text: "a1" },
    ]);
    expect(screen.queryByTestId("transcript-day-separator")).toBeNull();
    expect(screen.queryByTestId("transcript-gap-marker")).toBeNull();
    expect(screen.queryByTestId("transcript-turn-time")).toBeNull();
  });
});
