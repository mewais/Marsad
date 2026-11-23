import type { Backend, HubLinkInfo, RequestOptions, SubscribeOptions, SubscriptionEnd, Unsubscribe } from "../backend";
import type { ExportSaved } from "../api";
import type {
  AttachmentPut,
  FileReadRequest,
  FilesStatRequest,
  AuthAnswer, AuthCancel, AuthPrompt, BaseRequest, HostEntry, FleetEvent, FleetHost, FsListRequest, FsPathRequest, GateReason, GateRequest, GitCommitRequest, GitDiffRequest,
  GitDiscardRequest, GitRefresh, GitStatus, GitStatusRequest, GitSummary, GitTarget, HistoryRequest, RenameSession, HostId, InventoryRequest, LinkState, SessionRef, SignInRepeat, SessionSnapshot, CommandRequest,
  HostEnvironment, HostEnvSettings, HostRef, UiEvent, ExportRequest, DiagTailParams, TrailItem, TrailRequest, LaunchRequest, LaunchReply,
  PtyInputRequest, PtyRef, PtyResizeRequest, PtyStartRequest, PtyEvent,
} from "../wire";
import { methods as M, topics as T } from "../wire";
import pkg from "../../../package.json";
import { DRIVERS, FIXTURE_HOSTS, driverDef, inventoryFor, type FixtureHost } from "./catalog";
import { advanceEarlier, failEarlier, historyRounds, journalEarlier, type HistoryItem } from "./earlier";
import { FixtureExports } from "./exports";
import { FixtureLog, scriptedTrail, seedHost, seedHub, trailOf } from "./diagnostics";
import { fsExists, fsHome, fsList } from "./fs";
import { buildTree, diffOf, type Tree } from "./git";
import { FixtureHub } from "./hub";
import { FixturePtys } from "./pty";
import { FixtureHostEnvs } from "./hostenv";
import { err, SessionSim, type Entry } from "./runtime";
import { FixtureFiles, MEDIA_SPEC } from "./media";
import { buildSession, fixtureId, SPAWN_CHOICES, SPECS, type Spec } from "./sessions";
import { SPAWN_REQUEST } from "./workstreams";
import { historyAdmits } from "./work";

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

interface HostRec {
  def: FixtureHost;
  prompt: AuthPrompt | null;
  sessions: Map<string, Entry>;
  /** Why the link was last down, in the hub's words (kept while the hub waits for the user, like the daemon). */
  cause?: string;
  /** The next successful sign-in is followed by a second one, for this reason (see `repeatNext`). */
  repeat?: SignInRepeat;
}

/** The hub's words for a link it parks until the user asks (`SupervisorHooks::hold`). */
const HELD_DETAIL = "this host signs in interactively: connect to sign in";

const connected = (id: string): LinkState => ({ state: "connected", hello: { protocol: { major: 1, minor: 0 }, build: "fixture", host_id: id, role: "daemon", features: [] } });
const nextAt = (ms: number) => ({ secs_since_epoch: Math.floor(ms / 1000), nanos_since_epoch: (ms % 1000) * 1e6 });

/**
 * In-memory scripted backend for browser dev, tests and Playwright. Speaks the same methods,
 * topics and payloads as the hub and host daemons (marsad-proto, wasit-runtime); state changes
 * plausibly when requests arrive.
 */
export class FixtureBackend implements Backend {
  readonly opts: Required<Omit<FixtureOptions, "specs">>;
  readonly hub: FixtureHub;
  private hosts = new Map<HostId, HostRec>();
  /** Blobs, uploads and session files (media previews). */
  readonly files = new FixtureFiles();
  private fleetSubs = new Set<(e: unknown) => void>();
  private wsSubs = new Set<(e: unknown) => void>();
  private trees = new Map<string, Tree>();
  private timers = new Set<ReturnType<typeof setTimeout>>();
  private interval: ReturnType<typeof setInterval> | undefined;
  private tickN = 0;
  private promptN = 1;
  private reqLog: { method: string; params: unknown; host?: HostId; trace?: string }[] = [];
  /** Every diagnostics event the UI recorded (`diagLog`), in order (tests). */
  readonly diagEvents: UiEvent[] = [];
  /** What `desktop.export_save` answers (tests set `{ saved: false }` for a cancelled dialog). */
  exportSaveAnswer: (id: string) => ExportSaved = (id) => ({ path: `/home/me/Downloads/${id}` });
  /** The hub's diagnostics exports (`diagnostics.export.start`, topic `diagnostics.exports`). */
  readonly exports: FixtureExports;
  /** How far each host's wall clock is ahead of this computer's, in ms (what `diag.sources` reveals to a reader that measures it). */
  readonly hostClockSkew: Record<HostId, number> = {};
  readonly sim: SessionSim;
  readonly envs: FixtureHostEnvs;
  /** The hosts' terminals (`pty.*`): one per session, scripted shells. */
  readonly ptys = new FixturePtys({ deliver: (fn) => this.deliver(fn) });
  private invSubs = new Map<HostId, Set<(e: unknown) => void>>();
  private navigators = new Set<(t: { host: HostId; session?: string }) => void>();
  /** The app's link to its local daemon (what the desktop shell reports); starts connected. */
  private hubLink: HubLinkInfo = { state: "connected" };
  private hubWatchers = new Set<(l: HubLinkInfo) => void>();
  /** Every open subscription with its end callback, so a test can end one the way the shell does. */
  private openSubs = new Set<{ topic: string; host?: HostId; params?: unknown; onEnd?: (e: SubscriptionEnd) => void; off: () => void }>();
  /** How many times the UI asked to reconnect to the local daemon (tests). */
  hubReconnects = 0;
  /** What reaches the app once its link to the daemon is back (subscription deliveries wait for it). */
  private parked: (() => void)[] = [];
  /** The hub's diagnostics log; also the local host's daemon's, which is the hub. */
  readonly hubLog: FixtureLog;
  private hostLogs = new Map<HostId, FixtureLog>();
  /** Whether a `diag.tail` filter reaches the daemon; false emulates a transport that drops it (the app must then check it itself). */
  tailFilters = true;
  /** Scripted trail history of a session (by key) and how many of its events it stands in for. */
  private trailBase = new Map<string, { base: TrailItem[]; from: number }>();
  /** The last connect attempt of each host's link, for the hub's connection records. */
  private attempts = new Map<HostId, number>();
  private attemptStart = new Map<HostId, number>();
  /** The trace of the request being served (the fixture serves one at a time, synchronously). */
  private requestTrace: string | undefined;

