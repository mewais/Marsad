import type { DiagTailEvent, DiagTailParams, SessionEvent, TrailCause, TrailEntry, TrailItem } from "../wire";
import type { Unsubscribe } from "../backend";
import { matchesFilter, parseRecord } from "../../lib/diagRecord";

/**
 * Fixture-only: what the daemons keep for diagnostics. Each daemon (the hub, every connected
 * host) has a log of NDJSON records in the format of `wasit-diag` (`b, seq, t, m, src, lvl, ev` and
 * the event's fields) that `diag.tail` serves; a session's trail is the projection of its events
 * (`trail.rs`), and a few sessions have a scripted history with the facts the scripted agent does
 * not produce (a dropped prompt, an anomaly, a failure).
 */

// ── logs ────────────────────────────────────────────────────────────────────

const RING = 4096;

/** One daemon's diagnostics log: the records it has written (the ring) and the tails open on it. */
export class FixtureLog {
  private seq = 0;
  private lines: { line: string; id: string }[] = [];
  private subs = new Set<{ params: DiagTailParams; cb: (e: DiagTailEvent) => void }>();

  constructor(
    private src: "hub" | "daemon",
    private boot: string,
    private now: () => number,
    private started: number,
  ) {}

  /** Write a record. `t` is its wall time (default: now). */
  emit(ev: string, fields: Record<string, unknown> = {}, lvl: "debug" | "info" | "warn" | "error" = "info", t = this.now()): void {
    const seq = ++this.seq;
    const line = JSON.stringify({ b: this.boot, seq, t, m: Math.max(0, t - this.started), src: this.src, lvl, ev, ...fields });
    this.lines.push({ line, id: `${this.boot}.${seq}` });
    if (this.lines.length > RING) this.lines.shift();
    for (const s of this.subs) if (this.matches(line, s.params)) s.cb({ kind: "record", line });
  }

  private matches(line: string, p: DiagTailParams): boolean {
    const r = parseRecord(line);
    return !!r && matchesFilter(r, p);
  }

  /**
   * Serve a tail like the daemon: from no cursor, or one that is not in the ring, it starts with a
   * `reset` of what the ring holds that matches; from one that is, with the records after it.
   */
  tail(params: DiagTailParams, from: string | undefined, cb: (e: DiagTailEvent) => void): Unsubscribe {
    const at = from === undefined ? -1 : this.lines.findIndex((l) => l.id === from);
    const matching = (ls: typeof this.lines) => ls.filter((l) => this.matches(l.line, params)).map((l) => l.line);
    if (at < 0) cb({ kind: "reset", records: matching(this.lines) });
    else for (const line of matching(this.lines.slice(at + 1))) cb({ kind: "record", line });
    const sub = { params, cb };
    this.subs.add(sub);
    return () => this.subs.delete(sub);
  }
}

/** Every record, as the lines the ring holds (tests). */
export const linesOf = (l: FixtureLog): string[] => (l as unknown as { lines: { line: string }[] }).lines.map((x) => x.line);

// ── trails ──────────────────────────────────────────────────────────────────

/** Length and fingerprint of a text, as the daemon keeps a foreign one (never the text). */
export function foreignError(code: string, text: string) {
  let h = 5381;
  for (const c of text.replace(/\d/g, "0")) h = ((h * 33) ^ c.charCodeAt(0)) >>> 0;
  return { code, len: [...text].length, fp: h.toString(16).padStart(8, "0") };
}

const flags = (argv: string[]) => argv.filter((a) => /^--?[a-z][a-z0-9-]{0,40}$/.test(a)).slice(0, 32);
const nonNull = <T extends Record<string, unknown>>(o: T): T => Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined && v !== null)) as T;

type Entry = TrailEntry;

/**
 * The trail entries of one session event (none for the content itself), as `trail::project` makes
 * them, plus what the scripted host does that has no event of its own: the write of a sent input
 * and the agent's acknowledgement when its turn starts. Several entries of one event take the
 * cursor of the event plus their index, as the runtime's derived events do.
 */
