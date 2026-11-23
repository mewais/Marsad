import type { Api } from "../bridge/api";
import { errorMessage } from "../bridge/api";
import type { SessionEvent, SessionKey } from "../bridge/wire";
import { splitKey } from "../bridge/wire";
import type { Feeds } from "./feeds";
import type { AppStore, EventList, TranscriptSlice } from "./model";
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
 *
 * A transcript of a resumed conversation has a second list, `earlier`: the conversation's history before this session
 * (`session.history {earlier: true}`), shown above the session's own events and never merged with them. It is wanted once
 * the session's own events are all loaded, and it follows what the daemon reports of its loading (`snapshot.earlier`):
 * each time the count of history items changes, the newest items are fetched and joined to what is held. The live stream
 * never carries history items; it only says that the loading moved, and so does the snapshot, which is what is followed.
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
  const paging = new Map<string, Promise<void>>();
  /** The earlier list's work, one job at a time per transcript (fetching the newest items, paging older ones), so they never interleave. */
  const jobs = new Map<string, Promise<void>>();
  /** Transcripts with a fetch of the newest earlier items queued and not yet started (it reads the daemon's progress when it starts). */
  const waiting = new Set<string>();
  /** How many history items the daemon reported when each transcript's earlier list was last fetched: asked again only when that moves. */
  const asked = new Map<string, number>();

  const none: EventList = { events: [], hasMore: false, loading: false };
  const blank: TranscriptSlice = { ...none, earlier: none };
  const patch = (tk: string, p: Partial<TranscriptSlice>) =>
    store.set((s) => ({ ...s, transcripts: { ...s.transcripts, [tk]: { ...blank, ...(s.transcripts[tk] as TranscriptSlice | undefined), ...p } } }));
  /** Change the earlier list of a transcript that exists (one that was closed meanwhile stays gone). */
  const patchEarlier = (tk: string, p: Partial<EventList> | ((cur: EventList) => Partial<EventList>)) =>
    store.set((s) => {
      const t = s.transcripts[tk];
      if (!t) return s;
      return { ...s, transcripts: { ...s.transcripts, [tk]: { ...t, earlier: { ...t.earlier, ...(typeof p === "function" ? p(t.earlier) : p) } } } };
    });

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

  /** Run `job` after the earlier-list work already queued for `tk`. Jobs report their own failures; none throws. */
  function serial(tk: string, job: () => Promise<void>): Promise<void> {
    const next = (jobs.get(tk) ?? Promise.resolve()).then(job);
    jobs.set(tk, next);
    void next.then(() => {
      if (jobs.get(tk) === next) jobs.delete(tk);
    });
    return next;
  }

  /**
   * The newest page of the earlier history, joined to what is held. The newest items are the ones that arrive while it loads, so
   * what is held ends where the new items begin: pages are read back from the newest until they reach it. (A page that does not
   * reach it, because more arrived than a page holds, is followed by the one before it.)
   */
  async function fetchNewest(tk: string, key: SessionKey, scope: TranscriptScope) {
    waiting.delete(tk);
    const t = store.get().transcripts[tk];
    if (!t) return;
    asked.set(tk, store.get().sessions[key]?.earlier.items ?? 0);
    const { host, id } = splitKey(key);
    patchEarlier(tk, { loading: true, error: undefined });
    try {
      const held = t.earlier.events;
      const heldLast = held.length ? held[held.length - 1].cursor : -1;
      const fresh: SessionEvent[] = [];
      let before: number | undefined;
      let hasMore = false;
      for (;;) {
        const page = await api.history(host, { id, before, limit: PAGE, scope, earlier: true });
        fresh.unshift(...page.events);
        hasMore = page.has_more;
        const joined = heldLast < 0 || (page.events.length > 0 && page.events[0].cursor <= heldLast);
        if (joined || !page.has_more || !page.events.length) break;
        before = page.events[0].cursor;
      }
      patchEarlier(tk, (cur) => {
        const last = cur.events.length ? cur.events[cur.events.length - 1].cursor : -1;
        return cur.events.length ? { events: [...cur.events, ...fresh.filter((e) => e.cursor > last)], loading: false } : { events: fresh, hasMore, loading: false };
      });
    } catch (e) {
      patchEarlier(tk, { loading: false, error: errorMessage(e) });
    }
  }

  /** The page of earlier history before the oldest item held. */
  async function pageOlder(tk: string, key: SessionKey, scope: TranscriptScope) {
    const cur = store.get().transcripts[tk]?.earlier;
    if (!cur || !cur.hasMore || !cur.events.length) return;
    const { host, id } = splitKey(key);
    patchEarlier(tk, { loading: true, error: undefined });
    try {
      const page = await api.history(host, { id, before: cur.events[0].cursor, limit: PAGE, scope, earlier: true });
      patchEarlier(tk, (c) => ({ events: [...page.events, ...c.events], hasMore: page.has_more, loading: false }));
    } catch (e) {
      patchEarlier(tk, { loading: false, error: errorMessage(e) });
    }
  }

  /**
   * Bring the earlier list of an open transcript up to what the daemon says it holds: once the session's own events are all here, and
   * whenever the daemon's count of history items is not the one it was last fetched at. Driven by the daemon's progress, never by time.
   * `force`: ask again although the count has not moved (the reader asked, after a failure). Resolves when the fetch it started is done.
   */
  function reconcile(tk: string, key: SessionKey, scope: TranscriptScope, force = false): Promise<void> {
    const s = store.get();
    const t = s.transcripts[tk];
    const e = s.sessions[key]?.earlier;
    if (!t || t.loading || t.hasMore || !e || e.state.kind === "none") return Promise.resolve();
    if (waiting.has(tk)) return jobs.get(tk) ?? Promise.resolve();
    if (!force && asked.get(tk) === e.items) return Promise.resolve();
    // Nothing has arrived yet: nothing to ask for. (It is asked again when something does.)
    if (e.items === 0) {
      asked.set(tk, 0);
      return Promise.resolve();
    }
    waiting.add(tk);
    return serial(tk, () => fetchNewest(tk, key, scope));
  }

  /** Every transcript that is open: the session's own, and each subagent's. */
  function openTranscripts(): [string, SessionKey, TranscriptScope][] {
    return [...[...live.keys()].map((k): [string, SessionKey, TranscriptScope] => [k, k, MAIN]), ...[...subs].filter(([, x]) => !x.failed && !x.pending).map(([tk, x]): [string, SessionKey, TranscriptScope] => [tk, x.key, x.scope])];
  }
  // The daemon's progress reaches the app as the session's snapshot: an open transcript follows it.
  store.subscribe(() => openTranscripts().forEach(([tk, key, scope]) => void reconcile(tk, key, scope)));

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
      void reconcile(key, key, MAIN);
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
      const queued = sub.pending ?? [];
      sub.pending = null;
      for (const ev of queued) append(tk, ev);
      void reconcile(tk, key, scope);
    })();
    return () => releaseSub(tk);
  }

  function releaseSub(tk: string) {
    const sub = subs.get(tk);
    if (!sub || --sub.n > 0) return;
    subs.delete(tk);
    paging.delete(tk);
    asked.delete(tk);
    waiting.delete(tk);
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
    /**
     * Fetch the page before the oldest event shown, resolving when it is in the store (or failed: see `error`). One control, one
     * direction: up through the session's own events, then on into the conversation's earlier history, then up through that.
     */
    loadEarlier(key: SessionKey, scope: TranscriptScope = MAIN): Promise<void> {
      const tk = scopeKey(key, scope);
      const pending = paging.get(tk);
      if (pending) return pending;
      const { host, id } = splitKey(key);
      const cur = store.get().transcripts[tk];
      if (!cur || cur.loading) return Promise.resolve();
      let p: Promise<void>;
      if (cur.hasMore && cur.events.length) {
        patch(tk, { loading: true });
        p = (async () => {
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
          }
          // The session's own events are all here now: what comes next is the earlier history.
          await reconcile(tk, key, scope);
        })();
      } else if (cur.earlier.hasMore) p = serial(tk, () => pageOlder(tk, key, scope));
      else if (cur.earlier.error) p = reconcile(tk, key, scope, true);
      else return Promise.resolve();
      paging.set(tk, p);
      void p.then(() => paging.delete(tk));
      return p;
    },
  };
}
export type Transcripts = ReturnType<typeof createTranscripts>;