  constructor(o: FixtureOptions = {}) {
    this.opts = { live: o.live ?? true, latencyMs: o.latencyMs ?? 140, speed: o.speed ?? 1, now: o.now ?? Date.now };
    const now = this.opts.now();
    this.envs = new FixtureHostEnvs(this.opts.now);
    this.exports = new FixtureExports({
      now: this.opts.now,
      hosts: () => [...this.hosts.values()].map((h) => ({ id: h.def.entry.id, up: this.isUp(h), link: h.def.link })),
      after: (ms, fn) => this.later(ms * this.opts.speed, fn),
    });
    this.sim = new SessionSim({
      now: this.opts.now,
      after: (ms, fn) => this.later(ms * this.opts.speed, fn),
      publish: (e) => this.publish(e),
      gateEnded: (e, gate, reason) => this.hub.ws.onGateEnded(e.host, e.snap.id, gate, reason),
    });
    for (const def of FIXTURE_HOSTS) {
      const d = structuredClone(def);
      if (d.link?.state === "reconnecting") d.link.next_at = nextAt(now + 4000);
      this.hosts.set(d.entry.id, { def: d, prompt: null, sessions: new Map() });
    }
    for (const spec of o.specs ?? SPECS) {
      const { snap, events, earlier, earlierPlan } = buildSession(spec, now);
      const entry: Entry = { snap, events, subs: new Set(), host: spec.host, ...(snap.earlier.state.kind !== "none" ? { earlier, earlierPlan } : {}) };
      this.hosts.get(spec.host)!.sessions.set(snap.id, entry);
      // A live fixture goes on importing the history that is still arriving, a record at a time.
      if (earlierPlan && this.opts.live) this.arrive(entry);
      if (spec.n === MEDIA_SPEC.n) this.files.seed(snap.id, snap.cwd ?? "");
      this.trees.set(`${spec.host}/${snap.id}`, buildTree(spec));
      const script = scriptedTrail(spec.n, events);
      if (script) this.trailBase.set(`${spec.host}/${snap.id}`, { base: script, from: events.length });
    }
    this.hubLog = new FixtureLog("hub", "hubb0001", this.opts.now, now - 3 * 3_600_000);
    for (const [h, n] of Object.entries(seedHub(this.hubLog, now))) this.attempts.set(h, n);
    this.hub = new FixtureHub({
      now: this.opts.now,
      hostDefs: () => [...this.hosts.values()].map((h) => h.def),
      addHost: (def) => this.addHost(def),
      removeHost: (id) => this.removeHost(id),
      updateHost: (e) => this.updateHost(e),
      reconnect: (id) => this.reconnect(id),
      answerAuth: (a) => this.answerAuth(a),
      cancelAuth: (a) => this.cancelAuth(a),
      createSession: (host, snap, first) => this.createSession(host, snap, first),
      session: (host, id) => this.hosts.get(host)?.sessions.get(id)?.snap,
      allSessions: () => [...this.hosts].flatMap(([host, h]) => [...h.sessions.values()].map((e) => ({ host, snap: e.snap }))),
      agentCommand: (host, driver) => this.envs.command(host, driver),
      emitWorkstreams: (e) => this.wsSubs.forEach((s) => s(e)),
    });
    // A returning user has seen the old idle sessions.
    for (const [hid, h] of this.hosts) for (const [id, e] of h.sessions) if (e.snap.phase.state === "idle" && e.snap.last_activity_ms < now - 60 * 60_000) this.hub.seen.push({ host: hid, session: id, cursor: e.snap.cursor, at_ms: now - 45 * 60_000 });
    if (this.opts.live) this.interval = setInterval(() => this.tick(), 1000);
  }

  /** Requests received so far (tests). */
  get requests() {
    return this.reqLog;
  }

  /** The subscriptions open now: what the UI is holding open on which daemon (tests). */
  get subscriptions(): { topic: string; host?: HostId; params?: unknown }[] {
    return [...this.openSubs].map(({ topic, host, params }) => ({ topic, host, params }));
  }

  dispose() {
    if (this.interval) clearInterval(this.interval);
    this.timers.forEach(clearTimeout);
    this.timers.clear();
  }

  // ── Backend ───────────────────────────────────────────────────────────────

  request<T>(method: string, params?: unknown, opts: RequestOptions = {}): Promise<T> {
    this.reqLog.push({ method, params, host: opts.host, ...(opts.trace ? { trace: opts.trace } : {}) });
    return new Promise<T>((resolve, reject) => {
      this.later(this.opts.latencyMs, () => {
        // The shell answers `desktop.*` itself; everything else needs the local daemon.
        if (this.hubLink.state !== "connected" && !method.startsWith("desktop.")) return reject(err("not_connected", "not connected to the local daemon"));
        this.requestTrace = opts.trace;
        try {
          const result = opts.host ? this.hostRequest(opts.host, method, params) : this.hubRequest(method, params);
          this.requestTrace = undefined;
          if (opts.trace) this.recordTraced(opts.trace, method, params, opts.host, result);
          resolve(result as T);
        } catch (e) {
          this.requestTrace = undefined;
          if (opts.trace) this.recordTraced(opts.trace, method, params, opts.host, undefined, e);
          reject(e);
        }
      });
    });
  }

  /** What the daemons record of a request that carries a trace: the hub's and the host's `rpc.traced`, a refusal, a launch. */
  private recordTraced(trace: string, method: string, params: unknown, host: HostId | undefined, result?: unknown, e?: unknown) {
    const ms = Math.max(1, this.opts.latencyMs);
    const code = e ? String((e as { code?: unknown }).code ?? "error") : undefined;
    const outcome = e ? "error" : "ok";
    const rec = this.hosts.get(host ?? "");
    const remote = !!rec?.def.link;
    this.hubLog.emit("rpc.traced", { trace, method, ...(remote ? { host } : {}), hops: remote ? 1 : 0, ms, outcome, ...(code ? { code } : {}) });
    if (rec && host) {
      const up = this.isUp(rec);
      if (remote && !up) this.hubLog.emit("rpc.forward_failed", { method, host, code: code ?? "error", ms }, "warn");
      if (up) {
        const log = this.hostLog(host)!;
        if (remote) log.emit("rpc.traced", { trace, method, hops: 0, ms: Math.max(1, Math.round(ms / 2)), outcome, ...(code ? { code } : {}) });
        if (e && method === M.SESSION_COMMAND) log.emit("session.command_refused", { session: (params as CommandRequest).id, trace, kind: (params as CommandRequest).command.command, code }, "warn");
      }
    }
    if (method === M.SESSIONS_LAUNCH) {
      const r = params as LaunchRequest;
      const reply = result as LaunchReply | undefined;
      this.hubLog.emit("launch", { trace, host: r.host ?? "x1", ...(reply ? { session: reply.id } : {}), driver: r.driver, resume: r.resume.kind === "new" ? "fresh" : r.resume.kind === "latest" ? "continue" : "resume", first_prompt: r.first_prompt != null, ms, outcome, ...(code ? { error: { code } } : {}) });
    }
  }