export function projectEvent(events: readonly SessionEvent[], i: number): TrailItem[] {
  const ev = events[i];
  const out: Entry[] = [];
  switch (ev.kind) {
    case "spawned":
      out.push({ ev: "process.spawned", generation: ev.generation, argc: ev.argv.length, flags: flags(ev.argv), pid: ev.pid });
      break;
    case "started":
      out.push(nonNull({ ev: "process.started" as const, generation: ev.generation, resume: ev.resume, resume_id: ev.resume_id, model: ev.model, mode: ev.mode, first_input: ev.first_input, trace: ev.trace }));
      break;
    case "input": {
      const c = ev.command;
      const files = c.command === "prompt" ? c.attachments ?? [] : [];
      const target = c.command === "answer_gate" || c.command === "answer_question" ? c.request_id : c.command === "set_model" ? c.model : c.command === "set_mode" ? c.mode : c.command === "run_slash" ? c.name : undefined;
      const text = c.command === "prompt" ? c.text : c.command === "run_slash" ? c.args : c.command === "answer_gate" ? c.decision.text ?? "" : "";
      const s = ev.status;
      out.push(
        nonNull({
          ev: "input.applied" as const,
          input: ev.input,
          kind: c.command,
          chars: [...text].length,
          attachments: files.length,
          bytes: files.reduce((a, f) => a + f.size, 0),
          status: s.status,
          trace: ev.trace,
          target,
          choice: c.command === "answer_gate" ? c.decision.choice : undefined,
          reason: s.status === "rejected" || s.status === "not_running" ? foreignError(s.status, s.reason) : s.status === "unsupported" ? foreignError(s.status, s.operation) : undefined,
        }),
      );
      if (s.status === "sent") out.push({ ev: "input.written", input: ev.input });
      break;
    }
    case "written":
      out.push({ ev: "input.written", input: ev.input });
      break;
    case "write_failed":
      out.push(nonNull({ ev: "input.write_failed" as const, error: { code: "stdin_write", text: ev.error }, input: ev.input }));
      break;
    case "stderr":
      out.push({ ev: "process.stderr", chars: [...ev.text].length });
      break;
    case "phase":
      out.push(nonNull({ ev: "phase" as const, to: ev.phase.state, cause: nonNull({ ...ev.cause }) as TrailCause, reason: ev.phase.state === "failed" || ev.phase.state === "lost" ? ev.phase.reason : undefined }));
      break;
    case "anomaly":
      out.push({ ev: "anomaly", kind: ev.anomaly, first: ev.first });
      break;
    case "exited":
      out.push(nonNull({ ev: "process.exited" as const, code: ev.code, signal: ev.signal }));
      break;
    case "lost":
      out.push({ ev: "process.lost", reason: ev.reason });
      break;
    case "notice":
      out.push({ ev: "runtime.notice", code: ev.code });
      break;
    case "agent":
      out.push(...projectAgent(events, i));
      break;
  }
  return out.map((entry, j) => ({ cursor: ev.cursor + j, t: ev.t, ...entry }) as TrailItem);
}

