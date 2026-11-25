import type { Ack, ExportChunk, ExportEvent, ExportFile, ExportFormat, ExportReadRequest, HostId, SessionExportRequest, SessionExportStarted, SessionId } from "../wire";
import { err } from "./runtime";

/** The most a read returns (`MAX_EXPORT_READ` of the runtime). */
export const FIXTURE_EXPORT_READ_MAX = 1 << 20;

export interface SessionExportHooks {
  now(): number;
  /** Run `fn` after `ms` (scaled by the fixture's speed). */
  after(ms: number, fn: () => void): void;
  /** Deliver a subscription's first events: soon, or once the app is connected to the daemon. */
  deliver(fn: () => void): void;
  /** The session's title, as the export names it. */
  title(host: HostId, session: SessionId): string;
}

interface Sub {
  onEvent: (e: ExportEvent) => void;
  ended: (end: { code: string; message: string } | null) => void;
}

interface Run {
  id: string;
  host: HostId;
  session: SessionId;
  state: "running" | "done" | "failed";
  /** Every event so far: a subscriber from the start gets all of them (the topic replays). */
  events: ExportEvent[];
  subs: Set<Sub>;
  /** The files of a finished export, by format, with their text. */
  files: Map<ExportFormat, { path: string; text: string }>;
  steps: (() => void)[];
}

const FORMAT_EXT: Record<ExportFormat, string> = { json: "json", markdown: "md" };

/** Standard base64 of `bytes`. */
function base64Of(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
  return btoa(bin);
}

/**
 * Fixture-only: a session's conversation export (`session.export.start`, `.cancel`, `.read`, topic
 * `session.export:<id>`). A started export plays a scripted progression (two progress reports, then `done`
 * with the files and what was masked), one step at a time. Tests may play the steps by hand (`manual`,
 * `step`) and end an export as failed (`fail`). The files are short texts, so a read returns one chunk.
 */
export class FixtureSessionExports {
  /** Steps are played by `step()` only (a test looks at each state), not by themselves. */
  manual = false;
  /** Makes the next `start` fail with this error (a host that refuses). */
  refuse: { code: string; message: string } | null = null;
  /** What `done` counts as masked, by kind (tests change it). */
  redactions: Record<string, number> = { api_key: 2, password: 1 };
  private runs = new Map<string, Run>();
  private n = 0;

  constructor(private h: SessionExportHooks) {}

  start(host: HostId, r: SessionExportRequest): SessionExportStarted {
    if (this.refuse) throw err(this.refuse.code, this.refuse.message);
    if (r.formats.length === 0 || new Set(r.formats).size !== r.formats.length) throw err("invalid_params", "formats: each at most once, at least one");
    if (!Number.isInteger(r.tool_output_cap) || r.tool_output_cap < 1) throw err("invalid_params", "tool_output_cap must be at least 1");
    const busy = [...this.runs.values()].find((x) => x.session === r.session && x.state === "running");
    if (busy) throw err("conflict", `an export of this session is already running (${busy.id}); wait for it to end`);
    const id = `sx${String(++this.n).padStart(3, "0")}`;
    const title = this.h.title(host, r.session);
    const files = new Map<ExportFormat, { path: string; text: string }>();
    for (const f of r.formats) {
      const path = `~/.local/share/marsad/sessions/${r.session}/exports/conversation-${id}.${FORMAT_EXT[f]}`;
      files.set(f, { path, text: f === "json" ? `${JSON.stringify({ session: r.session, title, events: [] }, null, 2)}\n` : `# ${title}\n\n_Exported by the fixture; ${r.include_subagents ? "with" : "without"} subagents._\n` });
    }
    const run: Run = { id, host, session: r.session, state: "running", events: [], subs: new Set(), files, steps: [] };
    this.runs.set(id, run);
    const total = [...files.values()].reduce((a, f) => a + f.text.length, 0);
    run.steps = [
      () => this.emit(run, { event: "progress", events_done: 40, bytes: Math.round(total / 3) }),
      () => this.emit(run, { event: "progress", events_done: 120, bytes: Math.round((total * 2) / 3) }),
      () => {
        const out: ExportFile[] = [...files].map(([format, f]) => ({ format, path: f.path, bytes: new TextEncoder().encode(f.text).length }));
        this.emit(run, { event: "done", files: out, redactions: { ...this.redactions } });
        run.state = "done";
      },
    ];
    if (!this.manual) this.play(run);
    return { export_id: id };
  }