  /** A host's own diagnostics log (the local host's is the hub's); made when first asked for. */
  private hostLog(id: HostId): FixtureLog | undefined {
    const h = this.hosts.get(id);
    if (!h) return undefined;
    if (!h.def.link) return this.hubLog;
    let log = this.hostLogs.get(id);
    if (!log) {
      const now = this.opts.now();
      log = new FixtureLog("daemon", `host${id}01`, this.opts.now, now - 3 * 3_600_000);
      seedHost(log, id, now, [...h.sessions.keys()]);
      this.hostLogs.set(id, log);
    }
    return log;
  }

  /** The trail of a session: its scripted history, then the projection of its events. */
  private trail(host: HostRec, id: string): TrailItem[] {
    const e = host.sessions.get(id)!;
    const t = this.trailBase.get(`${host.def.entry.id}/${id}`);
    return trailOf(e.events, t?.base, t?.from);
  }

  subscribe(topic: string, opts: SubscribeOptions, onEvent: (e: unknown) => void, onEnd?: (end: SubscriptionEnd) => void): Unsubscribe {
    const rec: { topic: string; host?: HostId; params?: unknown; onEnd?: (e: SubscriptionEnd) => void; off: Unsubscribe } = {
      topic,
      host: opts.host,
      params: opts.params,
      onEnd,
      off: () => undefined,
    };
    // A topic that ends by itself (a terminal after its shell exits) is no longer a subscription.
    rec.off = this.attach(topic, opts, onEvent, (end) => {
      if (!this.openSubs.delete(rec)) return;
      onEnd?.(end);
    });
    this.openSubs.add(rec);
    return () => {
      this.openSubs.delete(rec);
      rec.off();
    };
  }

  private attach(topic: string, opts: SubscribeOptions, onEvent: (e: unknown) => void, ended: (end: SubscriptionEnd) => void): Unsubscribe {
    if (topic === T.FLEET) {
      this.fleetSubs.add(onEvent);
      this.deliver(() => this.fleetSubs.has(onEvent) && onEvent({ kind: "reset", hosts: [...this.hosts.values()].map((h) => this.fleetHost(h)) } satisfies FleetEvent));
      return () => this.fleetSubs.delete(onEvent);
    }
    if (topic === T.WORKSTREAMS) {
      this.wsSubs.add(onEvent);
      this.deliver(() => this.wsSubs.has(onEvent) && onEvent(this.hub.ws.reset()));
      return () => this.wsSubs.delete(onEvent);
    }
    if (topic === T.DIAGNOSTICS_EXPORTS) {
      const off = this.exports.attach(onEvent);
      let live = true;
      this.deliver(() => live && onEvent(this.exports.reset()));
      return () => {
        live = false;
        off();
      };
    }
    if (topic === T.DIAG_TAIL) {
      const rec = opts.host ? this.hosts.get(opts.host) : undefined;
      const log = opts.host ? this.hostLog(opts.host) : this.hubLog;
      // A host that is not connected delivers nothing: the subscription waits for its link.
      if (!log || (rec && !this.isUp(rec))) return () => undefined;
      const filter = this.tailFilters ? (opts.params as DiagTailParams | undefined) ?? {} : {};
      const from = opts.from == null ? undefined : String(opts.from);
      let live = true;
      let off: Unsubscribe | undefined;
      this.deliver(() => {
        if (live) off = log.tail(filter, from, onEvent as never);
      });
      return () => {
        live = false;
        off?.();
      };
    }
    const host = this.hosts.get(opts.host ?? "x1");
    if (!host) return () => undefined;
    if (topic === T.DRIVERS_INVENTORY) {
      const id = host.def.entry.id;
      const subs = this.invSubs.get(id) ?? new Set();
      this.invSubs.set(id, subs);
      subs.add(onEvent);
      this.deliver(() => subs.has(onEvent) && this.isUp(host) && onEvent(this.hostInventory(id)));
      return () => subs.delete(onEvent);
    }
    if (topic.startsWith(T.ptyOutput(""))) {
      const id = topic.slice(T.ptyOutput("").length);
      const from = opts.from == null ? undefined : Number(opts.from);
      // Not while the host is unreachable: the subscription waits for its link.
      if (!this.isUp(host)) return () => undefined;
      return this.ptys.subscribe(host.def.entry.id, id, from, (e: PtyEvent) => onEvent(e), ended);
    }
    const trailTopic = /^session\.trail:(.+)$/.exec(topic);
    if (trailTopic) {
      const te = host.sessions.get(trailTopic[1]);
      if (!te) return () => undefined;
      const from = Number(opts.from ?? Number.POSITIVE_INFINITY);
      const send = (item: TrailItem) => onEvent(item);
      (te.trailSubs ??= new Set()).add(send);
      if (Number.isFinite(from)) this.deliver(() => te.trailSubs?.has(send) && this.trail(host, trailTopic[1]).filter((it) => it.cursor > from).forEach(send));
      return () => te.trailSubs?.delete(send);
    }
    const m = /^session\.events:(.+)$/.exec(topic);
    const entry = m ? host.sessions.get(m[1]) : undefined;
    if (!entry) return () => undefined;
    const from = Number(opts.from ?? Number.POSITIVE_INFINITY);
    entry.subs.add(onEvent);
    if (Number.isFinite(from)) this.deliver(() => entry.subs.has(onEvent) && entry.events.filter((e) => e.cursor > from).forEach((e) => onEvent(e)));
    return () => entry.subs.delete(onEvent);
  }

  async diagLog(events: readonly UiEvent[]): Promise<void> {
    this.diagEvents.push(...structuredClone([...events]));
  }

  watchHubLink(cb: (l: HubLinkInfo) => void): Unsubscribe {
    this.hubWatchers.add(cb);
    this.later(0, () => this.hubWatchers.has(cb) && cb(this.hubLink));
    return () => this.hubWatchers.delete(cb);
  }

  async appVersion(): Promise<string> {
    return pkg.version;
  }

  async reconnectHub(): Promise<void> {
    this.hubReconnects++;
    this.setHubLink({ state: "connecting" });
    this.later(300 * this.opts.speed, () => this.setHubLink({ state: "connected" }));
  }

  /** Force the app's link to the local daemon (tests, demos). */
  setHubLink(link: HubLinkInfo) {
    this.hubLink = link;
    for (const w of [...this.hubWatchers]) w(link);
    if (link.state === "connected") for (const fn of this.parked.splice(0)) fn();
  }

