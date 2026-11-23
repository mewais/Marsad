import { describe, expect, it } from "vitest";
import type { SessionEvent } from "../../src/bridge/wire";
import { historyAdmits } from "../../src/bridge/fixture/work";
import { promptsOf } from "../../src/features/session/promptHistory";
import { createTranscriptFold, type Item } from "../../src/features/session/transcript";
import { admits, MAIN, subagentScope } from "../../src/state/scope";

let n = 0;
const ev = (e: Record<string, unknown>) => ({ cursor: ++n * 65536, t: 1_000_000 + n, ...e }) as unknown as SessionEvent;
/** A history item: an event of the conversation before this session, with the time the conversation says. */
const hist = (event: unknown, at_ms?: number) => ev({ kind: "agent", event: { type: "history", at_ms, event } });
const live = (event: unknown) => ev({ kind: "agent", event });
const earlierFold = (events: SessionEvent[], scope = MAIN) => createTranscriptFold(scope, "earlier").update(events);
const liveFold = (events: SessionEvent[], scope = MAIN) => createTranscriptFold(scope, "live").update(events);
const kinds = (items: readonly Item[]) => items.map((i) => (i.kind === "notice" ? `notice:${i.text}` : i.kind));

const AT = Date.UTC(2026, 10, 20, 14, 2);

