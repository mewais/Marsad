import { describe, expect, it } from "vitest";
import type { SubagentInfo, TaskInfo } from "../../src/bridge/wire";
import { orderSubagents, otherBackgroundTasks, originWords, taskElapsedMs, taskEndedWords, taskStartedWords, taskState } from "../../src/features/session/tasks";
import { elapsed } from "../../src/lib/format";
import { parseRoute, sessionPath } from "../../src/router";

const sub = (call_id: string, o: Partial<SubagentInfo> = {}): SubagentInfo => ({ call_id, name: "Agent", description: "", started_at_ms: 0, running: false, tool_calls: 0, ...o });
const task = (task_id: string, o: Partial<TaskInfo> = {}): TaskInfo => ({ task_id, kind: "local_bash", background: true, started_at_ms: 0, running: true, ...o });

describe("task and subagent state", () => {
  it("is running until the agent's task ended; then finished, or the agent's own word", () => {
    expect(taskState({ running: true })).toEqual({ tone: "working", word: "Running" });
    expect(taskState({ running: false, ok: true, status: "completed" })).toEqual({ tone: "done", word: "Finished" });
    expect(taskState({ running: false, ok: false, status: "stopped" })).toEqual({ tone: "failed", word: "Stopped" });
    expect(taskState({ running: false, ok: false })).toEqual({ tone: "failed", word: "Failed" });
    expect(taskState({ running: false, status: "process_ended" })).toEqual({ tone: "done", word: "Process ended" });
    expect(taskState({ running: false })).toEqual({ tone: "done", word: "Ended" });
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