function projectAgent(events: readonly SessionEvent[], i: number): Entry[] {
  const a = (events[i] as Extract<SessionEvent, { kind: "agent" }>).event;
  switch (a.type) {
    case "session_init":
      return [nonNull({ ev: "agent.init" as const, tools: a.tools?.length ?? 0, mcp_servers: a.mcp_servers?.length ?? 0, slash_commands: a.slash_commands?.length ?? 0, native_session_id: a.native_session_id, model: a.model, cli_version: a.cli_version, mode: a.current_mode })];
    case "turn_started": {
      // The agent's first sign of life for an input is its acknowledgement, unless it said so already.
      const acked = a.input != null && events.slice(0, i).some((e) => e.kind === "agent" && e.event.type === "input_acknowledged" && e.event.input === a.input);
      return [...(a.input != null && !acked ? [{ ev: "input.acknowledged" as const, input: a.input, steered: false }] : []), nonNull({ ev: "turn.started" as const, input: a.input, native_turn_id: a.native_turn_id })];
    }
    case "input_sent":
      return [{ ev: "input.sent", input: a.input }, { ev: "input.written", input: a.input }];
    case "input_acknowledged":
      return [{ ev: "input.acknowledged", input: a.input, steered: a.steered ?? false }];
    case "input_dropped":
      return [nonNull({ ev: "input.dropped" as const, input: a.input, reason: a.reason, error: a.error ? foreignError("dropped", a.error) : undefined })];
    case "turn_completed":
      return [nonNull({ ev: "turn.completed" as const, interrupted: a.was_interrupted, num_turns: a.num_turns, input_tokens: a.usage.input_tokens, output_tokens: a.usage.output_tokens, cost_usd_micros: Math.round(a.usage.cost_usd * 1e6), reply_chars: [...(a.reply ?? "")].length, end_reason: a.end_reason })];
    case "tool_call_started":
      return [{ ev: "tool.started", call_id: a.call_id, name: a.name, subagent: a.is_subagent }];
    case "tool_call_finished":
      return [{ ev: "tool.finished", call_id: a.call_id, ok: a.ok }];
    case "gate_requested": {
      const g = a.gate;
      return [nonNull({ ev: "gate.raised" as const, request_id: g.request_id, gate_type: g.gate_type, choices: g.gate_type === "question" ? 0 : g.choices.length, tool: g.gate_type === "permission" ? g.tool_name : undefined })];
    }
    case "gate_resolved":
      return [nonNull({ ev: "gate.resolved" as const, request_id: a.request_id, reason: a.reason?.code })];
    case "model_changed":
      return [{ ev: "agent.model", model: a.model }];
    case "modes_updated":
      return a.current_mode ? [{ ev: "agent.mode", mode: a.current_mode }] : [];
    case "command_rejected":
      return [{ ev: "agent.rejected", command: a.command, error: foreignError("rejected", a.message) }];
    case "notice":
      return [{ ev: "agent.notice", error: foreignError("notice", a.message) }];
    case "compaction":
      return [nonNull({ ev: "agent.compaction" as const, pre_tokens: a.info.pre_tokens, post_tokens: a.info.post_tokens, duration_ms: a.info.duration_ms, trigger: a.info.trigger })];
    case "rate_limit_notice":
      return [nonNull({ ev: "agent.rate_limited" as const, resets_at: a.info.resets_at, is_using_overage: a.info.is_using_overage, status: a.info.status, rate_limit_type: a.info.rate_limit_type })];
    case "unknown":
      return [{ ev: "agent.unknown", raw_type: String((a.raw as { type?: unknown } | null)?.type ?? "line") }];
    default:
      return [];
  }
}

/** The trail of a session: `base` (a scripted history, if it has one) and the projection of the events from `from` on. */
export function trailOf(events: readonly SessionEvent[], base: readonly TrailItem[] = [], from = 0): TrailItem[] {
  const live = events.flatMap((_, i) => (i >= from ? projectEvent(events, i) : []));
  return [...base, ...live];
}

/** A scripted trail item at `cursor`/`t` (anchored to an event of the history). */
const at = (e: SessionEvent, j: number, entry: Entry, dt = 0): TrailItem => ({ cursor: e.cursor + j, t: e.t + dt, ...entry }) as TrailItem;

/**
 * Scripted histories (by spec number) for the sessions that tell a story: facts the scripted
 * agent never produces itself. Each anchors its items to events of the session's own history.
 */