  /**
   * End the open subscriptions of `topic` (all of them without one) the way the shell does when
   * the daemon refuses or closes a topic: each gets `end` once and delivers nothing more.
   */
  endSubscriptions(topic?: string, end: SubscriptionEnd = { code: "not_hub", message: "this daemon is not a hub" }) {
    for (const r of [...this.openSubs]) {
      if (topic !== undefined && r.topic !== topic && !r.topic.startsWith(topic)) continue;
      this.openSubs.delete(r);
      r.off();
      r.onEnd?.(end);
    }
  }

  // ── host services (routed by `host`) ──────────────────────────────────────

  private isUp(h: HostRec) {
    return !h.def.link || h.def.link.state === "connected";
  }

  private hostRequest(hostId: HostId, method: string, p: unknown): unknown {
    const host = this.hosts.get(hostId);
    if (!host) throw err("not_found", `No host ${hostId}`, { marsad_error: "not_found", what: `host ${hostId}` });
    if (!this.isUp(host)) throw err("upstream", `${host.def.entry.name} is not connected; try again once it is back.`);
    const now = this.opts.now();
    switch (method) {
      case M.HOST_INFO:
        return { host_id: hostId, build: "fixture", pid: 4242, started_at_ms: now - 86_400_000, data_dir: "~/.local/share/marsad", os: host.def.entry.platform?.os ?? "linux", arch: host.def.entry.platform?.arch ?? "x86_64",
          drivers: DRIVERS.filter((d) => host.def.drivers[d.id]).map(({ id, display_name, capabilities, prompt_input }) => ({ id, display_name, capabilities, prompt_input })) };
      case M.DRIVERS_INVENTORY:
        return { drivers: this.envs.inventory(hostId, inventoryFor(host.def, (p as InventoryRequest | undefined)?.project ?? null, now)) };
      case M.DRIVERS_REFRESH:
        return {};
      case M.HOST_ENVIRONMENT:
      case M.HOST_ENVIRONMENT_REFRESH:
        return this.envs.report(hostId, this.capturedEnv(host, now));
      case M.DIAG_SOURCES:
        return { now_ms: now + (this.hostClockSkew[hostId] ?? 0), boot: `host${hostId}01`, sources: [{ name: "daemon", gens: [] }] };
      case M.SESSIONS_LIST:
        return { sessions: [...host.sessions.values()].map((e) => e.snap) };
      case M.SESSION_SNAPSHOT:
        return this.entry(host, p as SessionRef).snap;
      case M.SESSION_HISTORY: {
        const r = p as HistoryRequest;
        // The daemon's filter: everything, the session's own transcript, or one subagent's; `before` and `has_more` are within the scope.
        const scope = r.scope ?? { kind: "all" };
        // `earlier: true` pages the conversation's earlier history instead of the session's own events; the cursors of neither are comparable.
        const entry = this.entry(host, r);
        const all = (r.earlier ? entry.earlier ?? [] : entry.events).filter((e) => (r.before == null || e.cursor < r.before) && historyAdmits(scope, e));
        const events = all.slice(Math.max(0, all.length - r.limit));
        return { events, has_more: all.length > events.length };
      }
      case M.SESSION_TRAIL_PAGE: {
        const r = p as TrailRequest;
        this.entry(host, r);
        const all = this.trail(host, r.id).filter((it) => r.before == null || it.cursor < r.before);
        const entries = all.slice(Math.max(0, all.length - r.limit));
        return { entries, has_more: all.length > entries.length };
      }
      case M.SESSION_ATTACHMENT_PUT: {
        const r = p as AttachmentPut;
        this.entry(host, r);
        return this.files.put(r);
      }
      case M.SESSION_FILE_READ: {
        const r = p as FileReadRequest;
        return this.files.read(r, this.entry(host, r).snap.cwd ?? null);
      }
      case M.SESSION_FILES_STAT: {
        const r = p as FilesStatRequest;
        return this.files.stat(r.id, this.entry(host, r).snap.cwd ?? null, r.paths);
      }
      case M.SESSION_COMMAND: {
        const r = p as CommandRequest;
        const e = this.entry(host, r);
        // Spawn answers are applied while the gate is still pending (the hub launches, then the agent hears back).
        if (r.command.command === "answer_gate" && e.snap.pending_gates.some((g) => g.gate.request_id === (r.command as { request_id: string }).request_id && g.gate.gate_type === "spawn")) this.hub.ws.onSpawnAnswer(r.command.request_id, r.command.decision.choice);
        const reply = this.sim.command(e, r.command, this.requestTrace);
        if (r.command.command === "answer_gate") this.hub.ws.onHandoffAnswer(r.command.request_id, r.command.decision.choice === "deny");
        return reply;
      }
      case M.SESSIONS_CONTINUE:
        this.sim.continueSession(this.entry(host, p as SessionRef));
        return {};
      case M.SESSIONS_RENAME: {
        const r = p as RenameSession;
        const e = this.entry(host, r);
        e.snap = { ...e.snap, display_name: r.display_name?.trim() || null };
        this.publish(e);
        return {};
      }
      case M.SESSIONS_DELETE: {
        const r = p as SessionRef;
        const e = this.entry(host, r);
        if (e.snap.phase.state === "working" || e.snap.phase.state === "needs_you" || e.snap.phase.state === "starting") this.sim.end(e);
        host.sessions.delete(r.id);
        this.ptys.sessionDeleted(hostId, r.id);
        this.fleet({ kind: "session_removed", host: hostId, id: r.id });
        return {};
      }
      case M.SESSIONS_STOP:
        this.sim.end(this.entry(host, p as SessionRef));
        return {};
      case M.GIT_STATUS:
        return this.tree(hostId, (p as GitStatusRequest).target).status;
      case M.GIT_DIFF: {
        const r = p as GitDiffRequest;
        return diffOf(this.tree(hostId, r.target), r.path, r.side, r.max_bytes ?? undefined);
      }
      case M.GIT_COMMIT: {
        const r = p as GitCommitRequest;
        if (!r.message.trim()) throw err("invalid_params", "A commit needs a message.", { marsad_error: "invalid_request", message: "empty commit message" });
        if (!r.files.length) throw err("invalid_params", "Pick at least one file to commit.");
        const t = this.tree(hostId, r.target);
        t.status = { ...t.status, ahead: t.status.ahead + 1, files: t.status.files.filter((f) => !r.files.includes(f.path)) };
        this.gitChanged(hostId, r.target);
        return { commit: (Math.floor(now / 1000) % 0xfffffff).toString(16).padStart(7, "0") };
      }
      case M.GIT_DISCARD: {
        const r = p as GitDiscardRequest;
        const t = this.tree(hostId, r.target);
        t.status = { ...t.status, files: t.status.files.filter((f) => !r.files.includes(f.path)) };
        this.gitChanged(hostId, r.target);
        return {};
      }
      case M.FS_LIST:
        return fsList(p as FsListRequest);
      case M.FS_HOME:
        return fsHome();
      case M.FS_EXISTS:
        return fsExists((p as FsPathRequest).path);
      case M.AGENTCONFIG_BASE: {
        const r = p as BaseRequest;
        if (!host.def.drivers[r.driver]) throw err("not_found", `${r.driver} is not installed here`);
        return { driver: r.driver, project: r.project ?? null, items: driverDef(r.driver).config, refreshing: false, errors: [], refreshed_at_ms: now - 4 * 60_000 };
      }
      case M.PTY_START: {
        const r = p as PtyStartRequest;
        return this.ptys.start(hostId, r, this.ptyTarget(host, r));
      }
      case M.PTY_LIST:
        return { ptys: this.ptys.list(hostId) };
      case M.PTY_INPUT: {
        const r = p as PtyInputRequest;
        this.ptys.input(hostId, r.id, r.data);
        return {};
      }
      case M.PTY_RESIZE: {
        const r = p as PtyResizeRequest;
        this.ptys.resize(hostId, r.id, r.cols, r.rows);
        return {};
      }
      case M.PTY_CLOSE:
        this.ptys.close(hostId, (p as PtyRef).id);
        return {};
      default:
        throw err("method_not_found", `Fixture does not implement ${method}`);
    }
  }

