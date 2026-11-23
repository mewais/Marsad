import { describe, expect, it } from "vitest";
import type { HistoryScope, SessionEvent } from "../../src/bridge/wire";
import { historyAdmits } from "../../src/bridge/fixture/work";
import { createTranscriptFold, type Item } from "../../src/features/session/transcript";
import { groupToolRuns } from "../../src/features/session/toolRuns";
import { admits, MAIN, scopeKey, subagentScope } from "../../src/state/scope";

let n = 0;
const ev = (e: Record<string, unknown>) => ({ cursor: ++n * 65536, t: n * 1000, ...e }) as unknown as SessionEvent;
const agent = (event: unknown) => ev({ kind: "agent", event });
const scoped = (call: string, event: unknown, agent_id = "ag") => agent({ type: "scoped", scope: { call_id: call, agent_id }, event });
const text = (t: string, id = "m") => ({ type: "assistant_delta", text: t, message_id: id });
const kinds = (items: readonly Item[]) => items.map((i) => (i.kind === "notice" ? `notice:${i.text}` : i.kind === "assistant" ? `assistant:${i.text}` : i.kind));

const started = (id: string, background: boolean, extra: Record<string, unknown> = {}) => agent({ type: "task_started", task_id: id, kind: "local_agent", call_id: `call-${id}`, description: `desc ${id}`, background, run_id: `run-${id}`, ...extra });
const ended = (id: string, status = "completed", ok = status === "completed", summary?: string, call = `call-${id}`) => agent({ type: "task_ended", task_id: id, call_id: call, status, ok, summary });

describe("whose transcript an event belongs to", () => {
  const events: SessionEvent[] = [
    ev({ kind: "input", input: 1, command: { command: "prompt", text: "go" }, status: { status: "sent" } }),
    ev({ kind: "phase", phase: { state: "working" }, cause: { kind: "other" } }),
    agent(text("own")),
    scoped("c1", text("sub1")),
    scoped("c2", text("sub2")),
    started("a", true, { call_id: "c1" }),
    ended("a", "completed", true, undefined, "c1"),
    agent({ type: "task_started", task_id: "x", kind: "local_bash", background: true }),
    agent({ type: "task_ended", task_id: "x", status: "completed", ok: true }),
    agent({ type: "turn_completed", was_interrupted: false, num_turns: 1, usage: {}, reply: "r" }),
  ];
  const scopes: HistoryScope[] = [{ kind: "all" }, MAIN, subagentScope("c1"), subagentScope("c2"), subagentScope("none")];

  it("the live filter is the daemon's rule for a history page, event by event", () => {
    for (const scope of scopes) {
      if (scope.kind === "all") continue;
      expect(events.map((e) => admits(scope, e)), JSON.stringify(scope)).toEqual(events.map((e) => historyAdmits(scope, e)));
    }
  });

  it("the session's own transcript has everything but what subagents said; a subagent's has what is scoped to it and its task", () => {
    expect(events.filter((e) => admits(MAIN, e))).toHaveLength(events.length - 2);
    expect(events.filter((e) => admits(subagentScope("c1"), e))).toHaveLength(3);
    // An agent record in a shape this build does not know is the session's own, never a subagent's.
    const odd = ev({ kind: "agent" });
    expect([admits(MAIN, odd), admits(subagentScope("c1"), odd)]).toEqual([true, false]);
    expect(events.filter((e) => admits(subagentScope("c2"), e))).toHaveLength(1);
  });

  it("each transcript has its own key in the store", () => {
    expect(scopeKey("x1/abc", MAIN)).toBe("x1/abc");
    expect(scopeKey("x1/abc", subagentScope("c1"))).toBe("x1/abc#c1");
  });
});

