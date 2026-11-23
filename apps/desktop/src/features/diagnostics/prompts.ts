import type { TrailItem } from "../../bridge/wire";
import { fieldStr, type DiagRecord } from "../../lib/diagRecord";
import type { AppEventRec } from "../../state";
import type { ClockOffset } from "./clock";
import { duration, errorWords, sizeWords, type Tone } from "./words";

/**
 * The path of each prompt through the system (design §5.1): accepted by the app, reached the hub,
 * reached the host, recorded by the host, queued or sent, written to the agent, acknowledged by it
 * -- or dropped, or refused, with the reason.
 *
 * Each step is shown only if something observed it. A step that was not seen while a later one was
 * is `not_seen`; one after the last thing seen is `pending` (not yet), unless the path ended
 * (dropped, refused), in which case there are no later steps. Nothing is guessed from what comes
 * before or after.
 *
 * Time. Every step has the clock of the machine that wrote it: the app and the hub are this
 * computer, the rest are the session's host. Each time is shown relative to the first step seen;
 * a time from a different clock than that first step's is approximate (`approx`), because machines'
 * clocks are not synchronised. When the host's clock offset has been measured (`PromptSources.offset`),
 * the host's times are first moved onto this computer's clock by it; they stay approximate. Steps are in the path's order, never sorted by time across machines.
 */

export type StepId = "app" | "hub" | "host" | "recorded" | "queued" | "sent" | "written" | "acknowledged" | "turn" | "dropped" | "write_failed" | "refused" | "not_accepted";
export type StepState = "seen" | "not_seen" | "pending";

export interface PathStep {
  id: StepId;
  label: string;
  state: StepState;
  tone: Tone;
  /** Wall time on `clock`'s machine (seen steps). */
  t?: number;
  /** Which machine's clock `t` is: `local` (this computer: the app and the hub) or the host's id. */
  clock?: string;
  /** Who observed it: `app`, `hub`, or the host's name. */
  source?: string;
  /** What the observer recorded: how long, the outcome, the reason. */
  detail?: string;
  /** Time since the first seen step (ms). */
  rel?: number;
  /** `rel` crosses clocks of different machines: only approximately true. */
  approx?: boolean;
  /** `t` was moved by the measured clock offset before `rel` was taken. */
  corrected?: boolean;
}

export interface PromptRow {
  /** Stable key: the input's id, or the trace of a prompt the host never recorded. */
  key: string;
  /** The host's id for it (the journal seq); absent for a prompt the host did not record. */
  input: number | null;
  trace: string | null;
  kind: "prompt" | "slash";
  chars: number;
  attachments: number;
  bytes: number;
  steps: PathStep[];
  /** Further facts for the expanded view. */
  facts: { k: string; v: string }[];
}

export interface PromptSources {
  /** The session's trail (the host's order). */
  trail: readonly TrailItem[];
  /** What the app recorded of its own acts. */
  app: readonly AppEventRec[];
  /** The hub's log records for the prompts' traces. */
  hub: readonly DiagRecord[];
  /** The host daemon's log records for them; empty for the local host (the hub is its daemon). */
  host: readonly DiagRecord[];
  session: { host: string; id: string; hostName: string; local: boolean };
  /** Whether each daemon's log has been read yet; a leg whose log is not is pending, not "not seen". */
  loaded: { hub: boolean; host: boolean };
  /** The host's wall clock minus this computer's, once measured; absent: the host's times are taken as they were written. */
  offset?: ClockOffset | null;
}

type Applied = Extract<TrailItem, { ev: "input.applied" }>;

