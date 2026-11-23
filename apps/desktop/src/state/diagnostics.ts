import type { Api } from "../bridge/api";
import { errorMessage } from "../bridge/api";
import type { DiagTailEvent, SessionKey, TrailItem } from "../bridge/wire";
import { splitKey } from "../bridge/wire";
import { keeps, matchesFilter, parseRecord, type DiagRecord } from "../lib/diagRecord";
import type { Feeds } from "./feeds";
import { TAIL_KEEP, type AppStore, type TailSlice, type TailSpec, type TrailSlice } from "./model";

/** A release that can be called twice without releasing someone else's hold. */
const once = (fn: () => void) => {
  let done = false;
  return () => {
    if (!done) {
      done = true;
      fn();
    }
  };
};

/** Entries of a trail fetched at once (the newest; older ones on request). */
const TRAIL_PAGE = 200;

/**
 * Session trails for the Developer-mode panel: the newest page of the trail from the host, then
 * the live `session.trail:<id>` subscription from its last cursor (resumed by the feed tracking if
 * it ends). Held only while a panel has it open (ref-counted): the last release closes the
 * subscription and forgets the slice, so a closed panel costs nothing on any machine.
 */
export function createTrails(store: AppStore, api: Api, feeds: Feeds) {
  interface Open {
    n: number;
    off: () => void;
  }
  const live = new Map<SessionKey, Open>();
  const blank: TrailSlice = { items: [], hasMore: false, loading: false };
  const patch = (key: SessionKey, p: Partial<TrailSlice>) => store.set((s) => ({ ...s, trails: { ...s.trails, [key]: { ...blank, ...s.trails[key], ...p } } }));
  const append = (key: SessionKey, item: TrailItem) =>
    store.set((s) => {
      const cur = s.trails[key];
      if (!cur || item.cursor <= (cur.items[cur.items.length - 1]?.cursor ?? -1)) return s;
      return { ...s, trails: { ...s.trails, [key]: { ...cur, items: [...cur.items, item] } } };
    });

  /** Load the newest page, then follow it live. Does nothing once `entry` has been released. */
  async function start(key: SessionKey, entry: Open) {
    const { host, id } = splitKey(key);
    patch(key, { loading: true, error: undefined });
    try {
      const page = await api.trailPage(host, { id, limit: TRAIL_PAGE });
      if (live.get(key) !== entry) return;
      patch(key, { items: page.entries, hasMore: page.has_more, loading: false });
    } catch (e) {
      if (live.get(key) === entry) patch(key, { loading: false, error: errorMessage(e) });
      return;
    }
    entry.off = feeds.open(`trail:${key}`, (onEnd) => {
      const items = store.get().trails[key]?.items;
      return api.subscribe.sessionTrail(host, id, items?.length ? items[items.length - 1].cursor : 0, (e) => append(key, e), onEnd);
    });
  }

  function release(key: SessionKey, entry: Open) {
    if (live.get(key) !== entry || --entry.n > 0) return;
    entry.off();
    live.delete(key);
    store.set((s) => {
      const trails = { ...s.trails };
      delete trails[key];
      return { ...s, trails };
    });
  }

  return {
    /** Ref-counted: returns what releases it. */
    open(key: SessionKey): () => void {
      const cur = live.get(key);
      if (cur) {
        cur.n++;
        return once(() => release(key, cur));
      }
      const entry: Open = { n: 1, off: () => undefined };
      live.set(key, entry);
      void start(key, entry);
      return once(() => release(key, entry));
    },
    /** After a failure: fetch the page again if it did not load, and start the subscriptions that ended again. */
    retry(key: SessionKey) {
      const entry = live.get(key);
      if (entry && store.get().trails[key]?.error) void start(key, entry);
      feeds.retry();
    },
    /** Fetch the page before the oldest loaded entry; resolves when it is in the store (or failed: see `error`). */
    async loadEarlier(key: SessionKey): Promise<void> {
      const cur = store.get().trails[key];
      if (!cur || cur.loading || !cur.hasMore || !cur.items.length) return;
      const { host, id } = splitKey(key);
      patch(key, { loading: true });
      try {
        const page = await api.trailPage(host, { id, before: cur.items[0].cursor, limit: TRAIL_PAGE });
        store.set((s) => {
          const t = s.trails[key];
          return t ? { ...s, trails: { ...s.trails, [key]: { ...t, items: [...page.entries, ...t.items], hasMore: page.has_more, loading: false } } } : s;
        });
      } catch (e) {
        if (store.get().trails[key]) patch(key, { loading: false, error: errorMessage(e) });
      }
    },
  };
}
export type Trails = ReturnType<typeof createTrails>;

