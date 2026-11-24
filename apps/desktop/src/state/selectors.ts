import type { HostId, SessionKey } from "../bridge/wire";
import { sessionKey, splitKey } from "../bridge/wire";
import type { WorkstreamView } from "../bridge/wire";
import { STATUS_ORDER, type StatusKey } from "../ui/status";
import { buildView, type SessionView } from "./domain";
import { hostView, hubView, linkView, type HostView } from "./hosts";
import type { AppState } from "./model";
import { heldLink, type HeldLink, type SignInQuestion, type WaitingHost } from "./signin";

const wsIndex = new WeakMap<Record<string, WorkstreamView>, Map<SessionKey, string>>();
/** Session key → workstream name, from the hub's workstream documents. */
export function workstreamIndex(all: Record<string, WorkstreamView>): Map<SessionKey, string> {
  let m = wsIndex.get(all);
  if (!m) {
    m = new Map();
    for (const v of Object.values(all)) {
      if (v.workstream.state === "archived") continue;
      for (const n of v.workstream.nodes) if (n.session) m.set(sessionKey(n.session.host, n.session.session), v.workstream.name);
    }
    wsIndex.set(all, m);
  }
  return m;
}

/**
 * The fleet is live: the app is connected to its local daemon and the `fleet` subscription is
 * running. Without that, every host's data is the last received.
 */
export const fleetLive = (s: AppState): boolean => s.hub.state === "connected" && !s.feedErrors.fleet;

/**
 * Whether what the app shows of `host` is the last known rather than live: the fleet is not
 * live, or the host has not delivered its session list over its current link (`sessions_live`,
 * which the hub clears when the link drops).
 */
export const hostStale = (s: AppState, host: HostId): boolean => {
  const h = s.hosts[host];
  return !fleetLive(s) || !h?.sessionsLive || !!h?.feedError;
};

/**
 * Why a request that needs the daemon cannot be made right now, or `null`: the app is not
 * connected to its local daemon, or (for `host`) that host is not connected. The words are
 * shown where the action is disabled; the request is not sent, so nothing waits on a link that
 * is down.
 */
export function reachBlock(s: AppState, host?: HostId): string | null {
  if (s.hub.state !== "connected") return hubView(s.hub).detail;
  const h = host ? s.hosts[host] : undefined;
  if (h && h.link.link === "remote" && h.link.state.state !== "connected") return `${h.entry.name} is not connected (${linkView(h.link).word.toLowerCase()}).`;
  return null;
}

/** The hub setting that turns the Diagnostics panels on. */
export const DEVELOPER_MODE = "developer.mode";

/** Developer mode is on only when the hub's settings have loaded and say so. */
export const selectDeveloperMode = (s: AppState): boolean => s.loads.settings.state === "ready" && s.settings[DEVELOPER_MODE] === true;

export function selectView(s: AppState, key: SessionKey): SessionView | undefined {
  const snap = s.sessions[key];
  const host = splitKey(key).host;
  return snap ? buildView(host, snap, s.seen[key], snap.cwd ? s.git[host]?.[snap.cwd] : undefined, workstreamIndex(s.workstreams).get(key) ?? "", hostStale(s, host)) : undefined;
}

export interface Filters {
  q: string;
  status: StatusKey | "all";
  driver: string | "any";
  hosts: HostId[] | "all";
  workstream: string | "any";
}
export const NO_FILTERS: Filters = { q: "", status: "all", driver: "any", hosts: "all", workstream: "any" };

export function matches(v: SessionView, f: Filters, hostName: string): boolean {
  if (f.status !== "all" && v.status !== f.status) return false;
  if (f.driver !== "any" && v.driver !== f.driver) return false;
  if (f.hosts !== "all" && !f.hosts.includes(v.host)) return false;
  if (f.workstream !== "any" && v.workstream !== f.workstream) return false;
  const q = f.q.trim().toLowerCase();
  if (!q) return true;
  return [v.name, v.project, v.branch, v.workstream, hostName, v.line].some((x) => x.toLowerCase().includes(q));
}

export type Groups = Record<StatusKey, SessionKey[]>;

