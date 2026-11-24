import { describe, expect, it } from "vitest";
import type { SessionEvent } from "../../src/bridge/wire";
import { MAIN } from "../../src/state/scope";
import { createTranscriptFold, type Item, type ToolItem, type TurnItem, type UserItem } from "../../src/features/session/transcript";

// Cursors and times are explicit: each test says which event happened when.
const cur = (n: number) => n * 65536;
const ev = (n: number, t: number, extra: Record<string, unknown>) => ({ cursor: cur(n), t, ...extra }) as unknown as SessionEvent;
const agent = (n: number, t: number, event: Record<string, unknown>) => ev(n, t, { kind: "agent", event });
const prompt = (n: number, t: number, input: number, text: string) => ev(n, t, { kind: "input", input, command: { command: "prompt", text }, status: { status: "sent" } });
const delta = (n: number, t: number, text: string, message_id = "m1") => agent(n, t, { type: "assistant_delta", text, message_id });
const started = (n: number, t: number, call_id = "c1") => agent(n, t, { type: "tool_call_started", name: "Read", call_id, input: { file_path: "a.ts" }, is_subagent: false });
const turnStart = (n: number, t: number, trigger = "input") => agent(n, t, { type: "turn_started", trigger, input: 1 });
const turnEnd = (n: number, t: number, was_interrupted = false) => agent(n, t, { type: "turn_completed", was_interrupted, num_turns: 1, usage: {} });
const fold = (events: SessionEvent[]) => createTranscriptFold(MAIN).update(events);
const turns = (items: readonly Item[]) => items.filter((i): i is TurnItem => i.kind === "turn");

describe("span: the cursor range each item was folded from", () => {
  it("an item made by one event spans that one cursor, and its key is that cursor", () => {
    const [u] = fold([prompt(1, 10, 1, "hi")]);
    expect(u).toMatchObject({ kind: "user", key: String(cur(1)), span: { from: cur(1), to: cur(1) } });
  });

  it("merged deltas extend the span to the last delta; the key and the start stay with the first", () => {
    const items = fold([delta(2, 20, "a"), delta(3, 21, "b"), delta(4, 22, "c")]);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "assistant", text: "abc", key: String(cur(2)), span: { from: cur(2), to: cur(4) } });
  });

  it("a tool call spans from its start to the last event that changed it", () => {
    const items = fold([started(5, 50), agent(6, 60, { type: "tool_call_updated", call_id: "c1", status: "running" }), agent(7, 70, { type: "tool_call_finished", call_id: "c1", ok: true, result: "x" })]);
    expect(items[0]).toMatchObject({ kind: "tool", done: true, span: { from: cur(5), to: cur(7) } });
  });

  it("an event that changes nothing in an item does not move its span", () => {
    const items = fold([started(5, 50), agent(6, 60, { type: "tool_call_updated", call_id: "c1" })]);
    expect(items[0].span).toEqual({ from: cur(5), to: cur(5) });
  });
});

describe("at: when a live item happened", () => {
  it("a live prompt, a live response and a live tool call take the time of their event", () => {
    const items = fold([prompt(1, 1_000, 1, "hi"), delta(2, 2_000, "first"), delta(3, 2_500, " second"), started(4, 3_000)]);
    expect(items[0]).toMatchObject({ kind: "user", at: 1_000 });
    // A merged response keeps the time of its first delta.
    expect(items[1]).toMatchObject({ kind: "assistant", at: 2_000, text: "first second" });
    expect(items[2]).toMatchObject({ kind: "tool", at: 3_000 });
  });

  it("an item of the earlier history takes the time the conversation gives, and none when it gives none", () => {
    const history = (n: number, t: number, at_ms: number | undefined, event: Record<string, unknown>) =>
      ev(n, t, { kind: "agent", event: { type: "history", at_ms, event } });
    const items = createTranscriptFold(MAIN, "earlier").update([
      history(1, 9_000_000, 1_234, { type: "user_message", text: "imported" }),
      history(2, 9_000_000, undefined, { type: "tool_call_started", name: "Read", call_id: "h", input: {}, is_subagent: false }),
    ]);
    expect(items[0]).toMatchObject({ kind: "user", at: 1_234 });
    expect(items[1]).toMatchObject({ kind: "tool" });
    expect((items[1] as ToolItem).at).toBeUndefined();
  });
});

describe("turn item: the end of a turn", () => {
  it("is appended at turn_completed, keyed by its cursor, after what the turn said", () => {
    const items = fold([turnStart(1, 1_000), prompt(2, 1_001, 1, "go"), delta(3, 2_000, "done"), turnEnd(4, 43_000)]);
    const last = items[items.length - 1];
    expect(last).toMatchObject({ kind: "turn", key: String(cur(4)), duration: 42_000, interrupted: false, span: { from: cur(4), to: cur(4) } });
    expect(items.slice(0, -1).map((i) => i.kind)).toEqual(["user", "assistant"]);
  });

  it("says it was interrupted when the turn was, and how long it ran", () => {
    expect(turns(fold([turnStart(1, 1_000), turnEnd(2, 13_000, true)]))).toMatchObject([{ duration: 12_000, interrupted: true }]);
  });

  it("measures each turn from its own start", () => {
    const items = fold([turnStart(1, 1_000), turnEnd(2, 5_000), turnStart(3, 10_000), turnEnd(4, 12_000)]);
    expect(turns(items).map((t) => t.duration)).toEqual([4_000, 2_000]);
  });

  it("has no duration when its start was not seen, and is still an end", () => {
    const [t] = turns(fold([turnEnd(1, 5_000)]));
    expect(t.duration).toBeUndefined();
    expect(t).toMatchObject({ kind: "turn", interrupted: false });
  });

  it("the earlier history has no turn items: its own turn ends are not shown as turns", () => {
    const items = fold([turnStart(1, 1_000), turnEnd(2, 2_000)]);
    expect(turns(items)).toHaveLength(1);
    expect(createTranscriptFold(MAIN, "earlier").update([ev(3, 3_000, { kind: "agent", event: { type: "history", event: { type: "turn_completed", was_interrupted: false, num_turns: 1, usage: {} } } })])).toEqual([]);
  });
});

describe("identity with spans", () => {
  it("items that an older window and the whole stream share are the same objects", () => {
    const events = [prompt(1, 10, 1, "a"), delta(2, 20, "x"), delta(3, 21, "y"), started(4, 30), agent(5, 31, { type: "tool_call_finished", call_id: "c1", ok: true, result: "r" }), turnEnd(6, 40)];
    const f = createTranscriptFold(MAIN);
    const tail = f.update(events.slice(1));
    const whole = f.update(events);
    expect(whole.length).toBe(tail.length + 1);
    whole.slice(1).forEach((it, i) => expect(it).toBe(tail[i]));
    expect((whole[1] as Item).span).toEqual({ from: cur(2), to: cur(3) });
  });
});

describe("typed items", () => {
  it("a user item and a tool item carry their span and time together", () => {
    const [u, t] = fold([prompt(1, 5, 1, "x"), started(2, 6)]);
    expect(u as UserItem).toMatchObject({ at: 5, span: { from: cur(1), to: cur(1) } });
    expect(t as ToolItem).toMatchObject({ at: 6, span: { from: cur(2), to: cur(2) } });
  });
});
