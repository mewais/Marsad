import type { AgentEvent, HistoryScope, SessionEvent, SessionSnapshot, SubagentInfo, TaskInfo } from "../wire";

/**
 * Fixture-only: what the engine's reducer makes of the events about subagents and background tasks
 * (`wasit-core` `tasks.rs`, `wasit-runtime` `core.rs`), so a fixture session's snapshot (`tasks`,
 * `background_running`, `subagents`) is always what its events say, the way the daemon's is. It is a
 * small port for the fixture's scripts, not a second source of truth: the Rust is the contract.
 */

export type Work = Pick<SessionSnapshot, "tasks" | "subagents" | "background_running">;

export const NO_WORK: Work = { tasks: [], subagents: [], background_running: 0 };

const MAX_TASKS = 100;
export const UNLISTED = "unlisted";
export const TURN_ENDED = "turn_ended";
export const PROCESS_ENDED = "process_ended";

const runningInBackground = (tasks: readonly TaskInfo[]) => tasks.filter((t) => t.running && t.background).length;

function entry(tasks: TaskInfo[], id: string, kind: string, t: number): TaskInfo {
  const found = tasks.find((x) => x.task_id === id);
  if (found) return found;
  const task: TaskInfo = { task_id: id, kind, background: false, started_at_ms: t, running: true };
  tasks.push(task);
  if (tasks.length > MAX_TASKS) tasks.splice(Math.max(0, tasks.findIndex((x) => !x.running)), 1);
  return task;
}

/** The tasks after `a` (a copy: snapshots are never mutated). */
function reduceTasks(before: readonly TaskInfo[], a: AgentEvent, t: number): TaskInfo[] {
  const tasks = before.map((x) => ({ ...x }));
  switch (a.type) {
    case "task_started": {
      const task = entry(tasks, a.task_id, a.kind, t);
      task.kind = a.kind;
      task.call_id = a.call_id ?? task.call_id;
      task.run_id = a.run_id ?? task.run_id;
      if (a.description) task.description = a.description;
      task.subagent_type = a.subagent_type ?? task.subagent_type;
      task.background ||= a.background;
      break;
    }
    case "task_progress": {
      const task = entry(tasks, a.task_id, "", t);
      task.call_id = a.call_id ?? task.call_id;
      if (a.description) task.description = a.description;
      task.last_tool = a.last_tool ?? task.last_tool;
      task.usage = a.usage ?? task.usage;
      break;
    }
    case "task_updated": {
      const task = tasks.find((x) => x.task_id === a.task_id);
      if (task && a.status) task.status = a.status;
      break;
    }
    case "task_ended": {
      const task = entry(tasks, a.task_id, "", t);
      task.call_id = a.call_id ?? task.call_id;
      task.running = false;
      task.status = a.status;
      task.ok = a.ok;
      task.ended_at_ms = t;
      task.usage = a.usage ?? task.usage;
      break;
    }
    case "background_tasks_changed": {
      for (const l of a.tasks) {
        const task = entry(tasks, l.task_id, l.kind, t);
        if (!task.running) continue;
        task.background = true;
        task.description ||= l.description ?? "";
        task.subagent_type = l.subagent_type ?? task.subagent_type;
        task.run_id = l.run_id ?? task.run_id;
      }
      for (const task of tasks) {
        if (task.running && task.background && !a.tasks.some((l) => l.task_id === task.task_id)) {
          task.running = false;
          task.status ??= UNLISTED;
          task.ended_at_ms = t;
        }
      }
      break;
    }
    case "turn_completed":
      // A task that does not run in the background cannot outlive its turn.
      for (const task of tasks.filter((x) => x.running && !x.background)) {
        task.running = false;
        task.status ??= TURN_ENDED;
        task.ended_at_ms = t;
      }
      break;
  }
  return tasks;
}

