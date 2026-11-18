import type { Backend } from "../bridge/backend";
import type { HostEvent, HostId, HostInfo, SessionSnapshot } from "../bridge/types";
import { methods, sessionKey, topics } from "../bridge/types";
import { makeDriver } from "../ui/DriverBadge";
import type { AppState, AppStore } from "./model";

/**
 * Wires a Backend into the store: connection state, per-host `host.sessions` subscriptions
 * (reset / upsert / removed) and `host.info` (drivers). Returns a disposer.
 */
export function connectStore(store: AppStore, backend: Backend): () => void {
  const hostSubs = new Map<HostId, () => void>();
  const offs: (() => void)[] = [];

  const applyHostEvent = (host: HostId, e: HostEvent) => {
    store.set((s) => {
      const sessions = { ...s.sessions };
      if (e.kind === "reset") {
        for (const k of Object.keys(sessions)) if (k.startsWith(host + "/")) delete sessions[k];
        for (const snap of e.sessions) sessions[sessionKey(host, snap.id)] = snap;
      } else if (e.kind === "upsert") {
        sessions[sessionKey(host, e.session.id)] = e.session;
      } else {
        delete sessions[sessionKey(host, e.id)];
      }
      return { ...s, sessions, hostLoaded: { ...s.hostLoaded, [host]: true }, pending: prunePending(s.pending, sessions) };
    });
  };

  const learnDrivers = (host: HostId, info: HostInfo) => {
    store.set((s) => {
      const drivers = { ...s.drivers };
      const order = [...s.driverOrder];
      for (const d of info.drivers) {
        if (!drivers[d.id]) {
          drivers[d.id] = makeDriver(d.id, d.display_name, order.length);
          order.push(d.id);
        }
      }
      return { ...s, drivers, driverOrder: order, hostInfo: { ...s.hostInfo, [host]: info } };
    });
  };

  const syncHosts = (links: AppState["links"], order: HostId[]) => {
    store.set((s) => ({ ...s, links, hostOrder: order }));
    for (const id of order) {
      if (hostSubs.has(id)) continue;
      hostSubs.set(id, backend.subscribe(topics.HOST_SESSIONS, { host: id }, (e) => applyHostEvent(id, e as HostEvent)));
      // Drivers are known from host.info; a host that is down is retried when it connects.
      void backend.request<HostInfo>(methods.HOST_INFO, undefined, { host: id }).then((i) => learnDrivers(id, i), () => hostSubs.get(id) && retryInfo(id));
    }
  };

  const retried = new Set<HostId>();
  const retryInfo = (id: HostId) => retried.add(id);

  const onConn = (c: { hosts: { id: HostId }[] }) => {
    const links: AppState["links"] = {};
    for (const h of c.hosts) links[h.id] = h as AppState["links"][string];
    const prev = store.get().links;
    syncHosts(links, c.hosts.map((h) => h.id));
    for (const id of retried) {
      if (links[id]?.link === "connected" && prev[id]?.link !== "connected") {
        retried.delete(id);
        void backend.request<HostInfo>(methods.HOST_INFO, undefined, { host: id }).then((i) => learnDrivers(id, i), () => retried.add(id));
      }
    }
  };

  onConn(backend.connection.get());
  offs.push(backend.connection.subscribe(onConn));
  return () => {
    offs.forEach((o) => o());
    hostSubs.forEach((u) => u());
    hostSubs.clear();
  };
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
