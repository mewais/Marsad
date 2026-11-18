import type { DriverInventory, HostId, LaunchRequest, ResumableSession, SessionSnapshot } from "../../bridge/wire";

/** Pure logic of the New session dialog (tested without React). */

export type StartMode = "new" | "continue";

export interface NewSessionForm {
  host: HostId;
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
}

/** The CLI's own sessions for this project, newest first, tagged with where they were started. */
export function resumeRows(inv: DriverInventory | undefined, hostSessions: SessionSnapshot[], q = ""): ResumeRow[] {
  if (!inv) return [];
  const marsad = new Set(hostSessions.filter((s) => s.driver === inv.driver && s.native_session_id).map((s) => s.native_session_id!));
  const n = q.trim().toLowerCase();
  return inv.sessions
    .filter((s) => !n || s.title.toLowerCase().includes(n) || s.id.toLowerCase().includes(n))
    .map((s) => ({ ...s, startedIn: marsad.has(s.id) ? ("marsad" as const) : ("terminal" as const) }))
    .sort((a, b) => b.modified_ms - a.modified_ms);
}

export const mostRecent = (rows: ResumeRow[]): ResumeRow | undefined => rows[0];

/** Default model/mode as the CLI reports them (its own default model; first mode). */
export function defaultsFor(inv: DriverInventory | undefined): { model: string; mode: string } {
  return { model: inv?.models.find((m) => m.is_default)?.id ?? inv?.models[0]?.id ?? "", mode: inv?.modes[0]?.id ?? "" };
}

/** Why the form cannot be submitted yet ("" = it can). */
export function validate(f: NewSessionForm, inv: DriverInventory | undefined, hostUp: boolean): string {
  if (!hostUp) return "That host is not connected.";
  if (!inv?.installed) return inv?.error ?? "Pick an agent installed on this host.";
  if (!f.cwd.trim()) return "Pick a project folder.";
  if (f.start === "continue" && f.latest) return inv.capabilities.continue_latest ? "" : `${inv.display_name} cannot continue its most recent session.`;
  if (f.start === "continue" && !f.resumeId) return "Pick a session to continue.";
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
    resume: f.start !== "continue" ? { kind: "new" } : f.latest ? { kind: "latest" } : { kind: "resume", native_session_id: f.resumeId },
    labels: {},
    workstream,
    overrides: { items: [], env: [], disallowed_tools: [] },
  };
}
