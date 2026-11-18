import type { Backend, ConnectionState, RequestOptions, SubscribeOptions, Unsubscribe } from "../backend";
import type {
  AgentEvent, Command, CommandReply, CommandRequest, DriverCapabilities, HistoryPage, HistoryRequest, HostEvent, HostId, HostInfo,
  HostLink, RpcError, SessionEvent, SessionEventKind, SessionList, SessionRef, SessionSnapshot,
} from "../types";
import { topics, methods } from "../types";
import { FIXTURE_HOSTS, hostInfo, type FixtureHost } from "./catalog";
import { buildSession, SPECS, type Spec } from "./sessions";

export interface FixtureOptions {
  /** Background chatter (timers): live tool calls, reconnect countdown, a session finishing. Default true. */
  live?: boolean;
  /** Milliseconds of simulated round-trip per request. Default 140. */
  latencyMs?: number;
  /** Multiplier for how long the scripted agent takes to react to a command. 0 = immediately. Default 1. */
  speed?: number;
  now?: () => number;
  specs?: Spec[];
}

interface Entry {
  snap: SessionSnapshot;
  events: SessionEvent[];
  subs: Set<(e: unknown) => void>;
  host: HostId;
}

const err = (code: string, message: string, data?: unknown): RpcError => ({ code, message, data });

/**
 * In-memory scripted backend for browser dev, tests and Playwright. Speaks the same methods and
 * topics as the runtime (wire.rs); state changes plausibly when commands arrive.
 */
export class FixtureBackend implements Backend {
  readonly opts: Required<Omit<FixtureOptions, "specs">>;
  private hosts = new Map<HostId, { def: FixtureHost; link: HostLink; sessions: Map<string, Entry>; subs: Set<(e: unknown) => void> }>();
  private conn: ConnectionState;
  private connSubs = new Set<(s: ConnectionState) => void>();
  private timers = new Set<ReturnType<typeof setTimeout>>();
  private interval: ReturnType<typeof setInterval> | undefined;
  private tickN = 0;
  private reqLog: { method: string; params: unknown; host?: HostId }[] = [];

  constructor(o: FixtureOptions = {}) {
    this.opts = { live: o.live ?? true, latencyMs: o.latencyMs ?? 140, speed: o.speed ?? 1, now: o.now ?? Date.now };
    const now = this.opts.now();
    for (const def of FIXTURE_HOSTS) {
      this.hosts.set(def.id, {
        def,
        link: { id: def.id, name: def.name, where: def.where, os: def.os, link: def.link, retry_in_ms: def.retry_in_ms },
        sessions: new Map(),
        subs: new Set(),
      });
    }
    for (const spec of o.specs ?? SPECS) {
      const { snap, events } = buildSession(spec, now);
      this.hosts.get(spec.host)!.sessions.set(snap.id, { snap, events, subs: new Set(), host: spec.host });
    }
    this.conn = { hosts: [...this.hosts.values()].map((h) => h.link) };
    if (this.opts.live) this.interval = setInterval(() => this.tick(), 1000);
  }

  /** Requests received so far (tests). */
  get requests() {
    return this.reqLog;
  }

  dispose() {
    if (this.interval) clearInterval(this.interval);
    this.timers.forEach(clearTimeout);
    this.timers.clear();
  }

  // ── Backend ───────────────────────────────────────────────────────────────

  connection = {
    get: () => this.conn,
    subscribe: (cb: (s: ConnectionState) => void): Unsubscribe => {
      this.connSubs.add(cb);
      return () => this.connSubs.delete(cb);
    },
  };

  request<T>(method: string, params?: unknown, opts: RequestOptions = {}): Promise<T> {
    const hostId = opts.host ?? FIXTURE_HOSTS[0].id;
    this.reqLog.push({ method, params, host: hostId });
    return new Promise<T>((resolve, reject) => {
      this.later(this.opts.latencyMs, () => {
        const host = this.hosts.get(hostId);
        if (!host) return reject(err("unknown_host", `No host ${hostId}`));
        // host.info is served from the daemon's cache, so drivers are known even while a link is down.
        if (host.link.link !== "connected" && method !== methods.HOST_INFO) {
          return reject(err("host_unreachable", `${host.def.name} is ${host.link.link === "offline" ? "offline" : "reconnecting"}; try again once it is connected.`));
        }
        try {
          resolve(this.handle(host, method, params) as T);
        } catch (e) {
          reject(e);
        }
      });
    });
  }

