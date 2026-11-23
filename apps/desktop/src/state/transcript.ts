import type { Api } from "../bridge/api";
import { errorMessage } from "../bridge/api";
import type { SessionEvent, SessionKey } from "../bridge/wire";
import { splitKey } from "../bridge/wire";
import type { Feeds } from "./feeds";
import type { AppStore, TranscriptSlice } from "./model";

const PAGE = 60;

/**
 * Per-session transcripts are loaded lazily when a session view opens: the latest page of history,
 * then a live subscription from its last cursor. Older pages load on demand.
 */
export function createTranscripts(store: AppStore, api: Api, feeds: Feeds) {
  const live = new Map<SessionKey, { n: number; off: () => void }>();
  /** An older page being fetched, so a second request for the same page waits for it instead of returning early. */
  const earlier = new Map<SessionKey, Promise<void>>();

  const blank: TranscriptSlice = { events: [], hasMore: false, loading: false };
  const patch = (key: SessionKey, p: Partial<TranscriptSlice>) =>
    store.set((s) => ({ ...s, transcripts: { ...s.transcripts, [key]: { ...blank, ...(s.transcripts[key] as TranscriptSlice | undefined), ...p } } }));

  const append = (key: SessionKey, ev: SessionEvent) =>
    store.set((s) => {
      const cur = s.transcripts[key];
      if (!cur) return s;
      const last = cur.events[cur.events.length - 1]?.cursor ?? -1;
      if (ev.cursor <= last) return s;
      return { ...s, transcripts: { ...s.transcripts, [key]: { ...cur, events: [...cur.events, ev] } } };
    });

  async function load(key: SessionKey) {
    const { host, id } = splitKey(key);
    patch(key, { loading: true, error: undefined });
    try {
      const page = await api.history(host, { id, limit: PAGE });
      patch(key, { events: page.events, hasMore: page.has_more, loading: false });
      return page.events[page.events.length - 1]?.cursor ?? 0;
    } catch (e) {
      patch(key, { loading: false, error: errorMessage(e) });
      return null;
    }
  }

  function release(key: SessionKey) {
    const e = live.get(key);
    if (!e) return;
    if (--e.n <= 0) {
      e.off();
      live.delete(key);
    }
  }

  return {
    /** Ref-counted: returns a release function. */
    open(key: SessionKey): () => void {
      const cur = live.get(key);
      if (cur) {
        cur.n++;
        return () => release(key);
      }
      const entry = { n: 1, off: () => undefined as void };
      live.set(key, entry);
      const { host, id } = splitKey(key);
      const existing = store.get().transcripts[key];
      void (async () => {
        const from = existing?.events.length ? existing.events[existing.events.length - 1].cursor : await load(key);
        if (from == null || live.get(key) !== entry) return;
        // Started again (if it ends) from wherever the transcript has got to.
        entry.off = feeds.open(`events:${key}`, (onEnd) => {
          const have = store.get().transcripts[key]?.events;
          return api.subscribe.sessionEvents(host, id, have?.length ? have[have.length - 1].cursor : from, (e) => append(key, e), onEnd);
        });
      })();
      return () => release(key);
    },
    release,
    /** Fetch the page before the oldest loaded event; resolves when it is in the store (or failed: see `error`). */
    loadEarlier(key: SessionKey): Promise<void> {
      const pending = earlier.get(key);
      if (pending) return pending;
      const { host, id } = splitKey(key);
      const cur = store.get().transcripts[key];
      if (!cur || cur.loading || !cur.hasMore || !cur.events.length) return Promise.resolve();
      patch(key, { loading: true });
      const p = (async () => {
        try {
          const page = await api.history(host, { id, before: cur.events[0].cursor, limit: PAGE });
          store.set((s) => {
            const t = s.transcripts[key];
            return { ...s, transcripts: { ...s.transcripts, [key]: { ...t, events: [...page.events, ...t.events], hasMore: page.has_more, loading: false } } };
          });
        } catch (e) {
          patch(key, { loading: false, error: errorMessage(e) });
        } finally {
          earlier.delete(key);
        }
      })();
      earlier.set(key, p);
      return p;
    },
  };
}
export type Transcripts = ReturnType<typeof createTranscripts>;