describe("the fold of the earlier history", () => {
  it("makes the same rows as the live one: the user's messages, assistant text, thinking and tool calls with their results", () => {
    const items = earlierFold([
      hist({ type: "user_message", text: "Fix the **rounding**" }, AT),
      hist({ type: "thinking_delta", text: "hm" }, AT + 1000),
      hist({ type: "assistant_delta", text: "Looking.", message_id: "a" }, AT + 2000),
      hist({ type: "assistant_delta", text: " Done.", message_id: "a" }, AT + 3000),
      hist({ type: "tool_call_started", name: "Read", call_id: "c1", input: { file_path: "a.ts" }, is_subagent: false }, AT + 4000),
      hist({ type: "tool_call_finished", name: "Read", call_id: "c1", ok: true, result: "text" }, AT + 5000),
    ]);
    expect(kinds(items)).toEqual(["user", "thinking", "assistant", "tool"]);
    expect(items[0]).toMatchObject({ kind: "user", text: "Fix the **rounding**", delivery: { state: "sent" }, at: AT });
    expect(items[2]).toMatchObject({ kind: "assistant", text: "Looking. Done.", at: AT + 2000 });
    expect(items[3]).toMatchObject({ kind: "tool", callId: "c1", done: true, ok: true, result: "text" });
  });

  it("a history user message is never a prompt of this session: no delivery state beyond sent, never queued or not sent", () => {
    const [u] = earlierFold([hist({ type: "user_message", text: "old" })]);
    expect(u).toMatchObject({ kind: "user", delivery: { state: "sent" } });
    expect((u as { input?: number }).input).toBeUndefined();
  });

  it("an item without a time has none: the import time (the event's `t`) is never shown in its place", () => {
    const [u, a] = earlierFold([hist({ type: "user_message", text: "old" }), hist({ type: "assistant_delta", text: "answer", message_id: "x" })]);
    expect((u as { at?: number }).at).toBeUndefined();
    expect((a as { at?: number }).at).toBeUndefined();
  });

  it("images and files of a message, notices and compactions", () => {
    const items = earlierFold([
      hist({ type: "user_content", parts: [{ type: "image", source: { source: "path", path: "x.png" }, mime: "image/png" }] }),
      hist({ type: "notice", message: "Older messages exist and are not shown" }),
      hist({ type: "compaction", info: { trigger: "auto", pre_tokens: 1000, post_tokens: 100, cumulative_dropped_tokens: 900, duration_ms: 1000 } }, AT),
      hist({ type: "compaction", info: { trigger: "manual", pre_tokens: 1000, post_tokens: 100, cumulative_dropped_tokens: 900, duration_ms: 1000 } }),
    ]);
    expect(kinds(items)).toEqual(["media", "notice:Older messages exist and are not shown", expect.stringMatching(/^notice:History compacted at \d\d:\d\d \(auto\)/), "notice:History compacted (manual): 1k → 100 tokens in 1.0s · 900 dropped so far"]);
  });

  it("a compaction says when it happened by the conversation's time, never by when it was imported", () => {
    const [n1] = earlierFold([hist({ type: "compaction", info: { trigger: "auto", pre_tokens: 5, post_tokens: 1, cumulative_dropped_tokens: 4, duration_ms: 1 } }, AT)]);
    const d = new Date(AT);
    expect((n1 as { text: string }).text).toContain(`at ${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`);
  });

  it("holds nothing but the conversation's own messages: what a history item should never carry does not touch the transcript", () => {
    const items = earlierFold([
      hist({ type: "gate_requested", gate: { gate_type: "permission", request_id: "g" } }),
      hist({ type: "gate_resolved", request_id: "g", reason: "turn_ended" }),
      hist({ type: "turn_started", trigger: "agent" }),
      hist({ type: "task_started", task_id: "t", kind: "local_bash", background: true }),
      hist({ type: "rate_limit_notice", info: { status: "x", resets_at: 1, rate_limit_type: "y", is_using_overage: false } }),
      hist({ type: "user_message", text: "only this" }),
    ]);
    expect(kinds(items)).toEqual(["user"]);
  });

  it("holds no live events, and the live fold holds no history items", () => {
    const events = [
      ev({ kind: "input", input: 1, command: { command: "prompt", text: "live prompt" }, status: { status: "sent" } }),
      live({ type: "assistant_delta", text: "live answer", message_id: "l" }),
      hist({ type: "user_message", text: "old prompt" }),
      hist({ type: "assistant_delta", text: "old answer", message_id: "o" }),
    ];
    expect(kinds(earlierFold(events))).toEqual(["user", "assistant"]);
    expect(earlierFold(events).map((i) => (i as { text: string }).text)).toEqual(["old prompt", "old answer"]);
    expect(liveFold(events).map((i) => (i as { text: string }).text)).toEqual(["live prompt", "live answer"]);
  });

  it("a user message outside a history item is not a message of the session (it is only ever inside one)", () => {
    expect(liveFold([live({ type: "user_message", text: "stray" })])).toEqual([]);
  });

  it("the two lists are folded apart: an answer that ends the earlier part never grows into the live one", () => {
    const e = earlierFold([hist({ type: "assistant_delta", text: "history ends here" })]);
    const l = liveFold([live({ type: "assistant_delta", text: "and the session begins" })]);
    expect(e).toHaveLength(1);
    expect(l).toHaveLength(1);
    expect((e[0] as { text: string }).text).toBe("history ends here");
  });

  it("the main transcript's history leaves a subagent's lines out and a subagent's transcript is its own lines", () => {
    const sub = { call_id: "h-sub", agent_id: "a" };
    const events = [
      hist({ type: "user_message", text: "go" }),
      hist({ type: "tool_call_started", name: "Task", call_id: "h-sub", input: {}, is_subagent: true }),
      hist({ type: "scoped", scope: sub, event: { type: "assistant_delta", text: "from the subagent", message_id: "s" } }),
      hist({ type: "scoped", scope: { call_id: "other", agent_id: "b" }, event: { type: "assistant_delta", text: "from another", message_id: "s2" } }),
    ];
    expect(kinds(earlierFold(events, MAIN))).toEqual(["user", "tool"]);
    expect(earlierFold(events, subagentScope("h-sub")).map((i) => (i as { text?: string }).text)).toEqual(["from the subagent"]);
    // The live filter is the daemon's rule for a history page, item by item.
    for (const scope of [MAIN, subagentScope("h-sub"), subagentScope("other"), subagentScope("none")]) {
      expect(events.map((e) => admits(scope, e)), JSON.stringify(scope)).toEqual(events.map((e) => historyAdmits(scope, e)));
    }
  });

  it("items appended to the end are folded on; items prepended refold and hand back every item that did not change", () => {
    const a = hist({ type: "user_message", text: "a" });
    const b = hist({ type: "assistant_delta", text: "b", message_id: "m" });
    const c = hist({ type: "user_message", text: "c" });
    const older = hist({ type: "user_message", text: "older" });
    const f = createTranscriptFold(MAIN, "earlier");
    const first = f.update([a, b]);
    const appended = f.update([a, b, c]);
    expect(appended.slice(0, 2)).toEqual(first);
    expect(appended[0]).toBe(first[0]);
    expect(appended[1]).toBe(first[1]);
    const prepended = f.update([older, a, b, c]);
    expect(prepended.map((i) => (i as { text: string }).text)).toEqual(["older", "a", "b", "c"]);
    expect(prepended[1]).toBe(first[0]);
    expect(prepended[2]).toBe(first[1]);
    expect(prepended[3]).toBe(appended[2]);
  });
});

describe("the prompts of a conversation, for recall", () => {
  const input = (text: string) => ev({ kind: "input", input: ++n, command: { command: "prompt", text }, status: { status: "sent" } });

  it("are the earlier conversation's user messages first, then the session's own", () => {
    const earlier = [hist({ type: "user_message", text: "old 1" }), hist({ type: "assistant_delta", text: "x", message_id: "m" }), hist({ type: "user_message", text: "old 2" })];
    expect(promptsOf([input("new 1"), input("new 2")], earlier)).toEqual(["old 1", "old 2", "new 1", "new 2"]);
  });

  it("skip empty messages and repeats in a row, across the boundary too", () => {
    const earlier = [hist({ type: "user_message", text: "same" }), hist({ type: "user_message", text: "  " })];
    expect(promptsOf([input("same"), input("other")], earlier)).toEqual(["same", "other"]);
  });

  it("a history item that is not a user message is no prompt", () => {
    expect(promptsOf([], [hist({ type: "assistant_delta", text: "nope", message_id: "m" })])).toEqual([]);
  });
});