/**
 * Daemon log tails (`diag.tail`): the hub's or a host's recent diagnostics records, live. Each is
 * opened for a panel with a `TailSpec` and held only while that panel is open (ref-counted by
 * `spec.id`, same as trails). What the panel sees is what matches its filter, checked here for
 * every record whatever the transport did with the filter, and at most `TAIL_KEEP` of them.
 */
export function createTails(store: AppStore, api: Api, feeds: Feeds) {
  interface Open {
    n: number;
    off: () => void;
    /** `<boot>.<seq>` of the last record received (matching or not): where a resubscription resumes. */
    cursor?: string;
  }
  const live = new Map<string, Open>();

  const take = (spec: TailSpec, lines: string[], reset: boolean) => {
    const records: DiagRecord[] = [];
    let unreadable = 0;
    for (const line of lines) {
      const r = parseRecord(line);
      if (!r) unreadable++;
      else records.push(r);
    }
    const entry = live.get(spec.id);
    if (entry && records.length) entry.cursor = records[records.length - 1].id;
    const wanted = records.filter((r) => matchesFilter(r, spec.filter) && keeps(r, spec.keep));
    store.set((s) => {
      const cur = s.tails[spec.id];
      if (!cur) return s;
      if (!reset && !wanted.length && !unreadable && cur.state === "live") return s;
      const all = reset ? wanted : [...cur.records, ...wanted];
      const over = Math.max(0, all.length - TAIL_KEEP);
      const next: TailSlice = { state: "live", records: over ? all.slice(over) : all, trimmed: (reset ? 0 : cur.trimmed) + over, unreadable: (reset ? 0 : cur.unreadable) + unreadable };
      return { ...s, tails: { ...s.tails, [spec.id]: next } };
    });
  };
  const apply = (spec: TailSpec, e: DiagTailEvent) => {
    if (e?.kind === "reset") take(spec, e.records, true);
    else if (e?.kind === "record") take(spec, [e.line], false);
  };

  function release(id: string, entry: Open) {
    if (live.get(id) !== entry || --entry.n > 0) return;
    entry.off();
    live.delete(id);
    store.set((s) => {
      const tails = { ...s.tails };
      delete tails[id];
      return { ...s, tails };
    });
  }

  return {
    /** Ref-counted: returns what releases it. */
    open(spec: TailSpec): () => void {
      const cur = live.get(spec.id);
      if (cur) {
        cur.n++;
        return once(() => release(spec.id, cur));
      }
      const entry: Open = { n: 1, off: () => undefined };
      live.set(spec.id, entry);
      store.set((s) => ({ ...s, tails: { ...s.tails, [spec.id]: { state: "waiting", records: [], trimmed: 0, unreadable: 0 } } }));
      entry.off = feeds.open(`tail:${spec.id}`, (onEnd) => api.subscribe.diagTail(spec.host, spec.filter, entry.cursor, (e) => apply(spec, e), onEnd));
      return once(() => release(spec.id, entry));
    },
    /** Start the subscriptions that ended again. */
    retry: feeds.retry,
  };
}
export type Tails = ReturnType<typeof createTails>;
