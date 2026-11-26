import type { SessionSnapshot, SubagentInfo, SubagentSupport, TaskInfo, TaskOutcome, TurnOrigin } from "../../bridge/wire";
import { compactNumber, money } from "../../lib/format";
import type { StatusKey } from "../../ui/status";

/**
 * The words for the work an agent runs besides its own turn: subagents and background tasks. Everything
 * here is read off what the daemon reports (the snapshot's `subagents` and `tasks`, the task events); a
 * word the agent chose (a task's kind, its status) is shown as the agent said it, never mapped to a rule.
 */

/** What the UI shows of a subagent's state: the status key (icon and colour) and the word. */
export interface SubagentState {
  status: StatusKey;
  word: string;
  /** Why the word is what it is, for what the word alone does not say (shown as a tooltip). */
  why?: string;
}

/**
 * How each outcome of a subagent reads. `stopped` is shown as Stopped (not as a failure); `lost` and `unknown` say
 * what the daemon could not know, never a guess of success.
 */
const OUTCOMES: Record<TaskOutcome, SubagentState> = {
  completed: { status: "done", word: "Finished" },
  failed: { status: "failed", word: "Failed" },
  stopped: { status: "ended", word: "Stopped" },
  lost: { status: "failed", word: "Lost", why: "The agent process ended while it ran." },
  unknown: { status: "idle", word: "Unknown", why: "State not available." },
};

/**
 * A subagent's state, from the daemon's facts: Working while its task runs; then its outcome. An outcome the agent did
 * not name in the shared vocabulary falls back to its `ok` flag; with neither, the state is not available (Unknown).
 */
export function subagentState(s: { running: boolean; outcome?: TaskOutcome | null; ok?: boolean | null }): SubagentState {
  if (s.running) return { status: "working", word: "Working" };
  const outcome: TaskOutcome | undefined = s.outcome ?? (s.ok === true ? "completed" : s.ok === false ? "failed" : undefined);
  return outcome ? OUTCOMES[outcome] : OUTCOMES.unknown;
}

/** What a task is called: its description, else the agent's word for its kind. */
export const taskLabel = (t: { description?: string; kind: string }): string => t.description || t.kind;


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

/** What a driver that reports nothing about its subagents says: no figure is shown for any of them. */
export const NO_SUBAGENT_SUPPORT: SubagentSupport = { transcript: false, live_state: false, usage: false, steer: false, background: false };

/**
 * What the agent reports about a subagent beyond its state, as the words shown for it. Only what the driver's
 * capabilities say it reports is shown: an agent that does not report counts or usage gets no figure at all, never a "0".
 */
export function subagentFacts(s: Pick<SubagentInfo, "tool_calls" | "usage">, caps: SubagentSupport): string[] {
  const facts: string[] = [];
  if (caps.live_state) facts.push(`${s.tool_calls} tool call${s.tool_calls === 1 ? "" : "s"}`);
  if (caps.usage && s.usage) {
    facts.push(`${compactNumber(s.usage.total_tokens)} tokens`);
    if (s.usage.cost_usd != null) facts.push(money(s.usage.cost_usd));
  }
  return facts;
}