  /** The directory a terminal is for, and its session: the host's `resolve` (exactly one of `session` and `cwd`). */
  private ptyTarget(host: HostRec, r: PtyStartRequest) {
    const { session, cwd } = r.target;
    if (session != null && cwd == null) {
      const e = host.sessions.get(session);
      if (!e) throw err("not_found", `not found: session ${session}`, { marsad_error: "not_found", what: `session ${session}` });
      if (!e.snap.cwd) throw err("invalid_params", `invalid request: session ${session} has no working directory`, { marsad_error: "invalid_request", message: `session ${session} has no working directory` });
      return { cwd: e.snap.cwd, session };
    }
    if (session == null && cwd != null) return { cwd, session: null };
    throw err("invalid_params", "invalid request: exactly one of `session` and `cwd` must be given", { marsad_error: "invalid_request", message: "exactly one of `session` and `cwd` must be given" });
  }

  private hubRequest(method: string, p: unknown): unknown {
    // The desktop shell's window background: a browser tab can always show alpha (CSS only).
    if (method === "desktop.window_surface") return { transparent: (p as { translucent: boolean }).translucent, effect: null };
    // The desktop shell opens and reveals things itself; a browser has no such shell (see `Backend`).
    if (method === "desktop.open" || method === "desktop.reveal") return null;
    if (method === "desktop.export_save") return this.exportSaveAnswer((p as { id: string }).id);
    if (method === M.DIAGNOSTICS_EXPORT_START) return this.exports.start(p as ExportRequest);
    if (method === M.FLEET_GIT_REFRESH) {
      const r = p as GitRefresh;
      const e = [...(this.hosts.get(r.host)?.sessions.values() ?? [])].find((x) => x.snap.cwd === r.cwd);
      if (e) this.gitChanged(r.host, { session: e.snap.id, cwd: null });
      return {};
    }
    if (method === M.HOSTS_ENVIRONMENT_SECRETS) return this.envs.secretsLocation;
    if (method === M.HOSTS_ENVIRONMENT_GET || method === M.HOSTS_ENVIRONMENT_SET || method === M.HOSTS_ENVIRONMENT_TEST) return this.envRequest(method, p);
    return this.hub.handle(method, p);
  }

  private capturedEnv(host: HostRec, now: number): HostEnvironment {
    return host.def.entry.platform?.os === "windows"
      ? { source: "registry", captured_at_ms: now - 120_000, duration_ms: 310, path: ["C:\\Windows\\system32", "C:\\Users\\me\\AppData\\Roaming\\npm"], notes: [], variables: [], commands: {} }
      : { source: "login_interactive_shell", shell: "/bin/bash", captured_at_ms: now - 120_000, duration_ms: 140, path: ["/home/me/.nvm/versions/node/v22.11.0/bin", "/home/me/.local/bin", "/usr/local/bin", "/usr/bin", "/bin"], notes: [], variables: [], commands: {} };
  }

  private hostInventory(id: HostId) {
    const host = this.hosts.get(id);
    return { drivers: host ? this.envs.inventory(id, inventoryFor(host.def, null, this.opts.now())) : [] };
  }

  private inventoryChanged(id: HostId) {
    const inv = this.hostInventory(id);
    this.deliver(() => this.invSubs.get(id)?.forEach((cb) => cb(inv)));
  }

  /** `hosts.environment.*` (hub): stored here, applied to the host's simulated environment stage. */
  private envRequest(method: string, p: unknown): unknown {
    const id = method === M.HOSTS_ENVIRONMENT_SET ? (p as HostEnvSettings).host : (p as HostRef).id;
    const host = this.hosts.get(id);
    if (!host) throw err("not_found", `No host ${id}`, { marsad_error: "not_found", what: `host ${id}` });
    if (method === M.HOSTS_ENVIRONMENT_GET) return this.envs.get(id);
    if (method === M.HOSTS_ENVIRONMENT_SET) {
      const saved = this.envs.set(p as HostEnvSettings, this.isUp(host));
      if (!saved.not_applied) {
        saved.environment = this.envs.report(id, this.capturedEnv(host, this.opts.now()));
        this.inventoryChanged(id);
      }
      return saved;
    }
    if (!this.isUp(host)) throw err("upstream", `${host.def.entry.name} is not connected; try again once it is back.`);
    const t = this.envs.test(id, this.capturedEnv(host, this.opts.now()), inventoryFor(host.def, null, this.opts.now()));
    this.inventoryChanged(id);
    return t;
  }

  private entry(host: HostRec, r: SessionRef): Entry {
    const e = host.sessions.get(r.id);
    if (!e) throw err("session_not_found", `No such session: ${r.id}`);
    return e;
  }

  private tree(host: HostId, t: GitTarget): Tree {
    const key = t.session ? `${host}/${t.session}` : `${host}:${t.cwd}`;
    let tree = this.trees.get(key);
    if (!tree && t.session) {
      const snap = this.hosts.get(host)?.sessions.get(t.session)?.snap;
      if (!snap) throw err("session_not_found", `No such session: ${t.session}`);
      tree = buildTree({ n: 99, name: "new-session", driver: snap.driver, host, proj: "project", branch: "main", agoMin: 0, cost: 0, kind: "idle", line: "", stat: [0, 0, 0] });
      this.trees.set(key, tree);
    }
    if (!tree) throw err("not_a_repo", `${t.cwd} is not a git repository`, { marsad_error: "not_a_repo", path: t.cwd ?? "" });
    return tree;
  }

