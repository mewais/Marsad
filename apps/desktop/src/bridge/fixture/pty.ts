import type { HostId, PtyEvent, PtyInfo, PtyStarted, PtyStartRequest } from "../wire";
import type { SubscriptionEnd, Unsubscribe } from "../backend";
import { err } from "./runtime";

/**
 * Fixture-only: the host's terminals (marsad-agent `host/pty.rs`) with a scripted shell, following
 * the same contract:
 *
 * - a session has one terminal; `start` for it returns the live one (`created: false`), replaces an
 *   exited one and otherwise starts one; a directory target always starts a new terminal, up to
 *   `MAX_DIRECTORY_TERMINALS`;
 * - output has byte offsets; a subscription replays the scrollback from `from` (0 = from the start)
 *   and then follows live; `exited` closes the stream (its cursor is the end offset plus one), and a
 *   later subscriber reads the scrollback, then `exited`;
 * - a terminal ends when its shell exits, when it is closed or when its session is deleted.
 */

/** Terminals not tied to a session a host keeps at most (the host's `MAX_DIRECTORY_TERMINALS`). */
export const MAX_DIRECTORY_TERMINALS = 8;
const SCROLLBACK = 256 * 1024;
const enc = new TextEncoder();

const b64 = (bytes: Uint8Array) => {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
};

interface Sub {
  send: (e: PtyEvent) => void;
  end: (e: SubscriptionEnd) => void;
}

interface Term {
  id: string;
  host: HostId;
  cwd: string;
  session: string | null;
  cols: number;
  rows: number;
  /** Offset of `data[0]`. */
  start: number;
  data: Uint8Array;
  exit: { code: number | null } | null;
  /** The line being typed. */
  line: string;
  subs: Set<Sub>;
  /** Everything `pty.input` carried, in order (tests). */
  input: string[];
}

export interface PtyHooks {
  /** Deliver a subscription's events: soon, and once the app is connected to the daemon. */
  deliver(fn: () => void): void;
}

/** What a request for a terminal names: the working directory, and the session it belongs to. */
export interface PtyTarget {
  cwd: string;
  session: string | null;
}

export class FixturePtys {
  private terms = new Map<string, Term>();
  private n = 0;

  constructor(private hooks: PtyHooks) {}

  private find(host: HostId, id: string): Term {
    const t = this.terms.get(id);
    if (!t || t.host !== host) throw err("not_found", `not found: terminal ${id}`, { marsad_error: "not_found", what: `terminal ${id}` });
    return t;
  }

  private info(t: Term): PtyInfo {
    return { id: t.id, cwd: t.cwd, session: t.session, cols: t.cols, rows: t.rows, offset: t.start + t.data.length, exited: t.exit != null, ...(t.exit?.code != null ? { exit_code: t.exit.code } : {}) };
  }

  list(host: HostId): PtyInfo[] {
    return [...this.terms.values()].filter((t) => t.host === host).map((t) => this.info(t)).sort((a, b) => a.id.localeCompare(b.id));
  }

  start(host: HostId, r: PtyStartRequest, target: PtyTarget): PtyStarted {
    if (r.cols === 0 || r.rows === 0) throw err("invalid_params", "invalid request: cols and rows must be positive", { marsad_error: "invalid_request", message: "cols and rows must be positive" });
    const mine = [...this.terms.values()].filter((t) => t.host === host);
    if (target.session != null) {
      const have = mine.find((t) => t.session === target.session);
      if (have && !have.exit) return { pty: this.info(have), created: false };
      // An exited terminal is replaced: one slot per session.
      if (have) this.terms.delete(have.id);
    } else if (mine.filter((t) => t.session == null).length >= MAX_DIRECTORY_TERMINALS) {
      const message = `this host keeps at most ${MAX_DIRECTORY_TERMINALS} terminals that are not tied to a session; close one first`;
      throw err("limit_reached", `limit reached: ${message}`, { marsad_error: "limit_reached", message });
    }
    const t: Term = { id: `pty${String(++this.n).padStart(6, "0")}`, host, cwd: target.cwd, session: target.session, cols: r.cols, rows: r.rows, start: 0, data: new Uint8Array(0), exit: null, line: "", subs: new Set(), input: [] };
    this.terms.set(t.id, t);
    this.print(t, this.prompt(t));
    return { pty: this.info(t), created: true };
  }

  input(host: HostId, id: string, data: string) {
    const t = this.find(host, id);
    if (t.exit) throw err("invalid_params", `invalid request: terminal ${id} has exited`, { marsad_error: "invalid_request", message: `terminal ${id} has exited` });
    t.input.push(data);
    this.type(t, data);
  }

  resize(host: HostId, id: string, cols: number, rows: number) {
    const t = this.find(host, id);
    if (cols === 0 || rows === 0) throw err("invalid_params", "invalid request: cols and rows must be positive", { marsad_error: "invalid_request", message: "cols and rows must be positive" });
    if (t.exit) throw err("invalid_params", "invalid request: the terminal has exited", { marsad_error: "invalid_request", message: "the terminal has exited" });
    t.cols = cols;
    t.rows = rows;
  }

  /** `pty.close`: the terminal is forgotten and its shell ended (subscribers see `exited`). */
  close(host: HostId, id: string) {
    const t = this.find(host, id);
    this.terms.delete(id);
    this.end(t, 129);
  }

  /** A session was deleted: its terminal ends with it. */
  sessionDeleted(host: HostId, session: string) {
    for (const t of [...this.terms.values()]) {
      if (t.host !== host || t.session !== session) continue;
      this.terms.delete(t.id);
      this.end(t, 129);
    }
  }

