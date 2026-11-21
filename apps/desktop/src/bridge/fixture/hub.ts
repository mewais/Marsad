import type {
  AuthAnswer, AuthCancel, EffectiveRequest, HostDraft, HostEntry, HostRef, LaunchReply, LaunchRequest, LayerScope, NotificationPrefs, RecentProject, SeenMark, SessionSnapshot,
  SettingSet, StoredLayer, WorkstreamsEvent,
} from "../wire";
import { methods as M } from "../wire";
import { layerAt, mergeEffective, sameScope, seedLayers } from "./config";
import { driverDef, type FixtureHost } from "./catalog";
import { err } from "./runtime";
import { FixtureWorkstreams } from "./workstreams";

/** What the hub needs from the fixture core. */
export interface HubCore {
  now(): number;
  hostDefs(): FixtureHost[];
  addHost(def: FixtureHost): void;
  removeHost(id: string): void;
  updateHost(entry: HostEntry): HostEntry;
  reconnect(id: string): void;
  answerAuth(a: AuthAnswer): void;
  cancelAuth(a: AuthCancel): void;
  createSession(host: string, snap: SessionSnapshot, firstPrompt: string | null): void;
  session(host: string, id: string): SessionSnapshot | undefined;
  emitWorkstreams(e: WorkstreamsEvent): void;
}

/** Fixture-only: the hub's services (hosts, store documents, effective config, launch). */
export class FixtureHub {
  settings: Record<string, unknown> = { theme: "system", "defaults.driver": "claude" };
  notifications: NotificationPrefs = { needs_you: true, failed: true, finished: true, host_lost: true };
  layers: StoredLayer[] = seedLayers();
  seen: SeenMark[] = [];
  recent: RecentProject[];
  readonly ws: FixtureWorkstreams;
  private n = 100;

  constructor(private core: HubCore) {
    const now = core.now();
    this.recent = ["ledgerline", "marsad", "docs-site", "tilemap-renderer"].map((p, i) => ({ host: "x1", path: `~/src/${p}`, last_used_ms: now - i * 3_600_000 }));
    this.ws = new FixtureWorkstreams({ now: core.now, session: core.session, launch: (r) => this.launch(r), emit: core.emitWorkstreams });
  }

  handle(method: string, p: unknown): unknown {
    const c = this.core;
    switch (method) {
      case M.HOSTS_LIST:
        return { hosts: c.hostDefs().map((h) => h.entry) };
      case M.HOSTS_ADD: {
        const d = p as HostDraft;
        if (!d.name.trim()) throw err("invalid_params", "A host needs a name.");
        const id = d.id ?? d.name.toLowerCase().replace(/[^a-z0-9-]+/g, "-");
        if (c.hostDefs().some((h) => h.entry.id === id)) throw err("invalid_params", `A host with id “${id}” already exists.`);
        const entry: HostEntry = { id, name: d.name, kind: d.kind, platform: null, created_at_ms: c.now(), interactive_auth: false };
        c.addHost({ entry, link: { state: "connecting" }, drivers: { claude: "2.1.4", codex: "0.58.0" } });
        return entry;
      }
      case M.HOSTS_UPDATE:
        return c.updateHost(p as HostEntry);
      case M.HOSTS_REMOVE:
        c.removeHost((p as HostRef).id);
        return {};
      case M.HOSTS_RECONNECT:
        c.reconnect((p as HostRef).id);
        return {};
      case M.HOSTS_AUTH_ANSWER:
        c.answerAuth(p as AuthAnswer);
        return {};
      case M.HOSTS_AUTH_CANCEL:
        c.cancelAuth(p as AuthCancel);
        return {};
      case M.HOSTS_WSL_DISTROS:
        return {
          available: true,
          distros: [
            { name: "Ubuntu-24.04", default: true, running: true, version: 2 },
            { name: "Debian", default: false, running: false, version: 2 },
          ],
        };
      case M.HOSTS_IMPORT_SUGGESTIONS:
        return {
          source: "~/.ssh/config",
          hosts: [
            { alias: "hetzner", hostname: "95.217.40.12", user: "dev", port: 22, identity_file: "~/.ssh/id_ed25519", already_added: true },
            { alias: "ci-runner-02", hostname: "ci-runner-02.internal", user: "ops", port: 22, identity_file: "~/.ssh/id_ed25519", already_added: false },
            { alias: "mac-mini", hostname: "mac-mini.lan", user: "dev", port: null, identity_file: null, already_added: false },
          ],
        };
      case M.DAEMON_INFO:
        return { version: { build: "fixture-0.1.0", target: "x86_64-unknown-linux-gnu" }, host_id: "x1", hub: true, data_dir: "~/.local/share/marsad", pid: 4242 };
      case M.STORE_SETTINGS_GET:
        return { values: this.settings };
      case M.STORE_SETTINGS_SET: {
        const s = p as SettingSet;
        if (s.value === null) delete this.settings[s.key];
        else this.settings[s.key] = s.value;
        return { values: this.settings };
      }
      case M.STORE_NOTIFICATIONS_GET:
        return this.notifications;
      case M.STORE_NOTIFICATIONS_SET:
        this.notifications = { ...(p as NotificationPrefs) };
        return this.notifications;
      case M.STORE_LAYERS_LIST:
        return { layers: this.layers };
      case M.STORE_LAYERS_GET:
        return layerAt(this.layers, p as LayerScope);
      case M.STORE_LAYERS_SET: {
        const l = p as StoredLayer;
        this.layers = [...this.layers.filter((x) => !sameScope(x.scope, l.scope)), l];
        return {};
      }
      case M.STORE_SEEN_LIST:
        return { markers: this.seen };
      case M.STORE_SEEN_MARK: {
        const m = p as SeenMark;
        this.seen = [...this.seen.filter((x) => !(x.host === m.host && x.session === m.session)), m];
        return {};
      }
      case M.STORE_SEEN_MARK_ALL:
        return { markers: this.seen };
      case M.STORE_RECENT_LIST:
        return { projects: this.recent };
      case M.STORE_RECENT_ADD: {
        const r = p as RecentProject;
        this.recent = [{ ...r, last_used_ms: r.last_used_ms || c.now() }, ...this.recent.filter((x) => !(x.host === r.host && x.path === r.path))].slice(0, 50);
        return { projects: this.recent };
      }
      case M.AGENTCONFIG_EFFECTIVE:
        return this.effective(p as EffectiveRequest);
      case M.SESSIONS_LAUNCH:
        return this.launch(p as LaunchRequest);
      default:
        if (method.startsWith("workstreams.")) return this.ws.handle(method, p);
        throw err("method_not_found", `Fixture does not implement ${method}`);
    }
  }