  // ── fleet ─────────────────────────────────────────────────────────────────

  private fleetHost(h: HostRec): FleetHost {
    return { host: h.def.entry, link: h.def.link ? { link: "remote", state: h.def.link } : { link: "local" }, sessions_live: this.isUp(h), sessions: [...h.sessions.values()].map((e) => e.snap), auth_prompt: h.prompt, git: this.gitSummaries(h) };
  }

  /** What the hub's background git refresh would publish: one summary per session folder. */
  private gitSummaries(h: HostRec): GitSummary[] {
    const out = new Map<string, GitSummary>();
    for (const e of h.sessions.values()) {
      const cwd = e.snap.cwd;
      const t = cwd ? this.trees.get(`${h.def.entry.id}/${e.snap.id}`) : undefined;
      if (cwd && t && !out.has(cwd)) out.set(cwd, summaryOf(cwd, t.status, this.opts.now()));
    }
    return [...out.values()];
  }

  private gitChanged(host: HostId, t: GitTarget) {
    const snap = t.session ? this.hosts.get(host)?.sessions.get(t.session)?.snap : undefined;
    const cwd = snap?.cwd ?? t.cwd;
    if (!cwd) return;
    try {
      this.fleet({ kind: "git", host, summary: summaryOf(cwd, this.tree(host, t).status, this.opts.now()) });
    } catch {
      // not a repository in the fixture: nothing to publish
    }
  }

  private fleet(e: FleetEvent) {
    // While the app's link to the daemon is down nothing reaches it; the shell's subscription catches up from its cursor afterwards.
    if (this.hubLink.state !== "connected") this.parked.push(() => this.fleetSubs.forEach((s) => s(e)));
    else this.fleetSubs.forEach((s) => s(e));
  }

  /** Deliver a subscription's first events: soon, or once the app is connected to the daemon. */
  private deliver(fn: () => void) {
    this.later(0, () => (this.hubLink.state === "connected" ? fn() : this.parked.push(fn)));
  }

  private publish(e: Entry) {
    this.fleet({ kind: "session_upserted", host: e.host, session: e.snap });
    this.hub?.ws.onSession(e.host, e.snap.id);
  }

  /** Force a link state (tests, demos). */
  setLink(id: HostId, state: LinkState) {
    const h = this.hosts.get(id)!;
    this.recordLink(id, h.def.link, state);
    h.def.link = state;
    this.fleet({ kind: "link", host: id, state });
    // Like the hub: once the host is connected it delivers its session list.
    if (state.state === "connected") this.fleet({ kind: "sessions", host: id, sessions: [...h.sessions.values()].map((e) => e.snap) });
  }

  /** The hub's connection records of a link changing state: an attempt begins, ends with an outcome, and the state itself. */
  private recordLink(id: HostId, from: LinkState | null, to: LinkState) {
    const link = `hosts:${id}`;
    const now = this.opts.now();
    if (to.state === "connecting") {
      const n = (this.attempts.get(id) ?? 0) + 1;
      this.attempts.set(id, n);
      this.attemptStart.set(id, now);
      this.hubLog.emit("link.attempt_begin", { link, n });
    } else if (this.attemptStart.has(id) && (to.state === "connected" || to.state === "reconnecting" || to.state === "auth_needed" || to.state === "failed")) {
      const outcome = to.state === "connected" ? "connected" : to.state === "reconnecting" ? "backoff" : "park";
      const why = to.state === "reconnecting" ? to.last_error : to.state === "auth_needed" ? to.detail : to.state === "failed" ? to.reason : undefined;
      this.hubLog.emit("link.attempt_end", { link, n: this.attempts.get(id) ?? 1, outcome, ms: now - this.attemptStart.get(id)!, ...(why ? { error: { code: to.state, text: why } } : {}) });
      this.attemptStart.delete(id);
    }
    const text = to.state === "reconnecting" ? to.last_error : to.state === "auth_needed" ? to.detail : to.state === "failed" ? to.reason : undefined;
    this.hubLog.emit("link.state", { link, from: from?.state ?? "connecting", to: to.state, ...(to.state === "reconnecting" ? { attempt: to.attempt } : {}), ...(text ? { error: text } : {}) });
  }

  /** Show a sign-in prompt for a host (tests, demos). */
  setAuthPrompt(id: HostId, prompt: Omit<AuthPrompt, "id" | "host" | "expires_at_ms"> | null) {
    const h = this.hosts.get(id)!;
    h.prompt = prompt ? { ...prompt, id: this.promptN++, host: id, expires_at_ms: this.opts.now() + 120_000 } : null;
    this.fleet({ kind: "auth_prompt", host: id, prompt: h.prompt });
  }

  private addHost(def: FixtureHost) {
    this.hosts.set(def.entry.id, { def, prompt: null, sessions: new Map() });
    this.fleet({ kind: "host_upserted", host: def.entry });
    this.fleet({ kind: "link", host: def.entry.id, state: { state: "connecting" } });
    this.later(600 * this.opts.speed, () => this.setLink(def.entry.id, { state: "provisioning", reason: { kind: "command_not_found", detail: "marsad-agent: command not found" } }));
    this.later(2400 * this.opts.speed, () => {
      def.entry = { ...def.entry, platform: { os: "linux", arch: "x86_64", target: "x86_64-unknown-linux-gnu" } };
      this.fleet({ kind: "host_upserted", host: def.entry });
      this.setLink(def.entry.id, connected(def.entry.id));
    });
  }

  private updateHost(entry: HostEntry): HostEntry {
    const h = this.hosts.get(entry.id);
    if (!h) throw err("not_found", `No host ${entry.id}`);
    h.def.entry = { ...entry, created_at_ms: h.def.entry.created_at_ms };
    this.fleet({ kind: "host_upserted", host: h.def.entry });
    return h.def.entry;
  }

  private removeHost(id: HostId) {
    const h = this.hosts.get(id);
    if (!h) throw err("not_found", `No host ${id}`);
    if (!h.def.link) throw err("invalid_params", "The local host cannot be removed.");
    this.hosts.delete(id);
    this.fleet({ kind: "host_removed", id });
  }

  /** `hosts.reconnect`: the user's click arms one attempt; an interactive host's attempt asks the user something. */
  private reconnect(id: HostId) {
    const h = this.hosts.get(id);
    if (!h) throw err("not_found", `No host ${id}`);
    if (!h.def.link) return;
    this.setLink(id, { state: "connecting" });
    if (h.def.entry.interactive_auth) {
      this.later(300 * this.opts.speed, () => this.setAuthPrompt(id, { prompt: this.passwordPrompt(h), kind: "secret", attempt: 1 }));
    } else this.later(800 * this.opts.speed, () => this.setLink(id, connected(id)));
  }