  subscribe(topic: string, opts: SubscribeOptions, onEvent: (e: unknown) => void): Unsubscribe {
    const host = this.hosts.get(opts.host ?? FIXTURE_HOSTS[0].id);
    if (!host) return () => undefined;
    if (topic === topics.HOST_SESSIONS) {
      host.subs.add(onEvent);
      this.later(0, () => onEvent({ kind: "reset", sessions: [...host.sessions.values()].map((e) => e.snap) } satisfies HostEvent));
      return () => host.subs.delete(onEvent);
    }
    const m = /^session\.events:(.+)$/.exec(topic);
    const entry = m ? host.sessions.get(m[1]) : undefined;
    if (!entry) return () => undefined;
    const from = Number(opts.from ?? Number.POSITIVE_INFINITY);
    entry.subs.add(onEvent);
    if (Number.isFinite(from)) this.later(0, () => entry.events.filter((e) => e.cursor > from).forEach((e) => onEvent(e)));
    return () => entry.subs.delete(onEvent);
  }

  // ── request handlers ──────────────────────────────────────────────────────

  private handle(host: { def: FixtureHost; sessions: Map<string, Entry> }, method: string, params: unknown): unknown {
    switch (method) {
      case methods.HOST_INFO:
        return hostInfo(host.def, this.opts.now()) satisfies HostInfo;
      case methods.SESSIONS_LIST:
        return { sessions: [...host.sessions.values()].map((e) => e.snap) } satisfies SessionList;
      case methods.SESSION_SNAPSHOT:
        return this.entry(host, params as SessionRef).snap;
      case methods.SESSION_HISTORY: {
        const r = params as HistoryRequest;
        const all = this.entry(host, r).events.filter((e) => r.before == null || e.cursor < r.before);
        const events = all.slice(Math.max(0, all.length - r.limit));
        return { events, has_more: all.length > events.length } satisfies HistoryPage;
      }
      case methods.SESSION_COMMAND: {
        const r = params as CommandRequest;
        return this.command(this.entry(host, r), r.command);
      }
      case methods.SESSIONS_CONTINUE: {
        const e = this.entry(host, params as SessionRef);
        this.setPhase(e, { state: "idle" });
        return {};
      }
      case methods.SESSIONS_STOP: {
        const e = this.entry(host, params as SessionRef);
        this.setPhase(e, { state: "exited", code: 0 });
        return {};
      }
      case methods.HANDOFF_DECIDE: {
        const p = params as { request_id: string; choice: string };
        for (const h of this.hosts.values()) for (const e of h.sessions.values()) {
          const g = e.snap.pending_gates.find((x) => x.gate.request_id === p.request_id);
          if (g) {
            this.resolveGate(e, p.request_id, `Handoff ${p.choice === "deny" ? "denied" : "allowed"}`);
            return {};
          }
        }
        throw err("rejected", "That handoff is no longer pending.");
      }
      default:
        throw err("method_not_found", `Fixture does not implement ${method}`);
    }
  }

  private entry(host: { sessions: Map<string, Entry> }, r: SessionRef): Entry {
    const e = host.sessions.get(r.id);
    if (!e) throw err("session_not_found", `No such session: ${r.id}`);
    return e;
  }

  private command(e: Entry, c: Command): CommandReply {
    const live = ["starting", "idle", "working", "needs_you"].includes(e.snap.phase.state);
    if (!live && c.command !== "prompt") throw err("not_running", "The session is not running.", { error: "not_running", reason: e.snap.phase.state });
    this.emit(e, { kind: "input", command: c, status: { status: "sent" } });
    switch (c.command) {
      case "prompt":
        this.startTurn(e, c.text);
        return { outcome: e.snap.phase.state === "working" ? "queued" : "sent" };
      case "run_slash":
        this.startTurn(e, `/${c.name} ${c.args}`.trim());
        return { outcome: "sent" };
      case "answer_gate":
      case "answer_question": {
        const g = e.snap.pending_gates.find((x) => x.gate.request_id === c.request_id);
        if (!g) throw err("rejected", `Gate ${c.request_id} is not pending.`, { error: "rejected", reason: "gate not pending" });
        this.resolveGate(e, c.request_id, c.command === "answer_gate" && c.decision.action === "deny" ? "Understood, I will not do that." : "Thanks, continuing.");
        return { outcome: "sent" };
      }
      case "interrupt":
        e.snap = { ...e.snap, pending_gates: [], turn: undefined };
        this.setPhase(e, { state: "idle" });
        return { outcome: "sent" };
      case "set_model":
        this.patch(e, { model: c.model });
        return { outcome: "sent" };
      case "set_mode":
        this.patch(e, { mode: c.mode });
        return { outcome: "sent" };
      case "shutdown":
        this.setPhase(e, { state: "exited", code: 0 });
        return { outcome: "sent" };
    }
  }

