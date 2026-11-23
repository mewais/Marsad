import type { SessionSnapshot, SubagentInfo, TaskInfo, TurnOrigin } from "../../bridge/wire";

/**
 * The words for the work an agent runs besides its own turn: subagents and background tasks. Everything
 * here is read off what the daemon reports (the snapshot's `subagents` and `tasks`, the task events); a
 * word the agent chose (a task's kind, its status) is shown as the agent said it, never mapped to a rule.
 */

/** What the UI shows of a task's or a subagent's state: the pill's tone and word. */
export interface TaskState {
  tone: "working" | "done" | "failed";
  word: string;
}

const sentence = (s: string): string => {
  const t = s.replace(/_/g, " ").trim();
  return t.charAt(0).toUpperCase() + t.slice(1);
};

/**
 * Running until the daemon says the agent's task ended (a backgrounded subagent keeps running long after the
 * tool call that started it returned); then the agent's verdict: finished, or its own status word.
 */
export function taskState(t: { running: boolean; ok?: boolean | null; status?: string | null }): TaskState {
  if (t.running) return { tone: "working", word: "Running" };
  if (t.ok === true) return { tone: "done", word: "Finished" };
  if (t.ok === false) return { tone: "failed", word: t.status ? sentence(t.status) : "Failed" };
  return { tone: "done", word: t.status ? sentence(t.status) : "Ended" };
}

/** What a task is called: its description, else the agent's word for its kind. */
export const taskLabel = (t: { description?: string; kind: string }): string => t.description || t.kind;

/** How long it has run (running: until `now`), or ran. */
export const taskElapsedMs = (t: { started_at_ms: number; ended_at_ms?: number | null; running: boolean }, now: number): number => Math.max(0, (t.running ? now : t.ended_at_ms ?? now) - t.started_at_ms);

/** The session's subagents: the running ones first, then the finished, each newest first. */
export function orderSubagents(list: readonly SubagentInfo[]): SubagentInfo[] {
  return [...list].sort((a, b) => Number(b.running) - Number(a.running) || (b.running ? b.started_at_ms : b.ended_at_ms ?? b.started_at_ms) - (a.running ? a.started_at_ms : a.ended_at_ms ?? a.started_at_ms));
}

/** Whether the agent's task `t` is the one a subagent runs as. */
const isSubagentTask = (subagents: readonly SubagentInfo[], t: TaskInfo): boolean => subagents.some((s) => (s.task_id ? s.task_id === t.task_id : !!t.call_id && s.call_id === t.call_id));

/** The background tasks running now that are not subagents (those are listed as subagents), as the daemon lists them. */
export function otherBackgroundTasks(snap: Pick<SessionSnapshot, "tasks" | "subagents">): TaskInfo[] {
  return snap.tasks.filter((t) => t.running && t.background && !isSubagentTask(snap.subagents, t)).sort((a, b) => b.started_at_ms - a.started_at_ms);
}

/** The line in the transcript where a task started. */
export function taskStartedWords(t: { kind: string; description?: string; background: boolean }): string {
  return `${t.background ? "Background task" : "Task"} started · ${t.kind}${t.description ? ` · “${t.description}”` : ""}`;
}

/** The line where it ended: the agent's status word and its one-line account. */
export function taskEndedWords(t: { kind: string; description?: string; background: boolean } | undefined, e: { status: string; summary?: string | null }): string {
  const what = t ? `${t.background ? "Background task" : "Task"} ended · ${t.kind}${t.description ? ` · “${t.description}”` : ""}` : "A task ended";
  return `${what} · ${e.status}${e.summary ? ` — ${e.summary}` : ""}`;
}

/** Why the agent says it ran a turn nobody asked for: its own word, and the task it answers when the event names one we know. */
export function originWords(o: TurnOrigin, task?: { kind: string; description?: string }): string {
  return `origin: ${o.kind}${o.producer ? ` (${o.producer})` : ""}${task ? ` · ${taskLabel(task)}` : ""}`;
}