  private passwordPrompt(h: HostRec): string {
    const k = h.def.entry.kind;
    return `${k.kind === "ssh" ? k.target : h.def.entry.id}'s password: `;
  }

  /** The daemon answers a prompt id it does not hold (answered, withdrawn, or from an attempt that ended) with `not_found`. */
  private pending(host: HostId, id: number): HostRec & { prompt: AuthPrompt } {
    const h = this.hosts.get(host);
    if (!h?.prompt || h.prompt.id !== id) throw err("not_found", `not found: prompt ${id} of ${host}`);
    return h as HostRec & { prompt: AuthPrompt };
  }

  private answerAuth(a: AuthAnswer) {
    const p = this.pending(a.host, a.id).prompt;
    if (p.kind === "secret") {
      if (a.answer === "wrong") return this.setAuthPrompt(a.host, { prompt: p.prompt, kind: "secret", attempt: p.attempt + 1, ...(p.repeat ? { repeat: p.repeat } : {}) });
      return this.setAuthPrompt(a.host, { prompt: "Verification code: ", kind: "code", attempt: 1, ...(p.repeat ? { repeat: p.repeat } : {}) });
    }
    if (p.kind === "code") {
      this.setAuthPrompt(a.host, { prompt: "Duo Push sent to “iPhone 15”. Approve the request on your device to continue.", kind: "info", attempt: 1, ...(p.repeat ? { repeat: p.repeat } : {}) });
      this.later(2500 * (this.opts.speed || 1), () => this.finishAuth(a.host, p.id + 1));
      return;
    }
    this.finishAuth(a.host, a.id);
  }

  private finishAuth(id: HostId, promptId: number) {
    const h = this.hosts.get(id);
    if (!h?.prompt || h.prompt.id !== promptId) return;
    const repeat = h.repeat;
    if (repeat) {
      // The hub has to connect again within this sign-in (a helper update, say): the user is asked again, and told why.
      h.repeat = undefined;
      this.setAuthPrompt(id, { prompt: this.passwordPrompt(h), kind: "secret", attempt: 1, repeat });
      return;
    }
    this.setAuthPrompt(id, null);
    h.cause = undefined;
    this.setLink(id, connected(id));
  }

  private cancelAuth(a: AuthCancel) {
    const h = this.pending(a.host, a.id);
    this.setAuthPrompt(a.host, null);
    // The user's own cancel is an attempt that failed, not a link the hub holds: no `held`.
    this.setLink(a.host, { state: "auth_needed", problem: "auth_needed", detail: "Sign-in was cancelled.", ...(h.cause ? { cause: h.cause } : {}) });
  }

  // ── what the hub does on its own (tests, demos) ───────────────────────────

  /** The prompt the fleet shows for `id` now (tests): what an answer must be bound to. */
  promptOf(id: HostId): AuthPrompt | null {
    return this.hosts.get(id)?.prompt ?? null;
  }

  /** Play the desktop shell's `navigate` event: a notification brought the app forward on a session of `host`, or on the host itself. */
  navigate(target: { host: HostId; session?: string }) {
    this.navigators.forEach((cb) => cb(target));
  }

  onNavigate(cb: (t: { host: HostId; session?: string }) => void): Unsubscribe {
    this.navigators.add(cb);
    return () => void this.navigators.delete(cb);
  }

  /**
   * A connected host's link drops, as the hub reports it: a host that asks for a sign-in each time is parked
   * until the user connects it (`auth_needed`, `held`, with the drop's reason as `cause`); any other is
   * reconnected by the hub itself.
   */
  dropLink(id: HostId, cause: string) {
    const h = this.hosts.get(id)!;
    h.cause = cause;
    if (h.def.entry.interactive_auth) this.setLink(id, { state: "auth_needed", problem: "auth_needed", detail: HELD_DETAIL, cause, held: true });
    else this.setLink(id, { state: "reconnecting", attempt: 1, next_at: nextAt(this.opts.now() + 250), last_error: cause });
  }

  /**
   * The connect attempt that raised the host's prompt ends (it ran out of time, or its connection closed):
   * the hub withdraws the prompt, and a host that asks each time is held again.
   */
  endAttempt(id: HostId) {
    const h = this.hosts.get(id)!;
    this.setAuthPrompt(id, null);
    if (h.def.entry.interactive_auth) this.setLink(id, { state: "auth_needed", problem: "auth_needed", detail: HELD_DETAIL, ...(h.cause ? { cause: h.cause } : {}), held: true });
  }

  /** What the hub learns from a connect that asked the user something: the host signs in interactively. */
  markInteractive(id: HostId) {
    const h = this.hosts.get(id)!;
    h.def.entry = { ...h.def.entry, interactive_auth: true };
    this.fleet({ kind: "host_upserted", host: h.def.entry });
  }

  /** After the host's next successful sign-in the hub has to connect again within it, and the user is asked again for `why`. */
  repeatNext(id: HostId, why: SignInRepeat) {
    this.hosts.get(id)!.repeat = why;
  }

  private createSession(host: HostId, snap: SessionSnapshot, first: string | null) {
    const e: Entry = { snap, events: [], subs: new Set(), host };
    this.hosts.get(host)!.sessions.set(snap.id, e);
    this.sim.emit(e, { kind: "spawned", generation: 1, pid: snap.pid ?? null, argv: [driverDef(snap.driver).binary] });
    // Like the daemon: the first prompt is journaled with the start, before the agent says anything.
    const input = first ? this.sim.firstPrompt(e, first, this.requestTrace) : undefined;
    this.publish(e);
    // A resumed conversation's earlier messages arrive after the start: one record, then the end of the import.
    if (snap.earlier.state.kind === "loading") {
      e.earlierPlan = { chunks: [historyRounds(4, this.opts.now() - 26 * 3_600_000)], final: (info) => ({ ...info, state: { kind: "loaded", complete: true } }) };
      this.arrive(e);
    }
    this.later(300 * this.opts.speed, () => (first ? this.sim.startTurn(e, first, input) : this.sim.ready(e)));
  }

  /** The next record of a history that is still arriving comes in a moment, and the one after that (a live fixture). */
  private arrive(e: Entry) {
    this.later(1800 * this.opts.speed, () => {
      if (advanceEarlier(this.sim, e)) this.arrive(e);
    });
  }

  private later(ms: number, fn: () => void) {
    const h = setTimeout(() => {
      this.timers.delete(h);
      fn();
    }, ms);
    this.timers.add(h);
  }

  // ── background chatter (live mode) ────────────────────────────────────────