export function scriptedTrail(specN: number, events: readonly SessionEvent[]): TrailItem[] | null {
  const n = events.length;
  if (n < 6) return null;
  const E = (i: number) => events[Math.min(n - 1, Math.max(0, i))];
  const ok = { ev: "input.acknowledged" as const, steered: false };
  const common = (trace: string): TrailItem[] => [
    at(E(0), 0, { ev: "process.spawned", generation: 1, argc: 5, flags: ["--output-format", "--input-format", "--verbose"], pid: 4100 + specN }),
    at(E(0), 1, { ev: "phase", to: "starting", cause: { kind: "spawned", generation: 1 } }),
    at(E(0), 2, { ev: "process.started", generation: 1, resume: "fresh", model: "claude-sonnet-4.5", mode: "default", first_input: 3, trace }),
    at(E(1), 0, { ev: "agent.init", tools: 14, mcp_servers: 2, slash_commands: 9, native_session_id: `native-${specN}`, model: "claude-sonnet-4.5", cli_version: "2.1.4", mode: "default" }),
    at(E(1), 1, { ev: "phase", to: "idle", cause: { kind: "handshake_done" } }, 400),
    at(E(2), 0, { ev: "input.applied", input: 3, kind: "prompt", chars: 61, attachments: 0, bytes: 0, status: "sent", trace }),
    at(E(2), 1, { ev: "input.written", input: 3 }),
    at(E(3), 0, { ...ok, input: 3 }),
    at(E(3), 1, { ev: "turn.started", input: 3 }),
    at(E(3), 2, { ev: "phase", to: "working", cause: { kind: "turn_started", input: 3 } }),
  ];
  // A long working session: prompts typed while it worked were held and then lost to an interrupt (a few minutes ago); the agent kept producing output after its turn had ended.
  if (specN === 10) {
    const mid = Math.max(4, n - 12);
    return [
      ...common("u-7c1e0a52b8d4f396"),
      ...events.slice(4, mid).flatMap((_, i) => projectEvent(events, i + 4)),
      at(E(mid), 0, { ev: "input.applied", input: 640, kind: "prompt", chars: 58, attachments: 0, bytes: 0, status: "queued", trace: "u-2a9f4c01d37be685" }),
      at(E(mid), 1, { ev: "input.applied", input: 641, kind: "prompt", chars: 23, attachments: 0, bytes: 0, status: "queued", trace: "u-b84d1e6f0c7a2935" }, 100),
      at(E(mid), 2, { ev: "input.applied", input: 642, kind: "interrupt", chars: 0, attachments: 0, bytes: 0, status: "sent" }, 400),
      at(E(mid), 3, { ev: "input.dropped", input: 640, reason: "interrupted" }, 410),
      at(E(mid), 4, { ev: "input.dropped", input: 641, reason: "interrupted" }, 420),
      at(E(mid), 5, { ev: "turn.completed", interrupted: true, num_turns: 4, input_tokens: 48200, output_tokens: 3100, cost_usd_micros: 410000, reply_chars: 0, end_reason: "cancelled" }, 500),
      at(E(mid), 6, { ev: "phase", to: "idle", cause: { kind: "turn_completed", interrupted: true, end_reason: "cancelled" } }, 510),
      at(E(mid), 7, { ev: "tool.started", call_id: "late-1", name: "Edit", subagent: false }, 600),
      at(E(mid), 8, { ev: "anomaly", kind: "activity_while_idle", first: "tool_call_started" }, 610),
      at(E(mid), 9, { ev: "phase", to: "working", cause: { kind: "turn_started" } }, 700),
      ...events.slice(mid + 1).flatMap((_, i) => projectEvent(events, i + mid + 1)),
    ];
  }
  // A session that failed: the process exited with an error and said why.
  if (specN === 5) {
    const last = n - 1;
    return [
      ...common("u-5e0b9a14c7d82f63"),
      ...events.slice(4, last).flatMap((_, i) => projectEvent(events, i + 4)).filter((t) => t.ev !== "phase"),
      at(E(last), 0, { ev: "process.stderr", chars: 118 }, -300),
      at(E(last), 1, { ev: "process.exited", code: 1 }, -200),
      at(E(last), 2, { ev: "phase", to: "failed", cause: { kind: "exited", requested: false, handshake_timeout: false, code: 1 }, reason: "pnpm test exited 1" }),
    ];
  }
  return null;
}

// ── seeded logs ─────────────────────────────────────────────────────────────

const MIN = 60_000;
const link = (id: string) => `hosts:${id}`;

