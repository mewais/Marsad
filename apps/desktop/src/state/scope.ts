import type { HistoryScope, SessionEvent, SessionKey } from "../bridge/wire";

/**
 * Whose transcript a list of events is: the session's own conversation, or one subagent's (named by
 * the tool call that spawned it, `SubagentInfo::call_id`). `session.history` takes the same value;
 * "everything" is not a transcript, so it is not one here.
 */
export type TranscriptScope = Exclude<HistoryScope, { kind: "all" }>;

export const MAIN: TranscriptScope = { kind: "main" };
export const subagentScope = (callId: string): TranscriptScope => ({ kind: "subagent", call_id: callId });

/** Where a scope's transcript is kept in the store: the session's key for its own, a derived one per subagent. */
export const scopeKey = (key: SessionKey, scope: TranscriptScope): string => (scope.kind === "main" ? key : `${key}#${scope.call_id}`);

/**
 * Whether `e` belongs to `scope`: the daemon's rule for a history page, applied to the live stream (which
 * carries everything, so each transcript takes what is its own). The session's own transcript is every event
 * except the subagents' scoped ones; a subagent's is the events scoped to it and the start and end of its task.
 */
export function admits(scope: TranscriptScope, e: SessionEvent): boolean {
  if (e.kind !== "agent") return scope.kind === "main";
  // An item of the conversation's earlier history is filtered by what it holds, as the daemon does.
  const a = e.event?.type === "history" ? e.event.event : e.event;
  // An agent record with no event (a shape this build does not know) is the session's own, never a subagent's.
  if (!a) return scope.kind === "main";
  if (scope.kind === "main") return a.type !== "scoped";
  switch (a.type) {
    case "scoped":
      return a.scope.call_id === scope.call_id;
    case "task_started":
    case "task_ended":
      return a.call_id === scope.call_id;
    default:
      return false;
  }
}