  private tick() {
    const n = ++this.tickN;
    const win = this.hosts.get("win")!;
    if (win.def.link?.state === "reconnecting" && n > 15) this.setLink("win", connected("win"));
    else if (win.def.link?.state === "reconnecting" && n % 5 === 0) this.setLink("win", { ...win.def.link, attempt: win.def.link.attempt + 1, next_at: nextAt(this.opts.now() + 4000) });
    if (n === 8) this.setLink("pi", connected("pi"));
    const x1 = this.hosts.get("x1")!;
    for (const e of x1.sessions.values()) {
      if (e.snap.phase.state !== "working" || n % 4 !== 0) continue;
      const tools: [string, unknown][] = [["Edit", { file_path: "src-tauri/capabilities/default.json" }], ["Bash", { command: "cargo check" }], ["Read", { file_path: "Cargo.toml" }]];
      const [name, input] = tools[(n / 4) % tools.length | 0];
      this.sim.fakeTool(e, name, input, true, "ok");
      const usage = { ...e.snap.usage, cost_usd: +(e.snap.usage.cost_usd + 0.01).toFixed(2) };
      e.snap = { ...e.snap, live_usage: usage, usage };
      this.publish(e);
      if (n === 48) this.sim.finishTurn(e, "Tauri 2.5 upgrade is in: capabilities migrated, cargo check and the UI tests pass.");
    }
  }

  // ── test helpers ──────────────────────────────────────────────────────────

  /**
   * The spawn scenario: the invoice orchestrator asks to start a worker on another host (a
   * `spawn` gate in its session, the Inbox and the workstream). Allow adds the node, connected
   * from the orchestrator and marked "spawned by"; "always" choices update the workstream's
   * spawn policy.
   */
  startSpawnScenario() {
    this.raiseGate("x1", fixtureId(1), {
      gate_type: "spawn", request_id: SPAWN_REQUEST, from: "invoice-export-orchestrator", name: "pdf-font-checker", role: "Checks the Arabic font fallback in rendered invoice PDFs",
      host: "hetzner-ax41", driver: "OpenCode", model: "deepseek-v4.1-flash", cwd: "~/src/ledgerline", mode: null, instruction: "Render INV-2041 and confirm every Arabic line uses a fallback font.",
      workstream: "Invoice export v2", new_workstream: false, details: { host_id: "hz", driver_id: "opencode", workstream_id: "ws-invoice", depth: 1, max_depth: 2, spawned: 0, max_agents: 8 }, choices: SPAWN_CHOICES,
    });
  }

  /** Add a pending gate to a session, as if the agent just asked. */
  raiseGate(hostId: HostId, sessionId: string, gate: GateRequest) {
    this.sim.raiseGate(this.hosts.get(hostId)!.sessions.get(sessionId)!, gate);
  }

  /** Withdraw a pending external gate, as if its request went away (`reason`: what the owner said; the legacy `withdrawn` when none). */
  withdrawGate(hostId: HostId, sessionId: string, requestId: string, reason?: GateReason) {
    this.sim.withdrawGate(this.hosts.get(hostId)!.sessions.get(sessionId)!, requestId, reason);
  }

  /** Change facts of a session (tests): it is republished, with its conversation facts (`continuable`, `resume_command`) recomputed as the runtime does. */
  patchSession(hostId: HostId, sessionId: string, p: Partial<SessionSnapshot>) {
    this.sim.patch(this.hosts.get(hostId)!.sessions.get(sessionId)!, p);
  }

  /**
   * What the agent of a session does besides the turns the UI starts (tests, demos): tasks that run in the
   * background, a subagent's own lines, a turn it starts by itself. Each is the event the engine would journal, and the
   * snapshot follows from the events.
   */
  work(hostId: HostId, sessionId: string) {
    const e = this.hosts.get(hostId)!.sessions.get(sessionId)!;
    const sim = this.sim;
    return {
      startTask: (t: Parameters<SessionSim["startTask"]>[1]) => sim.startTask(e, t),
      endTask: (taskId: string, status?: string, summary?: string) => sim.endTask(e, taskId, status, summary),
      scoped: (scope: Parameters<SessionSim["scoped"]>[1], event: Parameters<SessionSim["scoped"]>[2]) => sim.scoped(e, scope, event),
      startOwnTurn: () => sim.startOwnTurn(e),
      finishOwnTurn: (reply: string, origin?: Parameters<SessionSim["finishOwnTurn"]>[2]) => sim.finishOwnTurn(e, reply, origin),
      /** A turn started by a prompt, as the daemon runs it (it ends by itself unless `finish` is called). */
      startTurn: (prompt: string) => sim.startTurn(e, prompt),
      finishTurn: (reply: string) => sim.finishTurn(e, reply),
    };
  }

  /**
   * The conversation's earlier history of a session, as the engine brings it (tests, demos): `step` takes the next record of a history
   * that is arriving (the last step is the end, with what was left out), `fail` ends it with a reason, `add` brings some items of
   * your own with the state it ends in (the snapshot follows from what the journal holds, like the daemon's).
   */
  earlier(hostId: HostId, sessionId: string) {
    const e = this.hosts.get(hostId)!.sessions.get(sessionId)!;
    const sim = this.sim;
    return {
      step: () => advanceEarlier(sim, e),
      fail: (reason: Parameters<typeof failEarlier>[2]) => failEarlier(sim, e, reason),
      add: (items: HistoryItem[], next?: Parameters<typeof journalEarlier>[3]) => journalEarlier(sim, e, items, next),
      /** The history items the journal holds so far. */
      items: () => e.earlier ?? [],
    };
  }

  /** The next start of a process for this session fails with `reason` (a prompt to it is then refused with `continue_failed`). */
  failNextContinue(hostId: HostId, sessionId: string, reason: string) {
    this.hosts.get(hostId)!.sessions.get(sessionId)!.continueFails = reason;
  }

  sessionIds(hostId: HostId): string[] {
    return [...(this.hosts.get(hostId)?.sessions.keys() ?? [])];
  }

  snapshot(hostId: HostId, id: string): SessionSnapshot {
    return this.hosts.get(hostId)!.sessions.get(id)!.snap;
  }

  hostIds(): HostId[] {
    return [...this.hosts.keys()];
  }
}

function summaryOf(cwd: string, g: GitStatus, now: number): GitSummary {
  let added = 0;
  let removed = 0;
  for (const f of g.files) {
    added += f.staged_added + f.unstaged_added;
    removed += f.staged_removed + f.unstaged_removed;
  }
  return { cwd, repo_root: g.repo_root, branch: g.branch, ahead: g.ahead, behind: g.behind, files: g.files.length, added, removed, refreshed_at_ms: now, error: null };
}