/** A connect attempt of the hub to a host that succeeded, as the connection trail records it. */
export function connectedAttempt(log: FixtureLog, host: string, n: number, t: number, o: { build?: string; os?: string; arch?: string } = {}) {
  const { build = "0.7.3", os = "linux", arch = "x86_64" } = o;
  log.emit("link.attempt_begin", { link: link(host), n }, "info", t);
  log.emit("transport.spawned", { link: link(host), n, program: "ssh", argc: 9, pid: 20_000 + n }, "info", t + 4);
  log.emit("boot.attempt", { host, n, shell: os === "windows" ? "powershell" : "posix", first_contact: n === 1, interactive: false, armed: false }, "info", t + 6);
  log.emit("boot.preamble", { host, n, lines: 0, bytes: 0 }, "info", t + 380);
  log.emit("boot.report", { host, n, os, arch, installed: build, ms: 372 }, "info", t + 392);
  log.emit("boot.action", { host, n, action: "run", target: build }, "info", t + 393);
  log.emit("boot.ok", { host, n, ms: 38 }, "info", t + 431);
  log.emit("boot.hello", { host, n, build, host_id_matches: true, machine: `m-${host}`, ms: 44 }, "info", t + 475);
  log.emit("link.attempt_end", { link: link(host), n, outcome: "connected", ms: 482 }, "info", t + 482);
  log.emit("link.state", { link: link(host), from: "connecting", to: "connected" }, "info", t + 483);
  log.emit("fleet.feed", { host, event: "started" }, "info", t + 490);
}

/** The hub's log as it would be after a morning of use: every host's connection history. Returns each host's last attempt number. */
export function seedHub(log: FixtureLog, now: number): Record<string, number> {
  const t0 = now - 3 * 60 * MIN;
  log.emit("process.start", { role: "hub", build: "fixture-0.1.0", target: "x86_64-unknown-linux-gnu", os: "linux", arch: "x86_64", pid: 4242 }, "info", t0);
  log.emit("daemon.start", { hub: true, host_id: "x1", build: "fixture-0.1.0", pid: 4242, machine: "m-x1", data: "~/.local/share/marsad", run_dir: "~/.local/share/marsad/run", socket: "~/.local/share/marsad/run/hub.sock", free_text_logging: false }, "info", t0 + 20);
  log.emit("daemon.ready", { total_ms: 212, env_ms: 120, adopt_ms: 31, node_ms: 9, hub_ms: 14, line_written: true }, "info", t0 + 232);
  // hz: connected on the first try.
  connectedAttempt(log, "hz", 1, t0 + 400);
  log.emit("hostenv.apply", { host: "hz", revision: "r3", outcome: "noop", ms: 12 }, "info", t0 + 1100);
  // win: up for two hours, then lost; every attempt since has failed, and the last one says why.
  connectedAttempt(log, "win", 1, t0 + 500, { os: "windows" });
  log.emit("link.closed", { link: link("win"), reason: "io", connected_ms: 2 * 60 * MIN, transport_exit: 255, stderr: "Connection reset by peer" }, "warn", now - 58 * MIN);
  log.emit("link.state", { link: link("win"), from: "connected", to: "reconnecting", attempt: 1, error: "connection reset by peer (ssh exited with code 255)" }, "info", now - 58 * MIN);
  for (const [n, dt, why] of [[2, 57 * MIN, "Connection timed out"], [3, 55 * MIN, "Connection reset by peer"]] as const) {
    const t = now - dt;
    log.emit("link.attempt_begin", { link: link("win"), n }, "info", t);
    log.emit("transport.spawned", { link: link("win"), n, program: "ssh", argc: 9, pid: 21_000 + n }, "info", t + 5);
    log.emit("boot.attempt", { host: "win", n, shell: "powershell", first_contact: false, interactive: false, armed: false }, "info", t + 8);
    log.emit("transport.exited", { link: link("win"), n, code: 255, stderr_bytes: 62 }, "info", t + 10_030);
    log.emit("boot.failed", { host: "win", n, stage: "spawn", kind: "unreachable", exit: 255, stderr: `ssh: connect to host win-build.lan port 22: ${why}`, error: { code: "unreachable", text: `could not reach the host (${why})` } }, "warn", t + 10_032);
    log.emit("link.attempt_end", { link: link("win"), n, outcome: "backoff", error: { code: "unreachable", text: `could not reach the host (${why})` }, ms: 10_034 }, "info", t + 10_034);
    log.emit("link.state", { link: link("win"), from: "connecting", to: "reconnecting", attempt: n, error: `${why} (ssh exited with code 255)` }, "info", t + 10_036);
  }
  // gpu: asks for a password; the sign-in was cancelled.
  const g = now - 40 * MIN;
  log.emit("link.attempt_begin", { link: link("gpu"), n: 1 }, "info", g);
  log.emit("transport.spawned", { link: link("gpu"), n: 1, program: "ssh", argc: 9, pid: 22_001 }, "info", g + 4);
  log.emit("boot.attempt", { host: "gpu", n: 1, shell: "posix", first_contact: true, interactive: true, armed: true }, "info", g + 7);
  log.emit("auth.prompt", { host: "gpu", prompt: 1, kind: "secret", attempt: 1 }, "info", g + 900);
  log.emit("auth.cancelled", { host: "gpu", prompt: 1, why: "user" }, "info", g + 6200);
  log.emit("boot.failed", { host: "gpu", n: 1, stage: "spawn", kind: "auth_needed", exit: 255, stderr: "ml@10.0.4.21: Permission denied (publickey,password).", error: { code: "auth_needed", text: "the host refused the sign-in" } }, "warn", g + 6300);
  log.emit("link.attempt_end", { link: link("gpu"), n: 1, outcome: "park", error: { code: "auth_needed", text: "the host refused the sign-in" }, ms: 6302 }, "info", g + 6302);
  log.emit("link.state", { link: link("gpu"), from: "connecting", to: "auth_needed", error: "auth_needed: This host asks for a password and a one-time code." }, "info", g + 6304);
  // pi: its helper is out of date; the update is under way.
  const p = now - 90_000;
  log.emit("link.attempt_begin", { link: link("pi"), n: 1 }, "info", p);
  log.emit("transport.spawned", { link: link("pi"), n: 1, program: "ssh", argc: 9, pid: 23_001 }, "info", p + 4);
  log.emit("boot.attempt", { host: "pi", n: 1, shell: "posix", first_contact: false, interactive: false, armed: false }, "info", p + 6);
  log.emit("boot.report", { host: "pi", n: 1, os: "linux", arch: "aarch64", installed: "0.7.1", ms: 640 }, "info", p + 650);
  log.emit("boot.build_mismatch", { host: "pi", expected: "0.7.3", found: "0.7.1" }, "warn", p + 652);
  log.emit("boot.action", { host: "pi", n: 1, action: "upload", bytes: 4_300_000, target: "0.7.3" }, "info", p + 655);
  log.emit("link.state", { link: link("pi"), from: "connecting", to: "provisioning", error: "build mismatch: expected 0.7.3, found 0.7.1" }, "info", p + 660);
  return { hz: 1, win: 3, gpu: 1, pi: 1 };
}

