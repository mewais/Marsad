import type { DriverInventory, HostId, LaunchRequest, ResumableSession, SessionSnapshot } from "../../bridge/wire";
import { continueState, notContinuableWords, sessionName } from "../../state/domain";

/** Pure logic of the New session dialog (tested without React). */

export type StartMode = "new" | "continue";

export interface NewSessionForm {
  host: HostId;
  /** The session's name (optional; in a workstream design, the node's). */
  name: string;
  driver: string;
  cwd: string;
  model: string;
  mode: string;
  start: StartMode;
  /** Native id of the CLI session to continue (`start === "continue"`, `latest` false). */
  resumeId: string;
  /** Continue the CLI's own most recent conversation in the folder (its native "continue"). */
  latest: boolean;
  prompt: string;
}

export interface ResumeRow extends ResumableSession {
  /** A Marsad session on this host already carries this native id. */
  startedIn: "marsad" | "terminal";
  /**
   * That Marsad session (`native_session_id` is the conversation's id): continuing the conversation continues it, with its
   * transcript. Absent for a conversation that only the agent's own storage holds.
   */
  session?: SessionSnapshot;
}

/** Which of several Marsad sessions on one conversation to offer: one that is running, else one that can be continued, else the latest. */
const preference = (s: SessionSnapshot): number => (continueState(s).kind === "running" ? 0 : continueState(s).kind === "ready" ? 1 : 2);
const better = (a: SessionSnapshot, b: SessionSnapshot) => preference(a) - preference(b) || b.last_activity_ms - a.last_activity_ms;

/** The CLI's own sessions for this project, newest first, tagged with where they were started and the Marsad session that carries them. */
export function resumeRows(inv: DriverInventory | undefined, hostSessions: SessionSnapshot[], q = ""): ResumeRow[] {
  if (!inv) return [];
  const marsad = new Map<string, SessionSnapshot>();
  for (const s of hostSessions) {
    if (s.driver !== inv.driver || !s.native_session_id) continue;
    const had = marsad.get(s.native_session_id);
    if (!had || better(s, had) < 0) marsad.set(s.native_session_id, s);
  }
  const n = q.trim().toLowerCase();
  return inv.sessions
    .filter((s) => !n || s.title.toLowerCase().includes(n) || s.id.toLowerCase().includes(n))
    .map((s): ResumeRow => ({ ...s, startedIn: marsad.has(s.id) ? "marsad" : "terminal", session: marsad.get(s.id) }))
    .sort((a, b) => b.modified_ms - a.modified_ms);
}

/**
 * What starting from the form does. A conversation that belongs to a Marsad session is that session's: it is
 * opened when it is running, continued (with its earlier transcript) when the daemon says it can be, and
 * refused with the daemon's reason when it cannot; never a second Marsad session on the same conversation.
 * Everything else launches a new session (a conversation only the agent's storage holds is resumed in it).
 */
export type StartTarget =
  | { kind: "launch" }
  | { kind: "open"; session: SessionSnapshot }
  | { kind: "continue"; session: SessionSnapshot }
  | { kind: "blocked"; session: SessionSnapshot; why: string };

export function startTarget(f: Pick<NewSessionForm, "start" | "latest" | "resumeId">, rows: ResumeRow[]): StartTarget {
  const session = f.start === "continue" && !f.latest && f.resumeId ? rows.find((r) => r.id === f.resumeId)?.session : undefined;
  if (!session) return { kind: "launch" };
  const c = continueState(session);
  return c.kind === "running" ? { kind: "open", session } : c.kind === "ready" ? { kind: "continue", session } : { kind: "blocked", session, why: notContinuableWords(c.why) };
}

/** What the box under the list says about a conversation that belongs to a Marsad session. */
export function ownerNote(t: StartTarget): string | null {
  switch (t.kind) {
    case "launch":
      return null;
    case "open":
      return `This conversation is running in your session “${sessionName(t.session)}”. Open it instead of starting a second one.`;
    case "continue":
      return `This conversation belongs to your session “${sessionName(t.session)}”. Continue picks that session up, with its earlier transcript.`;
    case "blocked":
      return `This conversation belongs to your session “${sessionName(t.session)}”. ${t.why}`;
  }
}

export const mostRecent = (rows: ResumeRow[]): ResumeRow | undefined => rows[0];

/** Default model/mode as the CLI reports them (its own default model; first mode). */
export function defaultsFor(inv: DriverInventory | undefined): { model: string; mode: string } {
  return { model: inv?.models.find((m) => m.is_default)?.id ?? inv?.models[0]?.id ?? "", mode: inv?.modes[0]?.id ?? "" };
}

/** Why the form cannot be submitted yet ("" = it can). */
export function validate(f: NewSessionForm, inv: DriverInventory | undefined, hostUp: boolean, target: StartTarget = { kind: "launch" }): string {
  if (!hostUp) return "That host is not connected.";
  if (!inv?.installed) return inv?.error ?? "Pick an agent installed on this host.";
  if (!f.cwd.trim()) return "Pick a project folder.";
  if (f.start === "continue" && f.latest) return inv.capabilities.continue_latest ? "" : `${inv.display_name} cannot continue its most recent session.`;
  if (f.start === "continue" && !f.resumeId) return "Pick a session to continue.";
  if (target.kind === "blocked") return target.why;
  if (f.start === "continue" && !inv.capabilities.resume) return `${inv.display_name} cannot resume sessions.`;
  return "";
}

export function launchRequest(f: NewSessionForm, workstream: LaunchRequest["workstream"] = null): LaunchRequest {
  return {
    host: f.host,
    driver: f.driver,
    cwd: f.cwd.trim(),
    model: f.model || null,
    mode: f.mode || null,
    first_prompt: f.prompt.trim() || null,
    name: f.name.trim() || null,
    resume: f.start !== "continue" ? { kind: "new" } : f.latest ? { kind: "latest" } : { kind: "resume", native_session_id: f.resumeId },
    labels: {},
    workstream,
    overrides: { items: [], env: [], disallowed_tools: [] },
  };
}
