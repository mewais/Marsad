import type { HostId, SessionKey } from "../bridge/wire";
import { sessionKey, splitKey } from "../bridge/wire";
import type { WorkstreamView } from "../bridge/wire";
import { STATUS_ORDER, type StatusKey } from "../ui/status";
import { buildView, type SessionView } from "./domain";
import { hostView, type HostView } from "./hosts";
import type { AppState } from "./model";

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

export function selectView(s: AppState, key: SessionKey): SessionView | undefined {
  const snap = s.sessions[key];
  const host = splitKey(key).host;
  return snap ? buildView(host, snap, s.seen[key], snap.cwd ? s.git[host]?.[snap.cwd] : undefined, workstreamIndex(s.workstreams).get(key) ?? "") : undefined;
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
  const out: Groups = { needs: [], failed: [], done: [], working: [], idle: [] };
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

export interface Counts {
  needs: number;
  failed: number;
  done: number;
  working: number;
  idle: number;
  total: number;
  todayCost: number;
  todaySessions: number;
}

export function selectCounts(s: AppState): Counts {
  const c: Counts = { needs: 0, failed: 0, done: 0, working: 0, idle: 0, total: 0, todayCost: 0, todaySessions: 0 };
  const midnight = new Date();
  midnight.setHours(0, 0, 0, 0);
  for (const key of Object.keys(s.sessions)) {
    const v = selectView(s, key)!;
    c[v.status]++;
    c.total++;
    if (v.lastMs >= midnight.getTime()) {
      c.todayCost += v.cost;
      c.todaySessions++;
    }
  }
  c.todayCost = Math.round(c.todayCost * 100) / 100;
  return c;
}

export const selectHosts = (s: AppState): HostView[] => s.hostOrder.filter((id) => s.hosts[id]).map((id) => hostView(s.hosts[id]));
export const selectHost = (s: AppState, id: HostId): HostView | undefined => (s.hosts[id] ? hostView(s.hosts[id]) : undefined);
export const sessionsOnHost = (s: AppState, id: HostId): SessionKey[] => Object.keys(s.sessions).filter((k) => k.startsWith(id + "/"));
