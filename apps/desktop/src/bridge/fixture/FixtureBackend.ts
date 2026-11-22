import type { Backend, RequestOptions, SubscribeOptions, Unsubscribe } from "../backend";
import type {
  AttachmentPut,
  FileReadRequest,
  FilesStatRequest,
  AuthAnswer, AuthCancel, AuthPrompt, BaseRequest, HostEntry, FleetEvent, FleetHost, FsListRequest, FsPathRequest, GateRequest, GitCommitRequest, GitDiffRequest,
  GitDiscardRequest, GitRefresh, GitStatus, GitStatusRequest, GitSummary, GitTarget, HistoryRequest, RenameSession, HostId, InventoryRequest, LinkState, SessionRef, SessionSnapshot, CommandRequest,
  HostEnvironment, HostEnvSettings, HostRef,
} from "../wire";
import { methods as M, topics as T } from "../wire";
import { DRIVERS, FIXTURE_HOSTS, driverDef, inventoryFor, type FixtureHost } from "./catalog";
import { fsExists, fsHome, fsList } from "./fs";
import { buildTree, diffOf, type Tree } from "./git";
import { FixtureHub } from "./hub";
import { FixtureHostEnvs } from "./hostenv";
import { err, SessionSim, type Entry } from "./runtime";
import { FixtureFiles, MEDIA_SPEC } from "./media";
import { buildSession, fixtureId, SPAWN_CHOICES, SPECS, type Spec } from "./sessions";
import { SPAWN_REQUEST } from "./workstreams";

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
}

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
  private reqLog: { method: string; params: unknown; host?: HostId }[] = [];
  readonly sim: SessionSim;
  readonly envs: FixtureHostEnvs;
  private invSubs = new Map<HostId, Set<(e: unknown) => void>>();

  constructor(o: FixtureOptions = {}) {
    this.opts = { live: o.live ?? true, latencyMs: o.latencyMs ?? 140, speed: o.speed ?? 1, now: o.now ?? Date.now };
    const now = this.opts.now();
    this.envs = new FixtureHostEnvs(this.opts.now);
    this.sim = new SessionSim({ now: this.opts.now, after: (ms, fn) => this.later(ms * this.opts.speed, fn), publish: (e) => this.publish(e) });
    for (const def of FIXTURE_HOSTS) {
      const d = structuredClone(def);
      if (d.link?.state === "reconnecting") d.link.next_at = nextAt(now + 4000);
      this.hosts.set(d.entry.id, { def: d, prompt: null, sessions: new Map() });
    }
    for (const spec of o.specs ?? SPECS) {
      const { snap, events } = buildSession(spec, now);
      this.hosts.get(spec.host)!.sessions.set(snap.id, { snap, events, subs: new Set(), host: spec.host });
      if (spec.n === MEDIA_SPEC.n) this.files.seed(snap.id, snap.cwd ?? "");
      this.trees.set(`${spec.host}/${snap.id}`, buildTree(spec));
    }
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

  dispose() {
    if (this.interval) clearInterval(this.interval);
    this.timers.forEach(clearTimeout);
    this.timers.clear();
  }

  // ── Backend ───────────────────────────────────────────────────────────────

  request<T>(method: string, params?: unknown, opts: RequestOptions = {}): Promise<T> {
    this.reqLog.push({ method, params, host: opts.host });
    return new Promise<T>((resolve, reject) => {
      this.later(this.opts.latencyMs, () => {
        try {
          resolve((opts.host ? this.hostRequest(opts.host, method, params) : this.hubRequest(method, params)) as T);
        } catch (e) {
          reject(e);
        }
      });
    });
  }

  subscribe(topic: string, opts: SubscribeOptions, onEvent: (e: unknown) => void): Unsubscribe {
    if (topic === T.FLEET) {
      this.fleetSubs.add(onEvent);
      this.later(0, () => onEvent({ kind: "reset", hosts: [...this.hosts.values()].map((h) => this.fleetHost(h)) } satisfies FleetEvent));
      return () => this.fleetSubs.delete(onEvent);
    }
    if (topic === T.WORKSTREAMS) {
      this.wsSubs.add(onEvent);
      this.later(0, () => onEvent(this.hub.ws.reset()));
      return () => this.wsSubs.delete(onEvent);
    }
    const host = this.hosts.get(opts.host ?? "x1");
    if (!host) return () => undefined;
    if (topic === T.DRIVERS_INVENTORY) {
      const id = host.def.entry.id;
      const subs = this.invSubs.get(id) ?? new Set();
      this.invSubs.set(id, subs);
      subs.add(onEvent);
      this.later(0, () => this.isUp(host) && onEvent(this.hostInventory(id)));
      return () => subs.delete(onEvent);
    }
    const m = /^session\.events:(.+)$/.exec(topic);
    const entry = m ? host.sessions.get(m[1]) : undefined;
    if (!entry) return () => undefined;
    const from = Number(opts.from ?? Number.POSITIVE_INFINITY);
    entry.subs.add(onEvent);
    if (Number.isFinite(from)) this.later(0, () => entry.events.filter((e) => e.cursor > from).forEach((e) => onEvent(e)));
    return () => entry.subs.delete(onEvent);
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
      case M.SESSIONS_LIST:
        return { sessions: [...host.sessions.values()].map((e) => e.snap) };
      case M.SESSION_SNAPSHOT:
        return this.entry(host, p as SessionRef).snap;
      case M.SESSION_HISTORY: {
        const r = p as HistoryRequest;
        const all = this.entry(host, r).events.filter((e) => r.before == null || e.cursor < r.before);
        const events = all.slice(Math.max(0, all.length - r.limit));
        return { events, has_more: all.length > events.length };
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
        const reply = this.sim.command(e, r.command);
        if (r.command.command === "answer_gate") this.hub.ws.onHandoffAnswer(r.command.request_id, r.command.decision.choice === "deny");
        return reply;
      }
      case M.SESSIONS_CONTINUE:
        this.sim.setPhase(this.entry(host, p as SessionRef), { state: "idle" });
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
        if (e.snap.phase.state === "working" || e.snap.phase.state === "needs_you" || e.snap.phase.state === "starting") this.sim.setPhase(e, { state: "exited", code: 0 });
        host.sessions.delete(r.id);
        this.fleet({ kind: "session_removed", host: hostId, id: r.id });
        return {};
      }
      case M.SESSIONS_STOP:
        this.sim.setPhase(this.entry(host, p as SessionRef), { state: "exited", code: 0 });
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
      case M.PTY_START:
      case M.PTY_LIST:
        throw err("unsupported", "Terminals need the Marsad helper on a real host; the demo backend has none.", { marsad_error: "unsupported", message: "no pty in the demo backend" });
      default:
        throw err("method_not_found", `Fixture does not implement ${method}`);
    }
  }

  private hubRequest(method: string, p: unknown): unknown {
    // The desktop shell's window background: a browser tab can always show alpha (CSS only).
    if (method === "desktop.window_surface") return { transparent: (p as { translucent: boolean }).translucent, effect: null };
    if (method === M.FLEET_GIT_REFRESH) {
      const r = p as GitRefresh;
      const e = [...(this.hosts.get(r.host)?.sessions.values() ?? [])].find((x) => x.snap.cwd === r.cwd);
      if (e) this.gitChanged(r.host, { session: e.snap.id, cwd: null });
      return {};
    }
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
    this.later(0, () => this.invSubs.get(id)?.forEach((cb) => cb(inv)));
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
    this.fleetSubs.forEach((s) => s(e));
  }

  private publish(e: Entry) {
    this.fleet({ kind: "session_upserted", host: e.host, session: e.snap });
    this.hub?.ws.onSession(e.host, e.snap.id);
  }

  /** Force a link state (tests, demos). */
  setLink(id: HostId, state: LinkState) {
    const h = this.hosts.get(id)!;
    h.def.link = state;
    this.fleet({ kind: "link", host: id, state });
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

  private reconnect(id: HostId) {
    const h = this.hosts.get(id);
    if (!h) throw err("not_found", `No host ${id}`);
    if (!h.def.link) return;
    this.setLink(id, { state: "connecting" });
    if (h.def.entry.interactive_auth) {
      this.later(300 * this.opts.speed, () => this.setAuthPrompt(id, { prompt: `${h.def.entry.kind.kind === "ssh" ? h.def.entry.kind.target : id}'s password: `, kind: "secret", attempt: 1 }));
    } else this.later(800 * this.opts.speed, () => this.setLink(id, connected(id)));
  }

  private answerAuth(a: AuthAnswer) {
    const h = this.hosts.get(a.host);
    if (!h?.prompt || h.prompt.id !== a.id) throw err("not_found", "That sign-in prompt has expired.");
    const p = h.prompt;
    if (p.kind === "secret") {
      if (a.answer === "wrong") return this.setAuthPrompt(a.host, { prompt: p.prompt, kind: "secret", attempt: p.attempt + 1 });
      return this.setAuthPrompt(a.host, { prompt: "Verification code: ", kind: "code", attempt: 1 });
    }
    if (p.kind === "code") {
      this.setAuthPrompt(a.host, { prompt: "Duo Push sent to “iPhone 15”. Approve the request on your device to continue.", kind: "info", attempt: 1 });
      this.later(2500 * (this.opts.speed || 1), () => this.finishAuth(a.host, p.id + 1));
      return;
    }
    this.finishAuth(a.host, a.id);
  }

  private finishAuth(id: HostId, promptId: number) {
    const h = this.hosts.get(id);
    if (!h?.prompt || h.prompt.id !== promptId) return;
    this.setAuthPrompt(id, null);
    this.setLink(id, connected(id));
  }

  private cancelAuth(a: AuthCancel) {
    const h = this.hosts.get(a.host);
    if (!h?.prompt) return;
    this.setAuthPrompt(a.host, null);
    this.setLink(a.host, { state: "auth_needed", problem: "auth_needed", detail: "Sign-in was cancelled." });
  }

  private createSession(host: HostId, snap: SessionSnapshot, first: string | null) {
    const e: Entry = { snap, events: [], subs: new Set(), host };
    this.hosts.get(host)!.sessions.set(snap.id, e);
    this.sim.emit(e, { kind: "spawned", generation: 1, pid: snap.pid ?? null, argv: [driverDef(snap.driver).binary] });
    this.publish(e);
    this.later(300 * this.opts.speed, () => (first ? this.sim.startTurn(e, first) : this.sim.setPhase(e, { state: "idle" })));
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

  /** Withdraw a pending external gate, as if its request went away. */
  withdrawGate(hostId: HostId, sessionId: string, requestId: string) {
    this.sim.withdrawGate(this.hosts.get(hostId)!.sessions.get(sessionId)!, requestId);
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