  effective(r: EffectiveRequest) {
    const host = r.host ?? "x1";
    const def = this.core.hostDefs().find((h) => h.entry.id === host);
    if (!def?.drivers[r.driver]) throw err("not_found", `${driverDef(r.driver)?.display_name ?? r.driver} is not installed on ${def?.entry.name ?? host}.`);
    const scopes: LayerScope[] = [{ scope: "global" }, { scope: "host", host }];
    if (r.workstream) scopes.push({ scope: "workstream_node", workstream: r.workstream.workstream, node: r.workstream.node });
    if (r.session) scopes.push({ scope: "session", host, session: r.session });
    const layers = scopes.map((scope) => ({ scope, layer: layerAt(this.layers, scope) }));
    if (r.overrides) layers.push({ scope: { scope: "session", host, session: r.session ?? "(new)" }, layer: r.overrides });
    const inWs = !!r.workstream || (!!r.session && [...this.ws.docs.values()].some((w) => w.state !== "archived" && w.nodes.some((n) => n.session?.session === r.session)));
    return mergeEffective({ driver: r.driver, base: driverDef(r.driver).config, layers, workstream: inWs });
  }

  launch(r: LaunchRequest): LaunchReply {
    const host = r.host ?? "x1";
    const effective = this.effective({ host, driver: r.driver, project: r.cwd, workstream: r.workstream, overrides: r.overrides });
    const d = driverDef(r.driver);
    const id = `fxnew${String(this.n++).padStart(7, "a")}`.slice(0, 12);
    const now = this.core.now();
    const snap: SessionSnapshot = {
      id, driver: r.driver, cwd: r.cwd, labels: r.labels, created_at_ms: now, capabilities: d.capabilities, prompt_input: d.prompt_input, phase: { state: "starting" }, generation: 1, pid: 5000 + this.n,
      native_session_id: r.resume.kind === "resume" ? r.resume.native_session_id : null, title: (r.first_prompt ?? "").slice(0, 80) || null, cli_version: d.version,
      model: r.model ?? d.models.find((m) => m.is_default)?.id ?? null, mode: r.mode ?? d.modes[0]?.id ?? null, available_models: d.models, available_modes: d.modes,
      slash_commands: d.slash, pending_gates: [], turn: null, last_turn: null, turns_completed: 0, usage: { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0 },
      live_usage: null, context_pct: 4, subagents: [], plan: [], last_activity_ms: now, exit: null, stderr_tail: [], cursor: 0, loaded: true,
    };
    if (r.overrides.items.length || r.overrides.env.length) this.layers.push({ scope: { scope: "session", host, session: id }, layer: r.overrides });
    if (r.cwd) this.handle(M.STORE_RECENT_ADD, { host, path: r.cwd, last_used_ms: now });
    this.core.createSession(host, snap, r.first_prompt ?? null);
    return { host, id, effective, unsupported: effective.items.filter((i) => i.unsupported).map((i) => ({ item: `${i.kind}:${i.name}`, reason: i.unsupported! })) };
  }
}