  /** The shell of a terminal exits by itself (tests; what typing `exit` does). */
  exit(host: HostId, id: string, code: number) {
    this.end(this.find(host, id), code);
  }

  /** `pty.output:<id>` from offset `from` (the start when omitted). `end` is called when the stream is over. */
  subscribe(host: HostId, id: string, from: number | undefined, send: (e: PtyEvent) => void, end: (e: SubscriptionEnd) => void): Unsubscribe {
    let t: Term;
    try {
      t = this.find(host, id);
    } catch (e) {
      let live = true;
      const rpc = e as { code: string; message: string; data?: unknown };
      this.hooks.deliver(() => live && end({ code: rpc.code, message: rpc.message, data: rpc.data }));
      return () => void (live = false);
    }
    const sub: Sub = { send, end };
    const at = from ?? 0;
    const endAt = t.start + t.data.length;
    // Output cursors reach the end offset; `exited` is the one after it.
    if (at > (t.exit ? endAt + 1 : endAt)) {
      this.hooks.deliver(() => end({ code: "invalid_params", message: `invalid request: cursor ${at} is beyond the output of terminal ${id}`, data: { marsad_error: "invalid_request", message: `cursor ${at} is beyond the output of terminal ${id}` } }));
      return () => undefined;
    }
    t.subs.add(sub);
    // What the terminal holds now is fixed here; what it prints after this goes out on its own, after this.
    const first = at < endAt ? Math.max(at, t.start) : null;
    const replay: PtyEvent | null = first == null ? null : { kind: "output", offset: first, data: b64(t.data.slice(first - t.start)) };
    const exit = t.exit;
    this.hooks.deliver(() => {
      if (!t.subs.has(sub)) return;
      if (replay) send(replay);
      if (exit) {
        t.subs.delete(sub);
        if (at <= endAt) send({ kind: "exited", ...(exit.code != null ? { code: exit.code } : {}) });
        end(null);
      }
    });
    return () => void t.subs.delete(sub);
  }

  /** How many subscriptions to terminal output are open (tests). */
  get subscribers(): number {
    let n = 0;
    for (const t of this.terms.values()) n += t.subs.size;
    return n;
  }

  /** What `pty.input` carried for the terminal of `session` (or the only terminal of the host), in order (tests). */
  inputOf(host: HostId, session?: string): string[] {
    const t = [...this.terms.values()].find((x) => x.host === host && (session === undefined || x.session === session));
    return t ? [...t.input] : [];
  }

  /** What the terminal's shell has printed so far, as text (tests). */
  screenOf(host: HostId, id: string): string {
    const t = this.find(host, id);
    return new TextDecoder().decode(t.data);
  }

  private prompt(t: Term) {
    return `me@${t.host}:${t.cwd}$ `;
  }

  /** What the shell does with typed bytes: echo, edit the line, run it on Enter. Escape sequences are the shell's and are ignored. */
  private type(t: Term, data: string) {
    for (let i = 0; i < data.length && !t.exit; i++) {
      const c = data[i];
      if (c === "\x1b") {
        // Esc, or a control sequence (`Esc [ … final`): consumed, nothing echoed.
        if (data[i + 1] === "[") {
          i += 2;
          while (i < data.length && !/[@-~]/.test(data[i])) i++;
        }
      } else if (c === "\r" || c === "\n") {
        const line = t.line;
        t.line = "";
        this.print(t, "\r\n");
        this.run(t, line.trim());
        if (!t.exit) this.print(t, this.prompt(t));
      } else if (c === "\x7f") {
        if (t.line) {
          t.line = t.line.slice(0, -1);
          this.print(t, "\b \b");
        }
      } else if (c === "\x03") {
        t.line = "";
        this.print(t, `^C\r\n${this.prompt(t)}`);
      } else if (c >= " ") {
        t.line += c;
        this.print(t, c);
      }
    }
  }

  private run(t: Term, line: string) {
    if (!line) return;
    const [cmd, ...args] = line.split(/\s+/);
    if (cmd === "echo") this.print(t, `${args.join(" ")}\r\n`);
    else if (cmd === "pwd") this.print(t, `${t.cwd}\r\n`);
    else if (cmd === "ls") this.print(t, "README.md  package.json  src\r\n");
    else if (cmd === "exit") this.end(t, args[0] !== undefined && /^\d+$/.test(args[0]) ? Number(args[0]) : 0);
    else this.print(t, `sh: 1: ${cmd}: not found\r\n`);
  }

  /** Output of the shell: into the scrollback and to every subscriber. */
  private print(t: Term, text: string) {
    const bytes = enc.encode(text);
    const offset = t.start + t.data.length;
    const all = new Uint8Array(t.data.length + bytes.length);
    all.set(t.data);
    all.set(bytes, t.data.length);
    const excess = Math.max(0, all.length - SCROLLBACK);
    t.data = all.slice(excess);
    t.start += excess;
    const ev: PtyEvent = { kind: "output", offset, data: b64(bytes) };
    for (const s of [...t.subs]) this.hooks.deliver(() => t.subs.has(s) && s.send(ev));
  }

  /** The shell has exited: `exited` goes to every subscriber, then their streams end. */
  private end(t: Term, code: number) {
    if (t.exit) return;
    t.exit = { code };
    t.line = "";
    for (const s of [...t.subs]) {
      this.hooks.deliver(() => {
        if (!t.subs.delete(s)) return;
        s.send({ kind: "exited", code });
        s.end(null);
      });
    }
  }
}
