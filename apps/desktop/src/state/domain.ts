import type { GitSummary, HostId, PendingGate, SeenMark, SessionKey, SessionSnapshot } from "../bridge/wire";
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
  /** From the hub's background git summary ("" until known / not a repository). */
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
      return s.plan.find((p) => p.status === "in_progress")?.content ?? (s.turn?.current_tool ? `Running ${s.turn.current_tool}` : s.turn?.last_tool ? `Ran ${s.turn.last_tool}${s.turn.tool_calls > 1 ? ` · ${s.turn.tool_calls} steps` : ""}` : s.turn?.prompt ?? "Working");
    default:
      return s.last_turn?.reply ?? (s.phase.state === "exited" ? "Session ended" : "Idle");
  }
}

/** The name the user gave the session, else its first prompt (`title`), else the id. */
export function sessionName(s: SessionSnapshot): string {
  return s.display_name?.trim() || s.title?.trim() || s.id;
}

const statCache = new WeakMap<GitSummary, SessionView["stat"]>();
/** Files and lines changed, stable per summary object (rows compare views shallowly). */
export function diffStat(g: GitSummary | null | undefined): SessionView["stat"] {
  if (!g || !g.files) return null;
  const hit = statCache.get(g);
  if (hit !== undefined) return hit;
  const st = { files: g.files, added: g.added, removed: g.removed };
  statCache.set(g, st);
  return st;
}

export function buildView(host: HostId, s: SessionSnapshot, mark: SeenMark | undefined, git: GitSummary | null | undefined, workstream: string): SessionView {
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

/**
 * The model picker's options and value for a session. CLIs may list aliases ("haiku") and
 * report the resolved model ("claude-haiku-5-5"): the alias it resolves to is selected, and a
 * reported model that matches nothing is still offered rather than shown as unselected.
 */
export function modelChoice(s: SessionSnapshot): { value: string | undefined; options: { value: string; label: string; description?: string }[] } {
  const options = s.available_models.map((m) => ({ value: m.id, label: m.display_name || m.id, description: m.description }));
  const cur = s.model ?? undefined;
  if (!cur || options.some((o) => o.value === cur)) return { value: cur, options };
  const alias = options.filter((o) => o.value !== "default" && cur.toLowerCase().includes(o.value.toLowerCase())).sort((a, b) => b.value.length - a.value.length)[0];
  if (alias) return { value: alias.value, options };
  return { value: cur, options: [{ value: cur, label: cur }, ...options] };
}
