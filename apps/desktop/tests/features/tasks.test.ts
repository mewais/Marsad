import { describe, expect, it } from "vitest";
import type { SubagentInfo, TaskInfo } from "../../src/bridge/wire";
import { NO_SUBAGENT_SUPPORT, orderSubagents, otherBackgroundTasks, originWords, subagentFacts, subagentState, taskEndedWords, taskStartedWords } from "../../src/features/session/tasks";
import { taskElapsedMs } from "../../src/ui/Elapsed";
import { elapsed } from "../../src/lib/format";
import { parseRoute, sessionPath } from "../../src/router";

const sub = (call_id: string, o: Partial<SubagentInfo> = {}): SubagentInfo => ({ call_id, name: "Agent", description: "", started_at_ms: 0, running: false, tool_calls: 0, ...o });
const task = (task_id: string, o: Partial<TaskInfo> = {}): TaskInfo => ({ task_id, kind: "local_bash", background: true, started_at_ms: 0, running: true, ...o });

describe("task and subagent state", () => {
  it("is Working while the agent's task runs; then its outcome, in the shared vocabulary", () => {
    expect(subagentState({ running: true })).toEqual({ status: "working", word: "Working" });
    expect(subagentState({ running: false, outcome: "completed" })).toEqual({ status: "done", word: "Finished" });
    expect(subagentState({ running: false, outcome: "failed" })).toEqual({ status: "failed", word: "Failed" });
    expect(subagentState({ running: false, outcome: "stopped" })).toEqual({ status: "ended", word: "Stopped" });
    expect(subagentState({ running: false, outcome: "lost" })).toEqual({ status: "failed", word: "Lost", why: "The agent process ended while it ran." });
    expect(subagentState({ running: false, outcome: "unknown" })).toEqual({ status: "idle", word: "Unknown", why: "State not available." });
  });

  it("an ended subagent with no outcome says what it can: its ok flag, else that the state is not available", () => {
    expect(subagentState({ running: false, ok: true })).toEqual({ status: "done", word: "Finished" });
    expect(subagentState({ running: false, ok: false })).toEqual({ status: "failed", word: "Failed" });
    expect(subagentState({ running: false, outcome: null, ok: null })).toEqual({ status: "idle", word: "Unknown", why: "State not available." });
    // A word the agent chose is not shown as a state of its own.
    expect(subagentState({ running: false })).toEqual({ status: "idle", word: "Unknown", why: "State not available." });
  });

  it("a subagent's facts are shown only as the capabilities report them: never a zero for an unreported figure", () => {
    const usage = { total_tokens: 12_400, tool_uses: 3, duration_ms: 1000, cost_usd: 0.42 };
    const reported = { ...NO_SUBAGENT_SUPPORT, live_state: true, usage: true };
    expect(subagentFacts({ tool_calls: 3, usage }, reported)).toEqual(["3 tool calls", "12k tokens", "$0.42"]);
    expect(subagentFacts({ tool_calls: 3, usage }, { ...NO_SUBAGENT_SUPPORT, live_state: true })).toEqual(["3 tool calls"]);
    expect(subagentFacts({ tool_calls: 3, usage }, { ...NO_SUBAGENT_SUPPORT, usage: true })).toEqual(["12k tokens", "$0.42"]);
    expect(subagentFacts({ tool_calls: 0, usage: null }, reported)).toEqual(["0 tool calls"]);
    expect(subagentFacts({ tool_calls: 0, usage: null }, NO_SUBAGENT_SUPPORT)).toEqual([]);
    expect(subagentFacts({ tool_calls: 2, usage: { ...usage, cost_usd: null } }, reported)).toEqual(["2 tool calls", "12k tokens"]);
  });

  it("elapsed time runs to now while it runs and stops when it ended", () => {
    expect(taskElapsedMs({ started_at_ms: 1000, running: true }, 42_000)).toBe(41_000);
    expect(taskElapsedMs({ started_at_ms: 1000, ended_at_ms: 6000, running: false }, 42_000)).toBe(5000);
    expect([0, 41_000, 185_000, 3_725_000].map(elapsed)).toEqual(["0s", "41s", "3m 05s", "1h 02m"]);
  });
});

describe("the subagents list", () => {
  it("running ones first, then finished; each newest first", () => {
    const list = [sub("old", { ended_at_ms: 10, started_at_ms: 1 }), sub("run-old", { running: true, started_at_ms: 5 }), sub("new", { ended_at_ms: 30, started_at_ms: 2 }), sub("run-new", { running: true, started_at_ms: 9 }), sub("mid", { ended_at_ms: 20, started_at_ms: 3 })];
    expect(orderSubagents(list).map((s) => s.call_id)).toEqual(["run-new", "run-old", "new", "mid", "old"]);
    // The daemon's list is not changed.
    expect(list[0].call_id).toBe("old");
  });

  it("background tasks that are not subagents: running ones the agent runs in the background, newest first", () => {
    const subagents = [sub("c1", { task_id: "a" }), sub("c2")];
    const tasks = [task("a"), task("b", { started_at_ms: 1 }), task("c", { started_at_ms: 2 }), task("d", { running: false }), task("e", { background: false }), task("f", { call_id: "c2" })];
    expect(otherBackgroundTasks({ subagents, tasks }).map((t) => t.task_id)).toEqual(["c", "b"]);
  });
});

describe("the words in the transcript", () => {
  it("say what the agent said, as it said it", () => {
    expect(taskStartedWords({ kind: "local_bash", description: "pnpm test", background: true })).toBe("Background task started · local_bash · “pnpm test”");
    expect(taskStartedWords({ kind: "local_agent", background: false })).toBe("Task started · local_agent");
    expect(taskEndedWords({ kind: "local_bash", description: "pnpm test", background: true }, { status: "completed", summary: "52 passed" })).toBe("Background task ended · local_bash · “pnpm test” · completed — 52 passed");
    expect(taskEndedWords(undefined, { status: "killed" })).toBe("A task ended · killed");
    expect(originWords({ kind: "task-notification", producer: "session-task" })).toBe("origin: task-notification (session-task)");
    expect(originWords({ kind: "task-notification" }, { kind: "local_bash", description: "pnpm test" })).toBe("origin: task-notification · pnpm test");
  });
});

describe("a subagent's address", () => {
  it("names the session and the subagent, and reads back", () => {
    expect(sessionPath("x1", "abc", undefined, "toolu_1")).toBe("/session/x1/abc?sub=toolu_1");
    expect(parseRoute("/session/x1/abc?sub=toolu_1")).toEqual({ name: "session", host: "x1", id: "abc", tab: "activity", sub: "toolu_1" });
    expect(parseRoute("/session/x1/abc")).toEqual({ name: "session", host: "x1", id: "abc", tab: "activity" });
    expect(sessionPath("x1", "abc", "changes")).toBe("/session/x1/abc?tab=changes");
    expect(sessionPath("x1", "abc")).toBe("/session/x1/abc");
    // A subagent has no place in another section.
    expect(sessionPath("x1", "abc", "config", "toolu_1")).toBe("/session/x1/abc?tab=config");
  });
});
