import type { Api } from "../bridge/api";
import { errorMessage } from "../bridge/api";
import type { FleetEvent, FleetHost, HostId, InventoryReply, SessionSnapshot, WorkstreamsEvent } from "../bridge/wire";
import { sessionKey } from "../bridge/wire";
import { makeDriver } from "../ui/DriverBadge";
import { linkView, type HostState } from "./hosts";
import type { AppState, AppStore } from "./model";

const toHost = (f: FleetHost): HostState => ({ entry: f.host, link: f.link, sessionsLive: f.sessions_live, authPrompt: f.auth_prompt ?? null });

const warned = new Set<string>();

/** An event of a kind this build does not know: say so once per kind and leave the state alone. */
function unknownEvent(topic: string, e: unknown): void {
  const kind = String((e as { kind?: unknown } | null)?.kind);
  if (warned.has(`${topic}:${kind}`)) return;
  warned.add(`${topic}:${kind}`);
  console.warn(`ignored a ${topic} event of unknown kind "${kind}"`);
}

/** Pure (apart from that warning): apply one `fleet` event. */
export function applyFleet(s: AppState, e: FleetEvent): AppState {
  switch (e?.kind) {
    case "reset": {
      const hosts: AppState["hosts"] = {};
      const sessions: AppState["sessions"] = {};
      const git: AppState["git"] = {};
      for (const f of e.hosts) {
        hosts[f.host.id] = toHost(f);
        git[f.host.id] = Object.fromEntries((f.git ?? []).map((g) => [g.cwd, g]));
        for (const snap of f.sessions) sessions[sessionKey(f.host.id, snap.id)] = snap;
      }
      return withDrivers({ ...s, fleetReady: true, fleetError: undefined, hosts, hostOrder: e.hosts.map((f) => f.host.id), sessions, git, pending: prunePending(s.pending, sessions) });
    }
    case "host_upserted": {
      const cur = s.hosts[e.host.id];
      const h: HostState = cur ? { ...cur, entry: e.host } : { entry: e.host, link: e.host.kind.kind === "local" ? { link: "local" } : { link: "remote", state: { state: "connecting" } }, sessionsLive: false, authPrompt: null };
      return { ...s, hosts: { ...s.hosts, [e.host.id]: h }, hostOrder: cur ? s.hostOrder : [...s.hostOrder, e.host.id] };
    }
    case "host_removed": {
      const hosts = { ...s.hosts };
      delete hosts[e.id];
      const sessions = { ...s.sessions };
      for (const k of Object.keys(sessions)) if (k.startsWith(e.id + "/")) delete sessions[k];
      return { ...s, hosts, sessions, hostOrder: s.hostOrder.filter((x) => x !== e.id) };
    }
    case "link": {
      const cur = s.hosts[e.host];
      if (!cur || cur.link.link === "local") return s;
      return { ...s, hosts: { ...s.hosts, [e.host]: { ...cur, link: { link: "remote", state: e.state } } } };
    }
    case "git":
      return { ...s, git: { ...s.git, [e.host]: { ...s.git[e.host], [e.summary.cwd]: e.summary } } };
    case "auth_prompt": {
      const cur = s.hosts[e.host];
      if (!cur) return s;
      return { ...s, hosts: { ...s.hosts, [e.host]: { ...cur, authPrompt: e.prompt } } };
    }
    case "sessions": {
      const sessions = { ...s.sessions };
      for (const k of Object.keys(sessions)) if (k.startsWith(e.host + "/")) delete sessions[k];
      for (const snap of e.sessions) sessions[sessionKey(e.host, snap.id)] = snap;
      const cur = s.hosts[e.host];
      const hosts = cur ? { ...s.hosts, [e.host]: { ...cur, sessionsLive: true } } : s.hosts;
      return withDrivers({ ...s, sessions, hosts, pending: prunePending(s.pending, sessions) });
    }
    case "session_upserted": {
      const sessions = { ...s.sessions, [sessionKey(e.host, e.session.id)]: e.session };
      return withDrivers({ ...s, sessions, pending: prunePending(s.pending, sessions) });
    }
    case "session_removed": {
      const sessions = { ...s.sessions };
      delete sessions[sessionKey(e.host, e.id)];
      return { ...s, sessions };
    }
    default:
      unknownEvent("fleet", e);
      return s;
  }
}