  /** Play the next step of the running exports; false when there is none. */
  step(): boolean {
    const run = [...this.runs.values()].find((x) => x.steps.length > 0);
    const next = run?.steps.shift();
    next?.();
    return !!next;
  }

  /** End the running export `id` (or the only running one) as failed, with `reason`. */
  fail(reason: string, id?: string) {
    const run = [...this.runs.values()].find((x) => x.state === "running" && (id === undefined || x.id === id));
    if (!run) return;
    run.steps = [];
    this.emit(run, { event: "failed", reason });
  }

  /** `session.export.cancel`: a running export ends as failed; a finished one is left as it is. */
  cancel(host: HostId, id: string): Ack {
    const run = this.find(host, id);
    if (run.state === "running") {
      run.steps = [];
      this.emit(run, { event: "failed", reason: "cancelled" });
    }
    return {};
  }

  /** `session.export.read`: `len` bytes of a finished export's file from `offset`, base64. */
  read(host: HostId, r: ExportReadRequest): ExportChunk {
    const run = this.find(host, r.export_id);
    if (run.state !== "done") throw err("conflict", `export ${r.export_id} is not finished`);
    if (!Number.isInteger(r.len) || r.len < 1 || r.len > FIXTURE_EXPORT_READ_MAX) throw err("invalid_params", `len must be 1..=${FIXTURE_EXPORT_READ_MAX}`);
    if (r.format == null && run.files.size > 1) throw err("invalid_params", "give the format: the export wrote more than one file");
    const f = r.format == null ? [...run.files.values()][0] : run.files.get(r.format);
    if (!f) throw err("not_found", `export ${r.export_id} has no ${r.format} file`);
    const bytes = new TextEncoder().encode(f.text);
    const offset = Math.min(r.offset, bytes.length);
    const end = Math.min(offset + r.len, bytes.length);
    return { offset, total: bytes.length, eof: end >= bytes.length, data: base64Of(bytes.subarray(offset, end)) };
  }

  /**
   * The progress topic of export `id` for a subscriber: every event from the start (`from` absent) or after
   * cursor `from`, then the live ones. The subscriber's `ended` is called once the export is finished.
   */
  attach(host: HostId | undefined, id: string, from: string | number | undefined, onEvent: (e: ExportEvent) => void, ended: Sub["ended"]): () => void {
    const run = host === undefined ? undefined : this.runs.get(id);
    if (!run || run.host !== host) {
      this.h.deliver(() => ended({ code: "not_found", message: `no such export: ${id}` }));
      return () => undefined;
    }
    const sub: Sub = { onEvent, ended };
    run.subs.add(sub);
    const after = from === undefined ? 0 : Number(from);
    this.h.deliver(() => {
      if (!run.subs.has(sub)) return;
      run.events.forEach((e, i) => {
        if (i + 1 > after && run.subs.has(sub)) onEvent(e);
      });
      if (run.state !== "running" && run.subs.delete(sub)) ended(null);
    });
    return () => run.subs.delete(sub);
  }

  /** The text of export `id`'s file in `format` (tests compare reads with it). */
  text(id: string, format: ExportFormat): string {
    const f = this.runs.get(id)?.files.get(format);
    if (!f) throw new Error(`no ${format} file of export ${id}`);
    return f.text;
  }

  private emit(run: Run, e: ExportEvent) {
    run.events.push(e);
    const terminal = e.event !== "progress";
    for (const sub of [...run.subs]) {
      sub.onEvent(e);
      if (terminal) {
        run.subs.delete(sub);
        sub.ended(null);
      }
    }
    if (e.event === "failed") run.state = "failed";
  }

  private play(run: Run) {
    if (!run.steps.length) return;
    this.h.after(500, () => {
      if (this.manual) return;
      run.steps.shift()?.();
      this.play(run);
    });
  }

  private find(host: HostId, id: string): Run {
    const run = this.runs.get(id);
    if (!run || run.host !== host) throw err("not_found", `no such export: ${id}`);
    return run;
  }
}
