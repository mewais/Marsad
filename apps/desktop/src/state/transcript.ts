import type { Api } from "../bridge/api";
import { errorMessage } from "../bridge/api";
import type { SessionEvent, SessionKey } from "../bridge/wire";
import { splitKey } from "../bridge/wire";
import type { Feeds } from "./feeds";
import type { AppStore, TranscriptSlice } from "./model";
import { admits, MAIN, scopeKey, type TranscriptScope } from "./scope";

const PAGE = 60;

const lastCursor = (events: readonly SessionEvent[]): number => events[events.length - 1]?.cursor ?? -1;

/**
 * Per-session transcripts are loaded lazily when a session view opens: the latest page of history,
 * then a live subscription from its last cursor. Older pages load on demand.
 *
 * A session has one live subscription (`session.events:<id>`), which carries everything the agent does,
 * including what its subagents do. Each open transcript takes the events that are its own (`admits`):
 * the session's own conversation, and one per subagent that is open. A subagent's transcript is loaded
 * with the daemon's scoped history and kept only while it is open.
 */
export function createTranscripts(store: AppStore, api: Api, feeds: Feeds) {
  /** The one subscription of a session, shared by every transcript of it that is open. */
  interface Feed {
    n: number;
    off: () => void;
    /** The newest cursor the subscription delivered (where it resumes if it ends). */
    last: number;
    /** Resolves when the subscription is running (true), or when it will not (false). Scoped history is read after it. */
    started: Promise<boolean>;
  }
  /** A subagent's transcript that is open. `pending`: its history is being fetched, live events wait here. `failed`: it could not be loaded. */
  interface Sub {
    key: SessionKey;
    scope: TranscriptScope;
    n: number;
    pending: SessionEvent[] | null;
    failed: boolean;
  }
  const live = new Map<SessionKey, Feed>();
  const subs = new Map<string, Sub>();
  /** An older page being fetched, so a second request for the same page waits for it instead of returning early. */
  const earlier = new Map<string, Promise<void>>();

  const blank: TranscriptSlice = { events: [], hasMore: false, loading: false };
  const patch = (tk: string, p: Partial<TranscriptSlice>) =>
    store.set((s) => ({ ...s, transcripts: { ...s.transcripts, [tk]: { ...blank, ...(s.transcripts[tk] as TranscriptSlice | undefined), ...p } } }));

  const append = (tk: string, ev: SessionEvent) =>
    store.set((s) => {
      const cur = s.transcripts[tk];
      if (!cur || ev.cursor <= lastCursor(cur.events)) return s;
      return { ...s, transcripts: { ...s.transcripts, [tk]: { ...cur, events: [...cur.events, ev] } } };
    });

  /** The latest page of `scope`'s history; the cursor it ends at, or null when it failed. */
  async function load(key: SessionKey, scope: TranscriptScope, tk = scopeKey(key, scope)) {
    const { host, id } = splitKey(key);
    patch(tk, { loading: true, error: undefined });
    try {
      const page = await api.history(host, { id, limit: PAGE, scope });
      patch(tk, { events: page.events, hasMore: page.has_more, loading: false });
      return lastCursor(page.events);
    } catch (e) {
      patch(tk, { loading: false, error: errorMessage(e) });
      return null;
    }
  }

  /** One event of the session's subscription: to the transcripts it belongs to. */
  function deliver(key: SessionKey, feed: Feed, ev: SessionEvent) {
    feed.last = Math.max(feed.last, ev.cursor);
    if (admits(MAIN, ev)) append(key, ev);
    for (const [tk, sub] of subs) {
      if (sub.key !== key || !admits(sub.scope, ev)) continue;
      if (sub.pending) sub.pending.push(ev);
      else if (!sub.failed) append(tk, ev);
    }
  }

  function releaseFeed(key: SessionKey) {
    const e = live.get(key);
    if (!e) return;
    if (--e.n <= 0) {
      e.off();
      live.delete(key);
    }
  }

  function acquireFeed(key: SessionKey): () => void {
    const cur = live.get(key);
    if (cur) {
      cur.n++;
      return () => releaseFeed(key);
    }
    let started!: (ok: boolean) => void;
    const entry: Feed = { n: 1, off: () => undefined, last: 0, started: new Promise<boolean>((r) => (started = r)) };
    live.set(key, entry);
    const { host, id } = splitKey(key);
    const existing = store.get().transcripts[key];
    void (async () => {
      const from = existing?.events.length ? lastCursor(existing.events) : await load(key, MAIN);
      if (from == null || live.get(key) !== entry) return started(false);
      entry.last = from;
      // Started again (if it ends) from wherever the subscription has got to.
      entry.off = feeds.open(`events:${key}`, (onEnd) => api.subscribe.sessionEvents(host, id, entry.last, (e) => deliver(key, entry, e), onEnd));
      started(true);
    })();
    return () => releaseFeed(key);
  }

  /**
   * A subagent's transcript: its scoped history, read once the subscription is running (so nothing it did in between is lost),
   * and whatever the subscription delivers for it while that is being fetched.
   */
  function acquireSub(key: SessionKey, scope: TranscriptScope): () => void {
    const tk = scopeKey(key, scope);
    const cur = subs.get(tk);
    if (cur) {
      cur.n++;
      return () => releaseSub(tk);
    }
    const sub: Sub = { key, scope, n: 1, pending: [], failed: false };
    subs.set(tk, sub);
    patch(tk, { events: [], hasMore: false, loading: true, error: undefined });
    void (async () => {
      const feed = live.get(key);
      const running = feed ? await feed.started : false;
      if (subs.get(tk) !== sub) return;
      const upTo = running ? await load(key, scope, tk) : null;
      if (subs.get(tk) !== sub) return;
      if (upTo === null) {
        sub.failed = true;
        sub.pending = null;
        if (!running) patch(tk, { loading: false, error: "The session's events could not be read." });
        return;
      }
      const waiting = sub.pending ?? [];
      sub.pending = null;
      for (const ev of waiting) append(tk, ev);
    })();
    return () => releaseSub(tk);
  }

  function releaseSub(tk: string) {
    const sub = subs.get(tk);
    if (!sub || --sub.n > 0) return;
    subs.delete(tk);
    earlier.delete(tk);
    store.set((s) => {
      if (!(tk in s.transcripts)) return s;
      const transcripts = { ...s.transcripts };
      delete transcripts[tk];
      return { ...s, transcripts };
    });
  }

  return {
    /** Ref-counted: returns a release function. The session's own transcript, or with `scope` a subagent's (which keeps the session's subscription open too). */
    open(key: SessionKey, scope: TranscriptScope = MAIN): () => void {
      const releaseFeedRef = acquireFeed(key);
      const releaseScope = scope.kind === "main" ? () => undefined : acquireSub(key, scope);
      return () => {
        releaseScope();
        releaseFeedRef();
      };
    },
    /** Fetch the page before the oldest loaded event; resolves when it is in the store (or failed: see `error`). */
    loadEarlier(key: SessionKey, scope: TranscriptScope = MAIN): Promise<void> {
      const tk = scopeKey(key, scope);
      const pending = earlier.get(tk);
      if (pending) return pending;
      const { host, id } = splitKey(key);
      const cur = store.get().transcripts[tk];
      if (!cur || cur.loading || !cur.hasMore || !cur.events.length) return Promise.resolve();
      patch(tk, { loading: true });
      const p = (async () => {
        try {
          const page = await api.history(host, { id, before: cur.events[0].cursor, limit: PAGE, scope });
          store.set((s) => {
            const t = s.transcripts[tk];
            // A subagent's transcript that was closed meanwhile is gone.
            if (!t) return s;
            return { ...s, transcripts: { ...s.transcripts, [tk]: { ...t, events: [...page.events, ...t.events], hasMore: page.has_more, loading: false } } };
          });
        } catch (e) {
          if (store.get().transcripts[tk]) patch(tk, { loading: false, error: errorMessage(e) });
        } finally {
          earlier.delete(tk);
        }
      })();
      earlier.set(tk, p);
      return p;
    },
  };
}
export type Transcripts = ReturnType<typeof createTranscripts>;