  // ── scripted agent behaviour ──────────────────────────────────────────────

  private startTurn(e: Entry, prompt: string) {
    const now = this.opts.now();
    e.snap = { ...e.snap, turn: { started_at_ms: now, prompt, tool_calls: 0 }, last_activity_ms: now };
    this.setPhase(e, { state: "working" });
    this.emit(e, { kind: "agent", event: { type: "turn_started" } });
    this.after(450, () => this.emit(e, { kind: "agent", event: { type: "assistant_delta", text: `On it: ${prompt.slice(0, 80)}`, message_id: `r${now}` } }));
    this.after(900, () => this.fakeTool(e, "Read", { file_path: "README.md" }, true, "# Project\n"));
    this.after(1700, () => this.finishTurn(e, `Done: ${prompt.slice(0, 80)}. Nothing else needs your attention.`));
  }

  private fakeTool(e: Entry, name: string, input: unknown, ok: boolean, result: unknown) {
    const id = `t${this.opts.now()}${Math.floor(Math.random() * 1e4)}`;
    this.emit(e, { kind: "agent", event: { type: "tool_call_started", name, call_id: id, input, is_subagent: false } });
    this.emit(e, { kind: "agent", event: { type: "tool_call_finished", name, call_id: id, ok, result } });
    e.snap = { ...e.snap, turn: e.snap.turn ? { ...e.snap.turn, tool_calls: e.snap.turn.tool_calls + 1, last_tool: name } : e.snap.turn };
    this.publish(e);
  }

  private finishTurn(e: Entry, reply: string) {
    const now = this.opts.now();
    const usage = { ...e.snap.usage, cost_usd: +(e.snap.usage.cost_usd + 0.06).toFixed(2) };
    this.emit(e, { kind: "agent", event: { type: "assistant_delta", text: reply, message_id: `d${now}` } });
    this.emit(e, { kind: "agent", event: { type: "turn_completed", was_interrupted: false, num_turns: 1, usage, reply } });
    e.snap = {
      ...e.snap, usage, turn: undefined, turns_completed: e.snap.turns_completed + 1,
      last_turn: { ended_at_ms: now, was_interrupted: false, num_turns: 1, reply }, last_activity_ms: now,
    };
    this.setPhase(e, { state: "idle" });
  }

  private resolveGate(e: Entry, requestId: string, reply: string) {
    const gates = e.snap.pending_gates.filter((g) => g.gate.request_id !== requestId);
    this.emit(e, { kind: "agent", event: { type: "gate_resolved", request_id: requestId } });
    e.snap = { ...e.snap, pending_gates: gates, last_activity_ms: this.opts.now() };
    if (gates.length) return this.publish(e);
    this.setPhase(e, { state: "working" });
    this.after(1200, () => this.finishTurn(e, reply));
  }

  // ── state plumbing ────────────────────────────────────────────────────────

  private patch(e: Entry, p: Partial<SessionSnapshot>) {
    e.snap = { ...e.snap, ...p, cursor: e.events[e.events.length - 1]?.cursor ?? e.snap.cursor };
    this.publish(e);
  }

  private setPhase(e: Entry, phase: SessionSnapshot["phase"]) {
    this.emit(e, { kind: "phase", phase });
    this.patch(e, { phase, last_activity_ms: this.opts.now() });
  }

