import type { ExecEvent, ExecStarted } from "../api";
import type { SubscriptionEnd, Unsubscribe } from "../backend";
import type { HostId } from "../wire";
import { err } from "./runtime";

/** How an execution ended, as a test (or the live script) reports it: the host's `exited` event. */
export interface ExecExit {
  code?: number | null;
  signal?: number | null;
  reason?: "time_limit" | "cancelled" | null;
  output_truncated?: boolean;
}

/** One `host.exec` execution: its events so far (numbered like the host's cursors) and who is subscribed. */
interface Exec {
  id: string;
  host: HostId;
  cwd: string;
  line: string;
  events: { seq: number; ev: ExecEvent }[];
  bytes: number;
  ended: boolean;
  subs: Set<{ onEvent: (e: ExecEvent) => void; onEnd: (end: SubscriptionEnd) => void }>;
}

/**
 * The hosts' `host.exec` executions, in memory. Output is produced when the test says so (`output`, `exit`), so a
 * test can watch a command stream and be cancelled. In live mode a command makes its own output and ends by itself.
 */
export class FixtureExecs {
  private execs = new Map<string, Exec>();
  private n = 0;

  constructor(private o: { live: boolean; later: (ms: number, fn: () => void) => void }) {}

  /** Every execution the fixture knows, in the order they started (tests). */
  get all(): { id: string; host: HostId; cwd: string; line: string; ended: boolean }[] {
    return [...this.execs.values()].map(({ id, host, cwd, line, ended }) => ({ id, host, cwd, line, ended }));
  }

  start(host: HostId, cwd: string, line: string): ExecStarted {
    const id = `ex${++this.n}`;
    this.execs.set(id, { id, host, cwd, line, events: [], bytes: 0, ended: false, subs: new Set() });
    if (this.o.live) {
      this.o.later(400, () => this.output(id, `ran: ${line}\n`));
      this.o.later(900, () => this.exit(id, { code: 0 }));
    }
    return { exec_id: id };
  }

  /** Ends the command the way `host.exec.cancel` does: killed, reason `cancelled`. Cancelling an ended one changes nothing. */
  cancel(id: string): void {
    const x = this.find(id);
    if (!x.ended) this.exit(id, { code: null, signal: 15, reason: "cancelled" });
  }

  /** Output the command writes (one chunk, on stdout unless `stream` says otherwise). */
  output(id: string, text: string, stream: "stdout" | "stderr" = "stdout"): void {
    const x = this.find(id);
    if (x.ended) throw new Error(`execution ${id} has ended`);
    this.push(x, { type: "output", stream, text });
  }

  /** The command ends: the `exited` event, then the stream ends. */
  exit(id: string, e: ExecExit): void {
    const x = this.find(id);
    if (x.ended) throw new Error(`execution ${id} has already ended`);
    this.push(x, {
      type: "exited",
      code: e.code ?? null,
      signal: e.signal ?? null,
      output_truncated: e.output_truncated ?? false,
      output_bytes: x.bytes,
      ...(e.reason ? { reason: e.reason } : {}),
    });
    x.ended = true;
    for (const s of [...x.subs]) s.onEnd(null);
    x.subs.clear();
  }

  /** `host.exec:<id>` from event `from` on (the host's replay), then live until the end. Unknown to this host: nothing. */
  attach(host: HostId, id: string, from: number, onEvent: (e: ExecEvent) => void, onEnd: (end: SubscriptionEnd) => void): Unsubscribe {
    const x = this.execs.get(id);
    if (!x || x.host !== host) return () => undefined;
    for (const r of x.events) if (r.seq > from) onEvent(r.ev);
    if (x.ended) {
      onEnd(null);
      return () => undefined;
    }
    const sub = { onEvent, onEnd };
    x.subs.add(sub);
    return () => x.subs.delete(sub);
  }

  private find(id: string): Exec {
    const x = this.execs.get(id);
    if (!x) throw err("not_found", `No execution ${id}`, { marsad_error: "not_found", what: `execution ${id}` });
    return x;
  }

  private push(x: Exec, ev: ExecEvent) {
    if (ev.type === "output") x.bytes += ev.text.length;
    x.events.push({ seq: x.events.length + 1, ev });
    for (const s of x.subs) s.onEvent(ev);
  }
}