describe("the fold of the session's own transcript", () => {
  it("shows nothing a subagent said, as text, thinking or tool rows", () => {
    const items = createTranscriptFold().update([
      agent(text("hello")),
      scoped("c1", text("sub text")),
      scoped("c1", { type: "thinking_delta", text: "sub thought" }),
      scoped("c1", { type: "tool_call_started", name: "Read", call_id: "t1", input: {}, is_subagent: false }),
      scoped("c1", { type: "tool_call_finished", name: "Read", call_id: "t1", ok: true, result: "x" }),
      agent(text("world", "m2")),
    ]);
    expect(kinds(items)).toEqual(["assistant:hello", "assistant:world"]);
  });

  it("says where a background task started and ended, by the agent's words; work inside a turn is its tool call's", () => {
    const items = createTranscriptFold().update([
      started("bg", true),
      started("fg", false),
      ended("fg"),
      ended("bg", "failed", false, "exit code 2"),
    ]);
    expect(kinds(items)).toEqual([
      "notice:Background task started · local_agent · “desc bg”",
      "notice:Background task ended · local_agent · “desc bg” · failed — exit code 2",
    ]);
    expect(items[1]).toMatchObject({ tone: "warn" });
    expect(items[0]).toMatchObject({ tone: "info" });
  });

  it("a task that ended whose start is not in the window is still said, without claiming it was background", () => {
    expect(kinds(createTranscriptFold().update([ended("old", "completed", true, "done")]))).toEqual(["notice:A task ended · completed — done"]);
  });

  it("a turn the agent started by itself is noticed; its end adds why when the agent says, naming the task it answers when known", () => {
    const f = createTranscriptFold();
    const a = [started("bg", true), agent({ type: "turn_started", trigger: "agent" })];
    expect(kinds(f.update(a)).slice(-1)[0]).toBe("notice:The agent started a turn on its own");
    const b = [...a, agent(text("done")), agent({ type: "turn_completed", was_interrupted: false, num_turns: 1, usage: {}, reply: "done", origin: { kind: "task-notification", producer: "session-task", run_id: "run-bg" } })];
    expect(kinds(f.update(b))).toEqual([
      "notice:Background task started · local_agent · “desc bg”",
      "notice:The agent started a turn on its own · origin: task-notification (session-task) · desc bg",
      "assistant:done",
    ]);
  });

  it("an origin the agent gives without a run we know is shown as it is; a turn that ends without one keeps the plain notice", () => {
    const own = (origin?: unknown) => kinds(createTranscriptFold().update([agent({ type: "turn_started", trigger: "agent" }), agent({ type: "turn_completed", was_interrupted: false, num_turns: 1, usage: {}, origin })]));
    expect(own({ kind: "task-notification", run_id: "unknown" })).toEqual(["notice:The agent started a turn on its own · origin: task-notification"]);
    expect(own()).toEqual(["notice:The agent started a turn on its own"]);
  });

  it("a turn for one of our inputs, or one the agent did not say what started, gets no notice, and an origin never lands on a later turn", () => {
    const f = createTranscriptFold();
    const items = f.update([
      agent({ type: "turn_started", trigger: "input", input: 3 }),
      agent({ type: "turn_completed", was_interrupted: false, num_turns: 1, usage: {}, origin: { kind: "stray" } }),
      agent({ type: "turn_started", trigger: "unreported" }),
      agent({ type: "turn_completed", was_interrupted: false, num_turns: 1, usage: {} }),
    ]);
    expect(items).toEqual([]);
  });

  it("a subagent's lines and other subagents' tasks leave the rows that were already there the same objects", () => {
    const f = createTranscriptFold();
    const first = [agent(text("a")), started("bg", true)];
    const r1 = f.update(first);
    const r2 = f.update([...first, scoped("c9", text("other")), started("other", false, { call_id: "c9" })]);
    expect(r2).toBe(r1);
  });
});

describe("the fold of a subagent's transcript", () => {
  const sub = subagentScope("c1");
  const events = [
    agent(text("main text")),
    started("a", true, { call_id: "c1", description: "Audit", subagent_type: "Explore" }),
    scoped("c1", text("I will read the files.")),
    scoped("c2", text("someone else")),
    scoped("c1", { type: "tool_call_started", name: "Read", call_id: "r1", input: { file_path: "a.ts" }, is_subagent: false }),
    scoped("c1", { type: "tool_call_finished", name: "Read", call_id: "r1", ok: true, result: "contents" }),
    scoped("c1", { type: "tool_call_started", name: "Task", call_id: "nested", input: { description: "dig" }, is_subagent: true }),
    ended("a", "completed", true, "All good.", "c1"),
  ];

  it("is the lines scoped to it, as the transcript of a session, with its task's start and end", () => {
    const items = createTranscriptFold(sub).update(events);
    expect(kinds(items)).toEqual(["notice:Background task started · local_agent · “Audit”", "assistant:I will read the files.", "tool", "tool", "notice:Background task ended · local_agent · “Audit” · completed — All good."]);
    expect(items[2]).toMatchObject({ kind: "tool", callId: "r1", done: true, ok: true, result: "contents", subagent: false });
    // A subagent it started itself is a card, the way into its own transcript.
    expect(items[3]).toMatchObject({ kind: "tool", callId: "nested", subagent: true });
    expect(groupToolRuns(items).map((r) => r.kind)).toEqual(["notice", "assistant", "tool", "tool", "notice"]);
  });

  it("takes only its own: the session's lines and the other subagents' are not in it", () => {
    expect(kinds(createTranscriptFold(subagentScope("c2")).update(events))).toEqual(["assistant:someone else"]);
    expect(createTranscriptFold(subagentScope("none")).update(events)).toEqual([]);
  });

  it("grows live: a text delta of the same message extends the item, the rest keep their identity", () => {
    const f = createTranscriptFold(sub);
    const a = [scoped("c1", text("one ", "m")), scoped("c1", { type: "tool_call_started", name: "Read", call_id: "r1", input: {}, is_subagent: false })];
    const r1 = f.update(a);
    const r2 = f.update([...a, scoped("c1", { type: "tool_call_finished", name: "Read", call_id: "r1", ok: true, result: "x" })]);
    expect(r2[0]).toBe(r1[0]);
    expect(r2[1]).not.toBe(r1[1]);
  });
});
