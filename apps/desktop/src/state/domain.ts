import { permissionKind } from "../lib/gatewords";
import type { GitSummary, HostId, ModelInfo, NotContinuable, PendingGate, SeenMark, SessionKey, SessionSnapshot } from "../bridge/wire";
import { sessionKey } from "../bridge/wire";
import { basename } from "../lib/format";
import { phaseToStatus, type StatusKey } from "../ui/status";
import type { SelectOption } from "../ui/Select";

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
  /** The daemon's verdict on continuing the session (see `continueState`). */
  cont: ContinueState;
  /**
   * What this view shows is the last the app knew, not live: the app lost its link to the local
   * daemon, or the session's host is not connected / has not delivered its list over the current
   * link. Its status then says where the session was, not what it is doing.
   */
  stale: boolean;
}

/**
 * Whether the session can be continued, exactly as the daemon says (`continuable` and, when not,
 * `not_continuable` on the snapshot): the app infers nothing from the phase or the agent.
 * - `running`: the process is running; there is nothing to continue and no Continue to offer.
 * - `ready`: a new process can be started now (Continue, or a prompt, which continues it first).
 * - `blocked`: the session has ended and cannot be continued; `why` is the daemon's rule.
 */
export type ContinueState = { kind: "running" } | { kind: "ready" } | { kind: "blocked"; why: Exclude<NotContinuable, "not_ended"> };

const RUNNING: ContinueState = { kind: "running" };
const READY: ContinueState = { kind: "ready" };
const BLOCKED: Record<Exclude<NotContinuable, "not_ended">, ContinueState> = {
  loading: { kind: "blocked", why: "loading" },
  agent_cannot_resume: { kind: "blocked", why: "agent_cannot_resume" },
  no_conversation_id: { kind: "blocked", why: "no_conversation_id" },
  nothing_saved: { kind: "blocked", why: "nothing_saved" },
};

/** The daemon's verdict on `snap` (stable objects, so views compare shallowly). */
export function continueState(snap: Pick<SessionSnapshot, "continuable" | "not_continuable">): ContinueState {
  if (snap.continuable) return READY;
  const why = snap.not_continuable;
  return why && why !== "not_ended" ? BLOCKED[why] ?? RUNNING : RUNNING;
}

/** Why a session that has ended cannot be continued, as one sentence. */
export function notContinuableWords(why: NotContinuable): string {
  switch (why) {
    case "not_ended":
      return "The session is still running.";
    case "loading":
      return "The session is still loading; wait for it to finish.";
    case "agent_cannot_resume":
      return "This agent cannot pick a conversation back up.";
    case "no_conversation_id":
      return "The agent has not said which conversation this was, so it cannot be picked up again.";
    case "nothing_saved":
      return "The agent has not saved this conversation yet (it saves it with the first message), so there is nothing to continue.";
  }
}

/**
 * Whether Interrupt does anything: the agent can be interrupted and a turn is starting or running
 * (or waiting for a decision, which an interrupt declines). The one rule for the button.
 */
export function canInterrupt(s: Pick<SessionSnapshot, "capabilities" | "phase">): boolean {
  return s.capabilities.interrupt && (s.phase.state === "starting" || s.phase.state === "working" || s.phase.state === "needs_you");
}

/**
 * Whether Esc interrupts: a turn is running and the session is not waiting on the user. Esc must
 * never deny a pending request by accident (interrupting a session that waits for a decision
 * declines it), so only the button interrupts then.
 */
export function escapeInterrupts(s: Pick<SessionSnapshot, "capabilities" | "phase">): boolean {
  return s.capabilities.interrupt && s.phase.state === "working";
}

/** The word for a session whose phase is more specific than its status ("Starting" while the agent's process comes up), else `undefined`: the status label stands. */
export const phaseWord = (v: Pick<SessionView, "phase">): string | undefined => (v.phase === "starting" ? "Starting" : undefined);

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
      if (g.gate_type === "permission") return permissionKind(g) === "question" ? `Asks: ${g.command}` : permissionKind(g) === "command" ? `Wants to run ${g.command}` : `Wants to use ${g.tool_name}: ${g.command}`;
      if (g.gate_type === "plan") return "Plan waiting for approval";
      if (g.gate_type === "spawn") return `Wants to start ${g.name} on ${g.host}: ${g.role}`;
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

/**
 * The name the user gave the session, else its first prompt (`title`), else the prompt it is
 * working on, else its folder. Never the raw session id.
 */
export function sessionName(s: SessionSnapshot): string {
  const prompt = s.turn?.prompt?.trim().split("\n")[0];
  return s.display_name?.trim() || s.title?.trim() || (prompt ? (prompt.length > 60 ? `${prompt.slice(0, 59)}…` : prompt) : "") || (s.cwd ? `New session in ${basename(s.cwd)}` : "New session");
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

export function buildView(host: HostId, s: SessionSnapshot, mark: SeenMark | undefined, git: GitSummary | null | undefined, workstream: string, stale = false): SessionView {
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
    cont: continueState(s),
    stale,
  };
}

/** Picker options for models the CLI reports; a model shown by its bare id is set in mono. */
export function modelOptions(models: ModelInfo[]): SelectOption[] {
  return models.map((m) => ({ value: m.id, label: m.display_name || m.id, description: m.description, mono: !m.display_name || m.display_name === m.id }));
}

/**
 * The model picker's options and value for a session. CLIs may list aliases ("haiku") and
 * report the resolved model ("claude-haiku-5-5"): the alias it resolves to is selected, and a
 * reported model that matches nothing is still offered rather than shown as unselected.
 */
export function modelChoice(s: SessionSnapshot): { value: string | undefined; options: SelectOption[] } {
  const options = modelOptions(s.available_models);
  const cur = s.model ?? undefined;
  if (!cur || options.some((o) => o.value === cur)) return { value: cur, options };
  const alias = options.filter((o) => o.value !== "default" && cur.toLowerCase().includes(o.value.toLowerCase())).sort((a, b) => b.value.length - a.value.length)[0];
  if (alias) return { value: alias.value, options };
  return { value: cur, options: [{ value: cur, label: cur, mono: true }, ...options] };
}