/** The traces the prompts of a session carry, newest last: what to ask the daemons about (at most `max`). */
export function promptTraces(trail: readonly TrailItem[], app: readonly AppEventRec[], session: { host: string; id: string }, max = 64): string[] {
  const out: string[] = [];
  const add = (t: string | null | undefined) => t && !out.includes(t) && out.push(t);
  for (const it of trail) if ((it.ev === "input.applied" && (it.kind === "prompt" || it.kind === "run_slash")) || it.ev === "process.started") add(it.trace);
  for (const a of app) if (a.event.ev === "ui.prompt" && a.event.host === session.host && a.event.session === session.id) add(a.event.trace);
  return out.slice(-max);
}

const isPrompt = (it: TrailItem): it is Applied => it.ev === "input.applied" && (it.kind === "prompt" || it.kind === "run_slash");

/** One row per prompt or slash command the session was given (or that the app sent it), oldest first. */
export function promptRows(src: PromptSources): PromptRow[] {
  const { session } = src;
  const hostClock = session.local ? "local" : session.host;
  const hostSource = session.local ? "hub" : session.hostName;

  // What the trail says of each input.
  const by = new Map<number, Partial<Record<"sent" | "written" | "acknowledged" | "dropped" | "write_failed" | "turn", TrailItem>>>();
  const slot = (n: number) => by.get(n) ?? by.set(n, {}).get(n)!;
  for (const it of src.trail) {
    if (it.ev === "input.sent") slot(it.input).sent = it;
    else if (it.ev === "input.written") slot(it.input).written = it;
    else if (it.ev === "input.acknowledged") slot(it.input).acknowledged = it;
    else if (it.ev === "input.dropped") slot(it.input).dropped = it;
    else if (it.ev === "input.write_failed" && typeof it.input === "number") slot(it.input).write_failed = it;
    // A turn the agent started by itself answers no prompt (the daemon gives it no input); only a turn that ran for an input is that input's.
    else if (it.ev === "turn.started" && it.trigger !== "agent" && typeof it.input === "number") slot(it.input).turn = it;
  }
  const applied = src.trail.filter(isPrompt);

  // What the app and the daemons recorded for a trace.
  const appFor = (trace: string) => src.app.filter((a) => "trace" in a.event && a.event.trace === trace);
  const hubFor = (trace: string) => src.hub.filter((r) => fieldStr(r, "trace") === trace);
  const hostFor = (trace: string) => src.host.filter((r) => fieldStr(r, "trace") === trace);
  /** The record of the request that carried the prompt: `session.command` when there was one (a prompt with attachments), else the launch that carried it. */
  const request = (rs: readonly DiagRecord[]) => {
    const traced = rs.filter((r) => r.ev === "rpc.traced");
    return traced.find((r) => fieldStr(r, "method") === "session.command") ?? traced.find((r) => fieldStr(r, "method") === "sessions.launch") ?? traced[0] ?? rs.find((r) => r.ev === "launch");
  };
  const outcome = (r: DiagRecord) => {
    const f = r.fields;
    const ms = typeof f.ms === "number" ? duration(f.ms) : "";
    const o = fieldStr(r, "outcome");
    const code = fieldStr(r, "code");
    const m = fieldStr(r, "method");
    return [m, o && o !== "ok" ? `${o}${code ? ` (${code})` : ""}` : o, ms].filter(Boolean).join(" · ");
  };

  const rowFor = (a: Applied | null, trace: string | null, app: { kind: "prompt" | "slash"; chars: number; attachments: number; bytes: number } | null): PromptRow => {
    const ids = a ? by.get(a.input) ?? {} : {};
    const appEvents = trace ? appFor(trace) : [];
    const appStep = appEvents.find((x) => x.event.ev === "ui.prompt") ?? appEvents.find((x) => x.event.ev === "ui.launch");
    const hub = trace ? request(hubFor(trace)) : undefined;
    const host = trace && !session.local ? request(hostFor(trace)) : undefined;
    const sessionSide = trace ? (session.local ? hubFor(trace) : hostFor(trace)) : [];
    const refused = sessionSide.find((r) => r.ev === "session.command_refused");
    const timedOut = sessionSide.find((r) => r.ev === "session.command_timeout");
    const noTrace = !trace;
    /** A leg nothing was seen of: still being read, never traced, or simply not seen. */
    const unread = (loaded: boolean, untraced: boolean): Partial<PathStep> => (untraced ? { detail: "no trace id" } : loaded ? {} : { state: "pending", detail: "log not read yet" });

    const step = (id: StepId, label: string, o: Partial<PathStep> & { seen?: boolean } = {}): PathStep => {
      const { seen, ...rest } = o;
      return { id, label, state: seen ? "seen" : "not_seen", tone: "plain", ...rest };
    };
    const at = (it: TrailItem | undefined) => (it ? { seen: true, t: it.t, clock: hostClock, source: hostSource } : { seen: false });

    // The path, in order. Terminal steps are added after the others are given their states.
    const path: PathStep[] = [
      step("app", "App", appStep ? { seen: true, t: appStep.t, clock: "local", source: "app", detail: appEvents.find((x) => x.event.ev === "ui.rpc" && x.event.method === "session.command") ? rpcDetail(appEvents) : undefined } : { detail: noTrace ? "no trace id" : undefined }),
      step("hub", "Hub", hub ? { seen: true, t: hub.t, clock: "local", source: "hub", detail: outcome(hub), tone: hub.fields.outcome && hub.fields.outcome !== "ok" ? "bad" : "plain" } : unread(src.loaded.hub, noTrace)),
    ];
    if (!session.local) path.push(step("host", session.hostName, host ? { seen: true, t: host.t, clock: session.host, source: session.hostName, detail: outcome(host), tone: host.fields.outcome && host.fields.outcome !== "ok" ? "bad" : "plain" } : unread(src.loaded.host, noTrace)));
    path.push(step("recorded", "Recorded", a ? { seen: true, t: a.t, clock: hostClock, source: hostSource } : {}));
    const terminals: PathStep[] = [];
    if (a) {
      const status = a.status;
      if (status === "queued") {
        path.push(step("queued", "Queued", { seen: true, t: a.t, clock: hostClock, source: hostSource }));
        path.push(step("sent", "Sent", at(ids.sent)));
      } else if (status === "sent") {
        path.push(step("sent", "Sent", { seen: true, t: a.t, clock: hostClock, source: hostSource }));
      } else {
        terminals.push(step("not_accepted", `Not accepted (${status})`, { seen: true, t: a.t, clock: hostClock, source: hostSource, tone: "bad", detail: a.reason ? errorWords(a.reason) : undefined }));
      }
      if (!terminals.length) {
        path.push(step("written", "Written", at(ids.written)));
        const ack = ids.acknowledged as Extract<TrailItem, { ev: "input.acknowledged" }> | undefined;
        path.push(step("acknowledged", "Acknowledged", { ...at(ack), detail: ack?.steered ? "steered into the running turn" : undefined, tone: ack ? "ok" : "plain" }));
        if (ids.turn) path.push(step("turn", "Turn started", at(ids.turn)));
      }
      const dropped = ids.dropped as Extract<TrailItem, { ev: "input.dropped" }> | undefined;
      if (dropped) terminals.push(step("dropped", `Dropped (${dropped.reason})`, { ...at(dropped), tone: "bad", detail: dropped.error ? errorWords(dropped.error) : undefined }));
      const wf = ids.write_failed as Extract<TrailItem, { ev: "input.write_failed" }> | undefined;
      if (wf) terminals.push(step("write_failed", "Write failed", { ...at(wf), tone: "bad", detail: errorWords(wf.error) }));
    } else if (refused) {
      terminals.push(step("refused", `Refused (${fieldStr(refused, "code") ?? "?"})`, { seen: true, t: refused.t, clock: hostClock, source: hostSource, tone: "bad", detail: "before it was recorded" }));
    }

    // Steps not seen: `not_seen` when something later was seen, `pending` after the last thing seen (none at all when the path ended).
    const lastSeen = path.reduce((n, s, i) => (s.state === "seen" ? i : n), -1);
    const states = path.flatMap((s, i): PathStep[] => (s.state !== "not_seen" || i < lastSeen ? [s] : terminals.length ? [] : [{ ...s, state: "pending" as const }]));
    const steps = [...states, ...terminals];

    // Times, relative to the first step seen; the host's are put on this computer's clock by the measured offset, and across machines they are still only approximate.
    const first = steps.find((s) => s.state === "seen" && s.t !== undefined);
    const corrected = (s: PathStep) => s.clock !== "local" && !!src.offset;
    const onOurClock = (s: PathStep) => s.t! - (corrected(s) ? src.offset!.offsetMs : 0);
    for (const s of steps) {
      if (s.state !== "seen" || s.t === undefined || !first || first.t === undefined) continue;
      s.rel = onOurClock(s) - onOurClock(first);
      s.approx = s.clock !== first.clock;
      if (corrected(s)) s.corrected = true;
    }

    const facts: { k: string; v: string }[] = [];
    if (a) facts.push({ k: "input", v: `#${a.input}` });
    if (trace) facts.push({ k: "trace", v: trace });
    const up = appEvents.find((x) => x.event.ev === "ui.upload");
    if (up && up.event.ev === "ui.upload") facts.push({ k: "upload", v: `${up.event.files} file${up.event.files === 1 ? "" : "s"}, ${sizeWords(up.event.bytes)}, ${duration(up.event.ms)}: ${up.event.outcome}${up.event.code ? ` (${up.event.code})` : ""}` });
    if (timedOut) facts.push({ k: "host", v: "the request timed out waiting for the session, but may have been applied later" });
    if (!a && !refused) facts.push({ k: "host", v: "the host has not recorded this prompt" });

    const size = a ?? app;
    return {
      key: a ? `input:${a.input}` : `trace:${trace}`,
      input: a?.input ?? null,
      trace,
      kind: a ? (a.kind === "run_slash" ? "slash" : "prompt") : app!.kind,
      chars: a?.chars ?? app?.chars ?? 0,
      attachments: size?.attachments ?? 0,
      bytes: size?.bytes ?? 0,
      steps,
      facts,
    };
  };

  const recorded = applied.map((a) => rowFor(a, a.trace ?? null, null));
  const seenTraces = new Set(applied.map((a) => a.trace).filter(Boolean));
  // Prompts the app sent that the host never recorded (refused, lost on the way, or still on it).
  const unrecorded = src.app
    .flatMap((a) => (a.event.ev === "ui.prompt" && a.event.host === session.host && a.event.session === session.id && !seenTraces.has(a.event.trace) ? [a.event] : []))
    .map((e) => rowFor(null, e.trace, { kind: e.kind === "slash" ? "slash" : "prompt", chars: e.chars, attachments: e.attachments, bytes: e.bytes }));
  return [...recorded, ...unrecorded];
}

/** How the request left the app: "answered ok in 12 ms". */
function rpcDetail(app: readonly AppEventRec[]): string | undefined {
  const r = app.find((x) => x.event.ev === "ui.rpc" && x.event.method === "session.command")?.event;
  return r && r.ev === "ui.rpc" ? `answered ${r.outcome}${r.code ? ` (${r.code})` : ""} in ${duration(r.ms)}` : undefined;
}

/** What a step shows for its time: "+14 ms", "≈+230 ms" (another machine's clock), "not seen" or "not yet". */
export function stepTime(s: PathStep): string {
  if (s.state === "pending") return "not yet";
  if (s.state === "not_seen") return "not seen";
  if (s.rel === undefined) return "";
  if (s.rel === 0) return "start";
  return `${s.approx ? "≈" : ""}${s.rel < 0 ? "-" : "+"}${duration(Math.abs(s.rel))}`;
}
