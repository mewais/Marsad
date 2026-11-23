import { useEffect, useRef, useState } from "react";
import type { HostId, SessionKey } from "../../bridge/wire";
import { clockFailure, measureClock, type ClockState } from "./clock";
import { useActions, useApi, useApp, useAppState, usePending, type TailSlice, type TailSpec, type TrailSlice } from "../../state";

/**
 * A session's trail, held open while the calling panel is mounted and `enabled` (the host can be
 * reached): the last release closes the subscription and forgets the data. `feedError`: why the
 * live subscription ended, when it did.
 */
export function useTrail(key: SessionKey, enabled: boolean): { slice: TrailSlice | undefined; feedError: string | undefined; retry: () => void; loadEarlier: () => void } {
  const { trails } = useApp();
  useEffect(() => (enabled ? trails.open(key) : undefined), [trails, key, enabled]);
  const slice = useAppState((s) => s.trails[key]);
  const feedError = useAppState((s) => s.feedErrors[`trail:${key}`]);
  return { slice, feedError, retry: () => trails.retry(key), loadEarlier: () => void trails.loadEarlier(key) };
}

/**
 * A daemon's log tail, held open while the calling panel is mounted and `spec` is given (a spec's
 * `id` names what is asked for: a new id is a new subscription, the old one is closed).
 */
export function useTail(spec: TailSpec | null): { slice: TailSlice | undefined; feedError: string | undefined; retry: () => void } {
  const { tails } = useApp();
  const id = spec?.id;
  const latest = useRef(spec);
  latest.current = spec;
  useEffect(() => (latest.current ? tails.open(latest.current) : undefined), [tails, id]);
  const slice = useAppState((s) => (id ? s.tails[id] : undefined));
  const feedError = useAppState((s) => (id ? s.feedErrors[`tail:${id}`] : undefined));
  return { slice, feedError, retry: tails.retry };
}

/**
 * The way to ask again when a panel cannot reach what it reads: connect to the local service
 * again when the app has no link to it, else connect to `host` again when it is not connected;
 * `null` when there is nothing to reconnect. The panel then reads again by itself, as soon as the
 * link is up (it holds nothing open meanwhile, and polls nothing).
 */
export function useReconnect(host?: HostId): { label: string; run: () => void; pending: boolean } | null {
  const actions = useActions();
  const api = useApi();
  const hub = useAppState((s) => s.hub.state);
  const hostName = useAppState((s) => (host ? s.hosts[host]?.entry.name : undefined));
  const hostDown = useAppState((s) => {
    const h = host ? s.hosts[host] : undefined;
    return !!h && h.link.link === "remote" && h.link.state.state !== "connected";
  });
  const pk = `reconnect:${host ?? ""}`;
  const busy = !!usePending(pk);
  if (hub !== "connected") return hub === "connecting" ? { label: "Reconnect", run: () => undefined, pending: true } : { label: "Reconnect to the local service", run: () => void actions.reconnectHub(), pending: false };
  if (host && hostDown) return { label: `Reconnect to ${hostName ?? host}`, run: () => void actions.run(pk, `Could not connect to ${hostName ?? host}`, () => api.hosts.reconnect(host)), pending: busy };
  return null;
}

/**
 * The offset of `host`'s wall clock from this computer's, measured once when the panel opens (and
 * again only if the host becomes readable again after it was not): `null` while `enabled` is false.
 * Nothing is polled; the times the panel shows are corrected by it and stay marked approximate.
 */
export function useClockOffset(host: HostId, enabled: boolean): ClockState | null {
  const api = useApi();
  const [state, setState] = useState<ClockState | null>(null);
  useEffect(() => {
    if (!enabled) return setState(null);
    let live = true;
    setState({ state: "measuring" });
    measureClock(api, host).then(
      (offset) => live && setState({ state: "ready", offset }),
      (e) => live && setState(clockFailure(e)),
    );
    return () => {
      live = false;
    };
  }, [api, host, enabled]);
  return state;
}