/** Each subagent shows what the agent's task for it says: the link is the spawning call, and the task decides whether it runs. */
function syncSubagents(subagents: readonly SubagentInfo[], tasks: readonly TaskInfo[]): SubagentInfo[] {
  return subagents.map((s) => {
    const task = s.task_id ? tasks.find((x) => x.task_id === s.task_id) : tasks.find((x) => x.call_id === s.call_id);
    if (!task) return s;
    return {
      ...s,
      task_id: task.task_id,
      running: task.running,
      status: task.status,
      ok: task.running ? undefined : task.ok,
      ended_at_ms: task.ended_at_ms,
      subagent_type: task.subagent_type ?? s.subagent_type,
      last_tool: task.last_tool ?? s.last_tool,
      usage: task.usage,
    };
  });
}

const subagentOf = (a: Extract<AgentEvent, { type: "tool_call_started" }>, t: number): SubagentInfo => {
  const input = (a.input ?? {}) as { description?: string; subagent_type?: string };
  return { call_id: a.call_id, name: a.name, model: a.subagent_model, description: input.description ?? "", started_at_ms: t, running: true, subagent_type: input.subagent_type, tool_calls: 0 };
};

/** What the tool call that started a subagent returning means: its end only for a subagent the agent reports no task for. */
function finishCall(subagents: readonly SubagentInfo[], callId: string, ok: boolean, t: number): SubagentInfo[] {
  return subagents.map((s) => (s.call_id === callId && !s.task_id ? { ...s, running: false, ok, ended_at_ms: t } : s));
}

/** The work state after one agent event. */
export function reduceWork(w: Work, a: AgentEvent, t: number): Work {
  let subagents = w.subagents;
  let tasks = w.tasks;
  if (a.type === "scoped") {
    const inner = a.event;
    subagents = subagents.map((s) => (s.call_id === a.scope.call_id ? { ...s, agent_id: s.agent_id ?? a.scope.agent_id } : s));
    if (inner.type === "tool_call_started") {
      subagents = subagents.map((s) => (s.call_id === a.scope.call_id ? { ...s, tool_calls: s.tool_calls + 1, last_tool: inner.name } : s));
      if (inner.is_subagent) subagents = [...subagents, subagentOf(inner, t)];
    } else if (inner.type === "tool_call_finished") subagents = finishCall(subagents, inner.call_id, inner.ok, t);
    return { tasks, subagents: syncSubagents(subagents, tasks), background_running: runningInBackground(tasks) };
  }
  if (a.type === "tool_call_started" && a.is_subagent) subagents = [...subagents, subagentOf(a, t)];
  else if (a.type === "tool_call_finished") subagents = finishCall(subagents, a.call_id, a.ok, t);
  else tasks = reduceTasks(tasks, a, t);
  return { tasks, subagents: syncSubagents(subagents, tasks), background_running: runningInBackground(tasks) };
}

/** The process ended: nothing runs any more. */
export function endAllWork(w: Work, t: number): Work {
  const tasks = w.tasks.map((x) => (x.running ? { ...x, running: false, status: x.status ?? PROCESS_ENDED, ended_at_ms: t } : x));
  return { tasks, subagents: syncSubagents(w.subagents, tasks), background_running: 0 };
}

/** What a session's events say about its work (a scripted history's snapshot). */
export function foldWork(events: readonly SessionEvent[]): Work {
  return events.reduce<Work>((w, e) => (e.kind === "agent" ? reduceWork(w, e.event, e.t) : w), NO_WORK);
}

/** `HistoryScope::admits` of the daemon (`wasit-runtime` `wire.rs`): which events a history page of `scope` holds. */
export function historyAdmits(scope: HistoryScope, e: SessionEvent): boolean {
  if (e.kind !== "agent") return scope.kind !== "subagent";
  // An item of the earlier history is filtered by what it holds.
  const a = e.event.type === "history" ? e.event.event : e.event;
  if (scope.kind === "all") return true;
  if (scope.kind === "main") return a.type !== "scoped";
  if (a.type === "scoped") return a.scope.call_id === scope.call_id;
  if (a.type === "task_started" || a.type === "task_ended") return a.call_id === scope.call_id;
  return false;
}