/** A host daemon's own log: its start, environment capture, adopting its sessions, and what went wrong since. */
export function seedHost(log: FixtureLog, host: string, now: number, sessions: string[]) {
  const t0 = now - 3 * 60 * MIN;
  log.emit("process.start", { role: "daemon", build: "0.7.3", target: "x86_64-unknown-linux-gnu", os: "linux", arch: "x86_64", pid: 31_000 }, "info", t0 - 20_000);
  log.emit("daemon.start", { hub: false, host_id: host, build: "0.7.3", pid: 31_000, machine: `m-${host}`, data: "~/.local/share/marsad", run_dir: "~/.local/share/marsad/run", socket: "~/.local/share/marsad/run/daemon.sock", free_text_logging: false }, "info", t0 - 19_980);
  log.emit("env.capture", { source: "login_interactive_shell", ms: 142 }, "info", t0 - 19_800);
  log.emit("daemon.ready", { total_ms: 231, env_ms: 142, adopt_ms: 40, node_ms: 8, line_written: true }, "info", t0 - 19_700);
  if (sessions[0]) {
    log.emit("session.retry", { session: sessions[0], attempt: 1, error: foreignError("shim_unreachable", "connecting to the session's supervisor: connection refused") }, "debug", now - 70 * MIN);
    log.emit("session.lost", { session: sessions[0], error: foreignError("shim_unreachable", "the session's supervisor is gone") }, "warn", now - 69 * MIN);
  }
  log.emit("session.command_refused", { session: sessions[1] ?? sessions[0] ?? "none", kind: "prompt", code: "busy" }, "warn", now - 25 * MIN);
}
