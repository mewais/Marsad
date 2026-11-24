import type { Api } from "../bridge/api";
import { errorMessage } from "../bridge/api";
import type { SessionEvent, SessionKey } from "../bridge/wire";
import { splitKey } from "../bridge/wire";
import type { Feeds } from "./feeds";
import { hasOlder, type AppStore, type EventList, type TranscriptSlice } from "./model";
import { admits, MAIN, scopeKey, type TranscriptScope } from "./scope";

const PAGE = 60;
/** The first page of a session opened at its snapshot: read beside the subscription, so it is small in bytes as well as events. */
const OPEN_PAGE = 60;
const OPEN_BYTES = 256 * 1024;
/** Every later page of the session's own events (background or requested): big in events, bounded in bytes. */
const LATER_PAGE = 500;
const LATER_BYTES = 1024 * 1024;
/**
 * Events kept resident in a session's own transcript. The background pager stops once it holds this many; live appends trim the
 * oldest back to it once the list exceeds it by `TRIM_SLACK`, unless the reader has paged up (`extended`). Releasing the transcript
 * keeps only the newest `RELEASE_KEEP`; what is trimmed is paged back in from the host when it is asked for.
 */
export const RESIDENT = 4000;
export const TRIM_SLACK = 1000;
export const RELEASE_KEEP = 200;

const lastCursor = (events: readonly SessionEvent[]): number => events[events.length - 1]?.cursor ?? -1;