  private emit(e: Entry, kind: SessionEventKind) {
    const last = e.events[e.events.length - 1]?.cursor ?? 0;
    const ev = { cursor: (Math.floor(last / 65536) + 1) * 65536, t: this.opts.now(), ...kind } as SessionEvent;
    e.events.push(ev);
    e.snap = { ...e.snap, cursor: ev.cursor };
    if (kind.kind === "agent" && kind.event.type === "gate_requested") {
      e.snap = { ...e.snap, pending_gates: [...e.snap.pending_gates, { gate: kind.event.gate, raised_at_ms: ev.t, cursor: ev.cursor }] };
    }
    e.subs.forEach((s) => s(ev));
  }

  private publish(e: Entry) {
    const host = this.hosts.get(e.host)!;
    host.subs.forEach((s) => s({ kind: "upsert", session: e.snap } satisfies HostEvent));
  }

  private after(ms: number, fn: () => void) {
    this.later(ms * this.opts.speed, fn);
  }

  private later(ms: number, fn: () => void) {
    const h = setTimeout(() => {
      this.timers.delete(h);
      fn();
    }, ms);
    this.timers.add(h);
  }

  // ── background chatter (live mode) ────────────────────────────────────────

  private setLink(id: HostId, link: HostLink["link"], retry?: number) {
    const h = this.hosts.get(id)!;
    h.link = { ...h.link, link, retry_in_ms: retry };
    this.conn = { hosts: [...this.hosts.values()].map((x) => x.link) };
    this.connSubs.forEach((s) => s(this.conn));
  }

  /** Force a link state (tests, demos). */
  setHostLink(id: HostId, link: HostLink["link"], retry?: number) {
    this.setLink(id, link, retry);
  }

  private tick() {
    const n = ++this.tickN;
    const win = this.hosts.get("win")!;
    if (win.link.link === "reconnecting") {
      const left = (win.link.retry_in_ms ?? 4000) - 1000;
      if (n > 15 && left <= 0) this.setLink("win", "connected");
      else this.setLink("win", "reconnecting", left > 0 ? left : 4000);
    }
    if (n === 70) this.setLink("hz", "reconnecting", 5000);
    if (n === 75) this.setLink("hz", "connected");
    const x1 = this.hosts.get("x1")!;
    for (const e of x1.sessions.values()) {
      if (e.snap.phase.state !== "working" || n % 4 !== 0) continue;
      const tools: [string, unknown][] = [["Edit", { file_path: "src-tauri/capabilities/default.json" }], ["Bash", { command: "cargo check" }], ["Read", { file_path: "Cargo.toml" }]];
      const [name, input] = tools[(n / 4) % tools.length | 0];
      this.fakeTool(e, name, input, true, "ok");
      e.snap = { ...e.snap, live_usage: { ...e.snap.usage, cost_usd: +(e.snap.usage.cost_usd + 0.01).toFixed(2) }, usage: { ...e.snap.usage, cost_usd: +(e.snap.usage.cost_usd + 0.01).toFixed(2) } };
      this.publish(e);
      if (n === 48) this.finishTurn(e, "Tauri 2.5 upgrade is in: capabilities migrated, cargo check and the UI tests pass.");
    }
  }

  /** Fixture-only: the "seen" state a returning user would have (idle sessions were seen). */
  initialSeen(): { seen: Record<string, number>; lookedAt: number } {
    const now = this.opts.now();
    const seen: Record<string, number> = {};
    for (const [hid, h] of this.hosts) for (const [id, e] of h.sessions) if (e.snap.phase.state === "idle" && e.snap.last_activity_ms < now - 60 * 60_000) seen[`${hid}/${id}`] = now;
    return { seen, lookedAt: now - 45 * 60_000 };
  }

  // ── test helpers ──────────────────────────────────────────────────────────

  /** Add a pending gate to a session, as if the agent just asked. */
  raiseGate(hostId: HostId, sessionId: string, gate: import("../types").GateRequest) {
    const e = this.hosts.get(hostId)!.sessions.get(sessionId)!;
    this.emit(e, { kind: "agent", event: { type: "gate_requested", gate } satisfies AgentEvent });
    this.setPhase(e, { state: "needs_you" });
  }

  sessionIds(hostId: HostId): string[] {
    return [...this.hosts.get(hostId)!.sessions.keys()];
  }

  capabilities(hostId: HostId, id: string): DriverCapabilities {
    return this.hosts.get(hostId)!.sessions.get(id)!.snap.capabilities;
  }
}
