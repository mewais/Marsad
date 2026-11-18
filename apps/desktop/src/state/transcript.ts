import type { Backend } from "../bridge/backend";
import type { HistoryPage, SessionEvent, SessionKey } from "../bridge/types";
import { methods, splitKey, topics } from "../bridge/types";
import type { AppStore, TranscriptSlice } from "./model";

const PAGE = 60;

/**
 * Per-session transcripts are loaded lazily when a session view opens: the latest page of history,
 * then a live subscription from its last cursor. Older pages load on demand.
 */
export function createTranscripts(store: AppStore, backend: Backend) {
  const live = new Map<SessionKey, { n: number; off: () => void }>();

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
      const page = await backend.request<HistoryPage>(methods.SESSION_HISTORY, { id, limit: PAGE }, { host });
      patch(key, { events: page.events, hasMore: page.has_more, loading: false });
      return page.events[page.events.length - 1]?.cursor ?? 0;
    } catch (e) {
      patch(key, { loading: false, error: (e as { message?: string }).message ?? "Could not load history" });
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
        entry.off = backend.subscribe(topics.sessionEvents(id), { host, from }, (e) => append(key, e as SessionEvent));
      })();
      return () => release(key);
    },
    release,
    async loadEarlier(key: SessionKey) {
      const { host, id } = splitKey(key);
      const cur = store.get().transcripts[key];
      if (!cur || cur.loading || !cur.hasMore || !cur.events.length) return;
      patch(key, { loading: true });
      try {
        const page = await backend.request<HistoryPage>(methods.SESSION_HISTORY, { id, before: cur.events[0].cursor, limit: PAGE }, { host });
        store.set((s) => {
          const t = s.transcripts[key];
          return { ...s, transcripts: { ...s.transcripts, [key]: { ...t, events: [...page.events, ...t.events], hasMore: page.has_more, loading: false } } };
        });
      } catch (e) {
        patch(key, { loading: false, error: (e as { message?: string }).message });
      }
    },
  };
}
export type Transcripts = ReturnType<typeof createTranscripts>;
