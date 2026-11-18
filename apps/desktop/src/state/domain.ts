import type { HostId, PendingGate, SessionKey, SessionSnapshot } from "../bridge/types";
import { sessionKey } from "../bridge/types";
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
  branch: string;
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

/** A finished turn the human has not looked at yet. */
export function isUnseenFinish(s: SessionSnapshot, seenAt: number | undefined): boolean {
  if (s.phase.state !== "idle" && s.phase.state !== "exited") return false;
  const done = s.last_turn?.ended_at_ms ?? (s.phase.state === "exited" ? s.exit?.at_ms : undefined);
  if (!done) return false;
  return done > (seenAt ?? 0);
}

export function sessionLine(s: SessionSnapshot): string {
  switch (s.phase.state) {
    case "needs_you": {
      const g = s.pending_gates[0]?.gate;
      if (!g) return "Waiting for you";
      if (g.gate_type === "question") return `Asks: ${g.questions[0]?.question ?? "a question"}`;
      if (g.gate_type === "permission") return `Wants to run ${g.command}`;
      if (g.gate_type === "plan") return "Plan waiting for approval";
      return `Handoff waiting: ${g.task}`;
    }
    case "failed":
      return s.phase.reason;
    case "lost":
      return s.phase.reason;
    case "working":
    case "starting":
      return s.plan.find((p) => p.status === "in_progress")?.content ?? (s.turn?.last_tool ? `Running ${s.turn.last_tool}` : s.turn?.prompt ?? "Working");
    default:
      return s.last_turn?.reply ?? (s.phase.state === "exited" ? "Session ended" : "Idle");
  }
}

export function sessionName(s: SessionSnapshot): string {
  return s.labels?.name ?? s.title ?? s.id;
}

export function buildView(host: HostId, s: SessionSnapshot, seenAt: number | undefined, lookedAt: number): SessionView {
  void lookedAt;
  const unseen = isUnseenFinish(s, seenAt);
  const l = s.labels ?? {};
  return {
    key: keyOf(host, s),
    host,
    id: s.id,
    name: sessionName(s),
    status: phaseToStatus(s.phase.state, unseen),
    phase: s.phase.state,
    driver: s.driver,
    project: basename(s.cwd),
    branch: l["git.branch"] ?? "",
    workstream: l.workstream ?? "",
    lastMs: s.last_activity_ms,
    cost: s.usage.cost_usd,
    line: sessionLine(s),
    stat: l["diff.files"] ? { files: Number(l["diff.files"]), added: Number(l["diff.added"] ?? 0), removed: Number(l["diff.removed"] ?? 0) } : null,
    unseen,
    gates: s.pending_gates,
  };
}