/** Pure (apart from the unknown-kind warning): apply one `workstreams` event. */
export function applyWorkstreams(s: AppState, e: WorkstreamsEvent): AppState {
  if (e?.kind === "reset") {
    const workstreams: AppState["workstreams"] = {};
    for (const v of e.workstreams) workstreams[v.workstream.id] = v;
    return { ...s, workstreams, wsOrder: e.workstreams.map((v) => v.workstream.id), workstreamsLoaded: true };
  }
  if (e?.kind === "upserted") {
    const id = e.workstream.workstream.id;
    return { ...s, workstreams: { ...s.workstreams, [id]: e.workstream }, wsOrder: s.wsOrder.includes(id) ? s.wsOrder : [...s.wsOrder, id] };
  }
  if (e?.kind === "removed") {
    const workstreams = { ...s.workstreams };
    delete workstreams[e.id];
    return { ...s, workstreams, wsOrder: s.wsOrder.filter((x) => x !== e.id) };
  }
  unknownEvent("workstreams", e);
  return s;
}

/** Learn driver presentation from inventories and session snapshots (never hardcoded). */
function withDrivers(s: AppState): AppState {
  let drivers: AppState["drivers"] | null = null;
  let order: string[] | null = null;
  const learn = (id: string, name?: string) => {
    const cur = (drivers ?? s.drivers)[id];
    if (cur && (!name || cur.displayName === name)) return;
    drivers ??= { ...s.drivers };
    order ??= [...s.driverOrder];
    if (!cur) order.push(id);
    drivers[id] = makeDriver(id, name ?? cur?.displayName);
  };
  for (const inv of Object.values(s.inventory)) for (const d of inv) learn(d.driver, d.display_name);
  for (const snap of Object.values(s.sessions)) learn(snap.driver);
  if (!drivers) return s;
  // Inventory order first (the order discovery reports), then drivers only seen in sessions.
  const inv = Object.values(s.inventory).flat().map((d) => d.driver);
  const rank = (id: string) => (inv.indexOf(id) < 0 ? 1e6 + order!.indexOf(id) : inv.indexOf(id));
  return { ...s, drivers, driverOrder: order!.sort((a, b) => rank(a) - rank(b)) };
}

/**
 * Wires the backend into the store: the hub's `fleet` topic (hosts, links, sign-in prompts,
 * sessions), `drivers.inventory` of every connected host, and the hub store documents the shell
 * needs (seen markers, workstreams, notification prefs, settings). Returns a disposer.
 */
export function connectStore(store: AppStore, api: Api): () => void {
  const inv = new Map<HostId, () => void>();
  const syncInventory = () => {
    const s = store.get();
    for (const id of s.hostOrder) {
      const h = s.hosts[id];
      const up = h && linkView(h.link).connected;
      if (up && !inv.has(id)) {
        inv.set(id, api.subscribe.inventory(id, (r: InventoryReply) => store.set((st) => withDrivers({ ...st, inventory: { ...st.inventory, [id]: r.drivers } }))));
      } else if (!up && inv.has(id)) {
        inv.get(id)!();
        inv.delete(id);
      }
    }
  };
  const offFleet = api.subscribe.fleet((e) => {
    store.set((s) => applyFleet(s, e));
    if (e?.kind === "reset" || e?.kind === "link" || e?.kind === "host_upserted" || e?.kind === "host_removed") syncInventory();
  });

  const fail = (what: string) => (e: unknown) => console.warn(`${what}: ${errorMessage(e)}`);
  api.store.seen().then((m) => store.set((s) => ({ ...s, ...seenState(m.markers) })), fail("seen markers"));
  const offWs = api.subscribe.workstreams((e) => store.set((s) => applyWorkstreams(s, e)));
  api.store.notifications().then((n) => store.set((s) => ({ ...s, notifications: n })), fail("notification prefs"));
  api.store.settings().then((st) => store.set((s) => ({ ...s, settings: st.values as Record<string, unknown> })), fail("settings"));

  return () => {
    offFleet();
    offWs();
    inv.forEach((u) => u());
    inv.clear();
  };
}

export function seenState(markers: { host: string; session: string; cursor: number; at_ms: number }[]): Pick<AppState, "seen" | "lookedAt"> {
  const seen: AppState["seen"] = {};
  let lookedAt = 0;
  for (const m of markers) {
    seen[sessionKey(m.host, m.session)] = m;
    lookedAt = Math.max(lookedAt, m.at_ms);
  }
  return { seen, lookedAt };
}

/** Drop "sent" markers whose gate is gone from the snapshot. */
function prunePending(pending: AppState["pending"], sessions: Record<string, SessionSnapshot>): AppState["pending"] {
  let next: AppState["pending"] | null = null;
  for (const k of Object.keys(pending)) {
    const m = /^gate:(.+):([^:]+)$/.exec(k);
    if (!m || pending[k] !== "sent") continue;
    const snap = sessions[m[1]];
    if (!snap || !snap.pending_gates.some((g) => g.gate.request_id === m[2])) {
      next ??= { ...pending };
      delete next[k];
    }
  }
  return next ?? pending;
}