/** Keys grouped by status; within a group newest activity first. Compare with `groupsEqual`. */
export function selectGroups(s: AppState, f: Filters): Groups {
  const out: Groups = { needs: [], failed: [], done: [], working: [], idle: [], ended: [] };
  const rows: { v: SessionView; at: number }[] = [];
  for (const key of Object.keys(s.sessions)) {
    const v = selectView(s, key)!;
    if (matches(v, f, s.hosts[v.host]?.entry.name ?? v.host)) rows.push({ v, at: v.lastMs });
  }
  rows.sort((a, b) => b.at - a.at);
  for (const r of rows) out[r.v.status].push(r.v.key);
  return out;
}

export function groupsEqual(a: Groups, b: Groups): boolean {
  return STATUS_ORDER.every((k) => a[k].length === b[k].length && a[k].every((x, i) => x === b[k][i]));
}

/** Live sessions per status. A session whose data is last-known is in `stale` instead: it is not live activity. */
export interface Counts {
  needs: number;
  failed: number;
  done: number;
  working: number;
  idle: number;
  ended: number;
  stale: number;
  /** Hosts that wait for the user to sign in (live fleet only): the hub does not reconnect them by itself. */
  hostsWaiting: number;
  total: number;
  todayCost: number;
  todaySessions: number;
}

export function selectCounts(s: AppState): Counts {
  const c: Counts = { needs: 0, failed: 0, done: 0, working: 0, idle: 0, ended: 0, stale: 0, hostsWaiting: heldHosts(s).length, total: 0, todayCost: 0, todaySessions: 0 };
  const midnight = new Date();
  midnight.setHours(0, 0, 0, 0);
  for (const key of Object.keys(s.sessions)) {
    const v = selectView(s, key)!;
    c[v.stale ? "stale" : v.status]++;
    c.total++;
    if (v.lastMs >= midnight.getTime()) {
      c.todayCost += v.cost;
      c.todaySessions++;
    }
  }
  c.todayCost = Math.round(c.todayCost * 100) / 100;
  return c;
}

/** Hosts in fleet order, this machine first. */
export const selectHosts = (s: AppState): HostView[] => {
  const live = fleetLive(s);
  const v = s.hostOrder.filter((id) => s.hosts[id]).map((id) => hostView(s.hosts[id], s.hub, live));
  return [...v.filter((h) => h.local), ...v.filter((h) => !h.local)];
};
export const selectHost = (s: AppState, id: HostId): HostView | undefined => (s.hosts[id] ? hostView(s.hosts[id], s.hub, fleetLive(s)) : undefined);

/** Hosts whose data is last-known (see `hostStale`), as one comparable string for `useAppState`. */
export const selectStaleHosts = (s: AppState): string => s.hostOrder.filter((id) => hostStale(s, id)).join("|");
export const sessionsOnHost = (s: AppState, id: HostId): SessionKey[] => Object.keys(s.sessions).filter((k) => k.startsWith(id + "/"));

/** The hosts' held links (see `heldLink`), in fleet order; none while the fleet is not live (every link is then the last known, not something to act on). */
function heldHosts(s: AppState): { id: HostId; held: HeldLink }[] {
  if (!fleetLive(s)) return [];
  const out: { id: HostId; held: HeldLink }[] = [];
  for (const id of s.hostOrder) {
    const h = s.hosts[id];
    const held = h && h.link.link === "remote" ? heldLink(h.link.state) : null;
    if (held) out.push({ id, held });
  }
  return out;
}

/** Hosts waiting for the user to sign in, in fleet order. */
export const selectWaitingHosts = (s: AppState): WaitingHost[] =>
  heldHosts(s).map(({ id, held }) => ({
    id,
    name: s.hosts[id].entry.name,
    ...(held.cause ? { cause: held.cause } : {}),
    detail: held.detail,
    sessions: Object.keys(s.sessions).filter((k) => k.startsWith(`${id}/`)).length,
  }));

/**
 * Every question waiting for the user, oldest first. The daemon numbers prompts in the order it
 * raises them (across hosts), so the order is stable: an answer or a new question never reorders
 * the ones already waiting.
 */
export function selectSignInQueue(s: AppState): SignInQuestion[] {
  const out: SignInQuestion[] = [];
  for (const id of s.hostOrder) {
    const h = s.hosts[id];
    if (h?.authPrompt) out.push({ prompt: h.authPrompt, hostName: h.entry.name });
  }
  return out.sort((a, b) => a.prompt.id - b.prompt.id);
}
