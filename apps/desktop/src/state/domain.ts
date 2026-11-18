import type { GitStatus, HostId, PendingGate, SeenMark, SessionKey, SessionSnapshot } from "../bridge/wire";
import { sessionKey } from "../bridge/wire";
import { basename } from "../lib/format";
import { phaseToStatus, type StatusKey } from "../ui/status";

/** Flat, primitive-heavy view of a session for rows and cards. Compared shallowly. */
export interface SessionView {
  key: SessionKey;
  host: HostId;
  id: string;
  name: string;
  status: StatusKey;
  phase: SessionSnapshot["phase"]["state"];
  driver: string;
  project: string;
  /** From `git.status`, once fetched ("" until then). */
  branch: string;
  /** Name of the workstream the session is a node of ("" = standalone). */
  workstream: string;
  lastMs: number;
  cost: number;
  /** One line about what it is doing / did / how it failed. */
  line: string;
  stat: { files: number; added: number; removed: number } | null;
  unseen: boolean;
  gates: PendingGate[];
}

export const keyOf = (host: HostId, s: SessionSnapshot): SessionKey => sessionKey(host, s.id);

export function finishedAt(s: SessionSnapshot): number | undefined {
  if (s.phase.state !== "idle" && s.phase.state !== "exited") return undefined;
  return s.last_turn?.ended_at_ms ?? (s.phase.state === "exited" ? s.exit?.at_ms : undefined);
}

/** A finished turn the human has not looked at yet (per `store.seen` markers). */
export function isUnseenFinish(s: SessionSnapshot, mark: SeenMark | undefined): boolean {
  const done = finishedAt(s);
  if (!done) return false;
  if (!mark) return true;
  return mark.cursor < s.cursor && done > mark.at_ms;
}

export function sessionLine(s: SessionSnapshot): string {
  switch (s.phase.state) {
    case "needs_you": {
      const g = s.pending_gates[0]?.gate;
      if (!g) return "Waiting for you";
      if (g.gate_type === "question") return `Asks: ${g.questions[0]?.question ?? "a question"}`;
      if (g.gate_type === "permission") return `Wants to run ${g.command}`;
      if (g.gate_type === "plan") return "Plan waiting for approval";
      return `Handoff waiting: ${g.from} wants to delegate “${g.instruction}”`;
    }
    case "failed":
    case "lost":
      return s.phase.reason;
    case "working":
    case "starting":
      return s.plan.find((p) => p.status === "in_progress")?.content ?? (s.turn?.last_tool ? `Running ${s.turn.last_tool}` : s.turn?.prompt ?? "Working");
    default:
      return s.last_turn?.reply ?? (s.phase.state === "exited" ? "Session ended" : "Idle");
  }
}

/** The wire has no user-given session name: the first prompt (`title`), else the id. */
export function sessionName(s: SessionSnapshot): string {
  return s.title?.trim() || s.id;
}

export function diffStat(g: GitStatus | null | undefined): SessionView["stat"] {
  if (!g || !g.files.length) return null;
  let added = 0;
  let removed = 0;
  for (const f of g.files) {
    added += f.staged_added + f.unstaged_added;
    removed += f.staged_removed + f.unstaged_removed;
  }
  return { files: g.files.length, added, removed };
}

export function buildView(host: HostId, s: SessionSnapshot, mark: SeenMark | undefined, git: GitStatus | null | undefined, workstream: string): SessionView {
  const unseen = isUnseenFinish(s, mark);
  return {
    key: keyOf(host, s),
    host,
    id: s.id,
    name: sessionName(s),
    status: phaseToStatus(s.phase.state, unseen),
    phase: s.phase.state,
    driver: s.driver,
    project: basename(s.cwd ?? undefined),
    branch: git?.branch ?? "",
    workstream,
    lastMs: s.last_activity_ms,
    cost: s.usage.cost_usd,
    line: sessionLine(s),
    stat: diffStat(git),
    unseen,
    gates: s.pending_gates,
  };
}
