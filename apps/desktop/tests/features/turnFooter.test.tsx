import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SessionEvent } from "../../src/bridge/wire";
import { TranscriptItem } from "../../src/features/session/TranscriptView";
import { groupToolRuns, type RowItem } from "../../src/features/session/toolRuns";
import { createTranscriptFold } from "../../src/features/session/transcript";
import { dayTime } from "../../src/lib/format";
import { MAIN } from "../../src/state/scope";

let n = 0;
const ev = (t: number, extra: Record<string, unknown>) => ({ cursor: ++n * 65536, t, ...extra }) as unknown as SessionEvent;
const agent = (t: number, event: Record<string, unknown>) => ev(t, { kind: "agent", event });

/** Folds events and draws every row, the way a transcript does. */
function rowsOf(events: SessionEvent[]): RowItem[] {
  return groupToolRuns(createTranscriptFold(MAIN).update(events));
}
const footers = () => screen.queryAllByTestId("turn-footer").map((f) => f.textContent);

describe("turn footer", () => {
  it("says how long a finished turn took, after what the turn said", () => {
    const rows = rowsOf([
      ev(1_000, { kind: "input", input: 1, command: { command: "prompt", text: "go" }, status: { status: "sent" } }),
      agent(1_100, { type: "turn_started", trigger: "input", input: 1 }),
      agent(2_000, { type: "assistant_delta", text: "All done.", message_id: "m" }),
      agent(43_100, { type: "turn_completed", was_interrupted: false, num_turns: 1, usage: {} }),
    ]);
    render(<>{rows.map((r) => <TranscriptItem key={r.key} item={r} />)}</>);
    expect(footers()).toEqual(["Done in 42s"]);
    expect(screen.getByTestId("turn-footer")).not.toHaveAttribute("data-interrupted");
    // The footer is the last thing in the turn.
    expect(screen.getByTestId("turn-footer").previousElementSibling).toHaveTextContent("All done.");
  });

  it("says an interrupted turn was interrupted, and after how long", () => {
    const rows = rowsOf([agent(1_000, { type: "turn_started", trigger: "input", input: 2 }), agent(13_000, { type: "turn_completed", was_interrupted: true, num_turns: 0, usage: {} })]);
    render(<>{rows.map((r) => <TranscriptItem key={r.key} item={r} />)}</>);
    expect(footers()).toEqual(["Interrupted after 12s"]);
    expect(screen.getByTestId("turn-footer")).toHaveAttribute("data-interrupted");
  });

  it("hides what was not seen: no duration without the start, and nothing about tokens or cost", () => {
    const rows = rowsOf([agent(5_000, { type: "turn_completed", was_interrupted: false, num_turns: 1, usage: { input_tokens: 9, cost_usd: 0.1 } })]);
    render(<>{rows.map((r) => <TranscriptItem key={r.key} item={r} />)}</>);
    expect(footers()).toEqual(["Done"]);
    expect(screen.queryByText(/\$|token/i)).toBeNull();
  });

  it("an interrupted turn with no start seen says only that it was interrupted", () => {
    const rows = rowsOf([agent(5_000, { type: "turn_completed", was_interrupted: true, num_turns: 0, usage: {} })]);
    render(<>{rows.map((r) => <TranscriptItem key={r.key} item={r} />)}</>);
    expect(footers()).toEqual(["Interrupted"]);
  });
});

describe("live messages show their time", () => {
  it("a prompt and a response that happened live carry the time of their event", () => {
    const at = Date.UTC(2026, 10, 24, 9, 15);
    const rows = rowsOf([
      ev(at, { kind: "input", input: 3, command: { command: "prompt", text: "hello" }, status: { status: "sent" } }),
      agent(at + 60_000, { type: "assistant_delta", text: "hi there", message_id: "m" }),
    ]);
    render(<>{rows.map((r) => <TranscriptItem key={r.key} item={r} />)}</>);
    const times = screen.getAllByTestId("msg-time").map((t) => t.textContent);
    expect(times[0]).toBe(dayTime(at));
    expect(times[1]).toContain(dayTime(at + 60_000));
  });
});