/** Events in cursor order, each cursor once: a page and what arrived beside it. */
function mergeByCursor(...lists: readonly SessionEvent[][]): SessionEvent[] {
  const all = lists.flat().sort((a, b) => a.cursor - b.cursor);
  return all.filter((e, i) => i === 0 || e.cursor !== all[i - 1].cursor);
}

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
    /** Opened at a snapshot's cursor: the live events that arrive before the first page does, held to be merged with it. Null once it is in. */
    held: SessionEvent[] | null;
    /** Opened at a snapshot's cursor: the first page reads before this cursor (the subscription delivers from it on). Null when opened sequentially. */
    before: number | null;
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
      let events = [...cur.events, ev];
      let hasMore = cur.hasMore;
      // Bounded while live: what is trimmed is older than the resident window, and the host still has it (`hasMore`).
      if (!cur.extended && events.length > RESIDENT + TRIM_SLACK) {
        events = events.slice(events.length - RESIDENT);
        hasMore = true;
      }
      return { ...s, transcripts: { ...s.transcripts, [tk]: { ...cur, events, hasMore } } };
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
    if (admits(MAIN, ev)) {
      if (feed.held) feed.held.push(ev);
      else append(key, ev);
    }
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
      // Closed: the resident window shrinks to the newest few; the rest is paged back in from the host if it is asked for.
      store.set((s) => {
        const t = s.transcripts[key];
        if (!t) return s;
        const trim = t.events.length > RELEASE_KEEP;
        if (!trim && !t.extended) return s;
        const events = trim ? t.events.slice(t.events.length - RELEASE_KEEP) : t.events;
        return { ...s, transcripts: { ...s.transcripts, [key]: { ...t, events, hasMore: t.hasMore || trim, extended: false } } };
      });
    }
  }

  /** Start the session's subscription from `entry.last`; restarted from wherever it has got to if it ends. */
  function subscribe(key: SessionKey, entry: Feed) {
    const { host, id } = splitKey(key);
    // Its host's: started again when that host's link comes back, not only when the hub's does.
    entry.off = feeds.open(`events:${key}`, (onEnd) => api.subscribe.sessionEvents(host, id, entry.last, (e) => deliver(key, entry, e), onEnd), host);
  }

  /**
   * The newest page of a session opened at its snapshot's cursor `c`, read beside the subscription (which was started from `c`, so it
   * delivers what is after it). Live events that arrive before the page does are held, and merged with it by cursor when it arrives.
   * A failed page leaves live events flowing and `hasMore` set, so "Load earlier" is the retry.
   */
  async function openLatest(key: SessionKey, entry: Feed, c: number) {
    const { host, id } = splitKey(key);
    patch(key, { loading: true, error: undefined });
    try {
      const page = await api.history(host, { id, before: c + 1, limit: OPEN_PAGE, scope: MAIN, max_bytes: OPEN_BYTES });
      settleOpen(key, entry, { events: page.events, hasMore: page.has_more });
    } catch (e) {
      settleOpen(key, entry, { hasMore: true, error: errorMessage(e) });
      return;
    }
    if (live.get(key) === entry) await pageInBackground(key, entry);
    if (live.get(key) === entry) void reconcile(key, key, MAIN);
  }

  /** Put the first page in with the live events held meanwhile; from here on live events are appended as they come. */
  function settleOpen(key: SessionKey, entry: Feed, page: { events?: SessionEvent[]; hasMore: boolean; error?: string }) {
    const held = entry.held ?? [];
    entry.held = null;
    store.set((s) => {
      const cur = s.transcripts[key];
      if (!cur) return s;
      const events = mergeByCursor(page.events ?? [], held);
      return { ...s, transcripts: { ...s.transcripts, [key]: { ...cur, events, hasMore: page.hasMore, loading: false, error: page.error } } };
    });
  }

  /**
   * After the first page: older pages of the session's own events, one request at a time, while the host has more and fewer than
   * `RESIDENT` are held. It stops at the first failure (which is left for the reader to retry) and when the feed is closed.
   */
  async function pageInBackground(key: SessionKey, entry: Feed) {
    for (;;) {
      const before = store.get().transcripts[key];
      if (live.get(key) !== entry || !before || !before.hasMore || before.error || before.events.length >= RESIDENT) return;
      // The last page is sized to the room left, so the list lands on RESIDENT exactly rather than a page past it.
      await pageEarlier(key, MAIN, Math.min(LATER_PAGE, RESIDENT - before.events.length));
      const after = store.get().transcripts[key];
      // No progress (nothing came, or the page failed): stop rather than ask again.
      if (!after || after.error || after.events[0]?.cursor === before.events[0]?.cursor) return;
    }
  }

  function acquireFeed(key: SessionKey): () => void {
    const cur = live.get(key);
    if (cur) {
      cur.n++;
      return () => releaseFeed(key);
    }
    let started!: (ok: boolean) => void;
    const entry: Feed = { n: 1, off: () => undefined, last: 0, started: new Promise<boolean>((r) => (started = r)), held: null, before: null };
    live.set(key, entry);
    const snapshot = store.get().sessions[key];
    const existing = store.get().transcripts[key];
    // Snapshot loaded and nothing held: subscribe from its cursor now, and read the newest page beside it (no waiting in between).
    if (snapshot && !existing?.events.length) {
      const c = snapshot.cursor;
      entry.last = c;
      entry.held = [];
      entry.before = c + 1;
      subscribe(key, entry);
      started(true);
      void openLatest(key, entry, c);
      return () => releaseFeed(key);
    }
    // Otherwise sequential: the latest page, then the subscription from its last cursor.
    void (async () => {
      const from = existing?.events.length ? lastCursor(existing.events) : await load(key, MAIN);
      if (from == null || live.get(key) !== entry) return started(false);
      entry.last = from;
      subscribe(key, entry);
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

  /**
   * One step up the transcript: the page before the oldest event held, from the session's own events (`limit` events, `LATER_PAGE` by default); then,
   * once those are all here, the earlier history. Resolves when the step is in the store (or failed: see `error`).
   */
  function pageEarlier(key: SessionKey, scope: TranscriptScope, limit = LATER_PAGE): Promise<void> {
    const tk = scopeKey(key, scope);
    const pending = paging.get(tk);
    if (pending) return pending;
    const { host, id } = splitKey(key);
    const cur = store.get().transcripts[tk];
    if (!cur || cur.loading) return Promise.resolve();
    // Where the first page reads before, while no event is held (a failed first page): the subscription delivers from there on.
    const bound = cur.events.length ? cur.events[0].cursor : scope.kind === "main" ? (live.get(key)?.before ?? null) : null;
    let p: Promise<void>;
    if (cur.hasMore && bound !== null) {
      patch(tk, { loading: true, error: undefined });
      p = (async () => {
        try {
          const page = await api.history(host, { id, before: bound, limit, scope, max_bytes: LATER_BYTES });
          store.set((s) => {
            const t = s.transcripts[tk];
            // A subagent's transcript that was closed meanwhile is gone.
            if (!t) return s;
            return { ...s, transcripts: { ...s.transcripts, [tk]: { ...t, events: mergeByCursor(page.events, t.events), hasMore: page.has_more, loading: false } } };
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
     * The reader's paging is kept resident: live appends do not trim what the reader has paged up to (see `extended`).
     */
    loadEarlier(key: SessionKey, scope: TranscriptScope = MAIN): Promise<void> {
      const tk = scopeKey(key, scope);
      const cur = store.get().transcripts[tk];
      if (cur && hasOlder(cur)) patch(tk, { extended: true });
      return pageEarlier(key, scope);
    },
  };
}
export type Transcripts = ReturnType<typeof createTranscripts>;
