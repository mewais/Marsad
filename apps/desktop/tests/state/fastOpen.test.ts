import { describe, expect, it } from "vitest";
import type { Api } from "../../src/bridge/api";
import type { HistoryRequest, SessionEvent, SessionSnapshot } from "../../src/bridge/wire";
import { createAppStore } from "../../src/state/model";
import type { Feeds } from "../../src/state/feeds";
import { createTranscripts, RELEASE_KEEP, RESIDENT, TRIM_SLACK } from "../../src/state/transcript";

/*
 * The open of a session at its snapshot's cursor (D6): the subscription starts from the cursor at once, the newest page is read
 * beside it, and older pages follow in the background. The daemon is replaced by a fake whose history answers can be held, so the
 * order of replies and requests is the test's to choose.
 */

const KEY = "host1/sess1";
const N = 6000; // the session's own events held by the fake daemon: cursors 1..N
const C = N; // the snapshot's cursor
const OPEN_BYTES = 256 * 1024;
const LATER_BYTES = 1024 * 1024;

const ev = (cursor: number): SessionEvent =>
  ({ cursor, t: 0, kind: "agent", event: { type: "assistant_delta", text: `e${cursor}`, message_id: `m${cursor}` } }) as unknown as SessionEvent;
const cursors = (events: readonly SessionEvent[]) => events.map((e) => e.cursor);
const ascending = (events: readonly SessionEvent[]) => events.every((e, i) => i === 0 || e.cursor > events[i - 1].cursor);

/** A fake daemon: `history` answers from `all` (manually when `manual`, one answer per `answer()`), `subscribe` records what it was started from. */
function fakeDaemon(all: SessionEvent[]) {
  const calls: HistoryRequest[] = [];
  const pending: { r: HistoryRequest; ok: (p: unknown) => void; fail: (e: unknown) => void }[] = [];
  const subs: { from: number; cb: (e: SessionEvent) => void; closed: boolean }[] = [];
  const page = (r: HistoryRequest) => {
    const pool = all.filter((e) => r.before == null || e.cursor < r.before);
    const events = pool.slice(Math.max(0, pool.length - r.limit));
    return { events, has_more: pool.length > events.length };
  };
  const api = {
    history: (_host: string, r: HistoryRequest) =>
      new Promise((ok, fail) => {
        calls.push(r);
        if (manual) pending.push({ r, ok, fail });
        else ok(page(r));
      }),
    subscribe: {
      sessionEvents: (_host: string, _id: string, from: number, cb: (e: SessionEvent) => void) => {
        const s = { from, cb, closed: false };
        subs.push(s);
        return () => {
          s.closed = true;
        };
      },
    },
  };
  let manual = true;
  return {
    api: api as unknown as Api,
    calls,
    pending,
    subs,
    open: () => subs.filter((s) => !s.closed),
    automatic: () => {
      manual = false;
    },
    /** Answer the oldest pending request from the daemon's data. */
    answer: () => {
      const p = pending.shift()!;
      p.ok(page(p.r));
    },
    reject: (e: unknown) => pending.shift()!.fail(e),
  };
}

/** Let the chained promises of the transcript settle (no timers: every step is a microtask). */
async function settle() {
  for (let i = 0; i < 50; i++) await Promise.resolve();
}

function setup(snapshotCursor: number | null, all: SessionEvent[] = Array.from({ length: N }, (_, i) => ev(i + 1))) {
  const daemon = fakeDaemon(all);
  const store = createAppStore(
    snapshotCursor === null ? {} : { sessions: { [KEY]: { cursor: snapshotCursor, earlier: { state: { kind: "none" }, items: 0 } } as unknown as SessionSnapshot } },
  );
  const feeds = { open: (_name: string, start: (onEnd: () => void) => () => void) => start(() => undefined) } as unknown as Feeds;
  const t = createTranscripts(store, daemon.api, feeds);
  const slice = () => store.get().transcripts[KEY];
  return { store, t, daemon, slice };
}

/** The live delivery of the one open subscription. */
const deliver = (daemon: ReturnType<typeof fakeDaemon>, cursor: number) => daemon.open()[0].cb(ev(cursor));

/** Open at C and answer the first page; then answer each background page in turn, checking that only one is ever in flight. */
async function openToResident(daemon: ReturnType<typeof fakeDaemon>, t: ReturnType<typeof createTranscripts>) {
  const close = t.open(KEY);
  await settle();
  daemon.answer();
  await settle();
  while (daemon.pending.length) {
    expect(daemon.pending).toHaveLength(1);
    daemon.answer();
    await settle();
  }
  return close;
}
describe("a session opened at its snapshot's cursor", () => {
  it("subscribes from the snapshot's cursor while the newest page is still pending, and asks for that page before the cursor", async () => {
    const { t, daemon } = setup(C);
    t.open(KEY);
    await settle();
    expect(daemon.subs).toHaveLength(1);
    expect(daemon.subs[0].from).toBe(C);
    expect(daemon.pending).toHaveLength(1);
    expect(daemon.pending[0].r).toMatchObject({ before: C + 1, limit: 60, max_bytes: OPEN_BYTES });
  });

  it("live events that arrive before the page are held and merged with it in cursor order, without repeats", async () => {
    const { t, daemon, slice } = setup(C);
    t.open(KEY);
    await settle();
    deliver(daemon, C + 1);
    deliver(daemon, C + 2);
    deliver(daemon, C + 2); // a repeat while held is one event
    expect(slice()?.events).toEqual([]);
    expect(slice()?.loading).toBe(true);
    daemon.answer();
    await settle();
    const got = cursors(slice()!.events);
    expect(got).toHaveLength(62);
    expect(got.slice(-3)).toEqual([C, C + 1, C + 2]);
    expect(ascending(slice()!.events)).toBe(true);
    // The first page is in; the background pager is now asking for the page before it.
    expect(daemon.pending).toHaveLength(1);
    expect(daemon.pending[0].r.before).toBe(C - 59);
  });

  it("a failed first page keeps live events flowing, and 'Load earlier' retries from the first live event", async () => {
    const { t, daemon, slice } = setup(C);
    t.open(KEY);
    await settle();
    deliver(daemon, C + 1);
    daemon.reject(new Error("boom"));
    await settle();
    expect(slice()).toMatchObject({ loading: false, error: "boom", hasMore: true });
    expect(cursors(slice()!.events)).toEqual([C + 1]);
    deliver(daemon, C + 2);
    expect(cursors(slice()!.events)).toEqual([C + 1, C + 2]);

    const retry = t.loadEarlier(KEY);
    await settle();
    expect(daemon.pending).toHaveLength(1);
    expect(daemon.pending[0].r.before).toBe(C + 1);
    daemon.answer();
    await retry;
    expect(slice()!.error).toBeUndefined();
    expect(cursors(slice()!.events).slice(-3)).toEqual([C, C + 1, C + 2]);
    expect(ascending(slice()!.events)).toBe(true);
  });

  it("a failed first page with nothing live yet retries before the snapshot's cursor", async () => {
    const { t, daemon, slice } = setup(C);
    t.open(KEY);
    await settle();
    daemon.reject(new Error("boom"));
    await settle();
    expect(slice()).toMatchObject({ events: [], hasMore: true, error: "boom" });
    void t.loadEarlier(KEY);
    await settle();
    expect(daemon.pending[0].r.before).toBe(C + 1);
  });

  it("the background pager asks for one page at a time, of 500 and 1 MiB, and stops once 4000 events are held", async () => {
    const { t, daemon, slice } = setup(C);
    const close = await openToResident(daemon, t);
    // 60 from the first page, then 500 a page, the last one sized to the room left: 60 + 7 * 500 + 440 is exactly RESIDENT.
    expect(daemon.calls).toHaveLength(9);
    expect(daemon.calls.slice(1).map((r) => r.limit)).toEqual([...Array(7).fill(500), 440]);
    expect(daemon.calls.slice(1).every((r) => r.max_bytes === LATER_BYTES)).toBe(true);
    expect(slice()!.events).toHaveLength(RESIDENT);
    expect(slice()!.hasMore).toBe(true);
    expect(ascending(slice()!.events)).toBe(true);
    close();
  });

  it("live appends trim the oldest to the resident window once the list passes it by the slack, and the host still has the rest", async () => {
    const { t, daemon, slice } = setup(C);
    const close = await openToResident(daemon, t);
    const held = slice()!.events.length; // RESIDENT
    for (let i = 1; i <= RESIDENT + TRIM_SLACK - held; i++) deliver(daemon, C + i);
    expect(slice()!.events).toHaveLength(RESIDENT + TRIM_SLACK);
    deliver(daemon, C + (RESIDENT + TRIM_SLACK - held) + 1);
    expect(slice()!.events).toHaveLength(RESIDENT);
    expect(slice()!.events[slice()!.events.length - 1].cursor).toBe(C + (RESIDENT + TRIM_SLACK - held) + 1);
    expect(slice()!.hasMore).toBe(true);
    expect(ascending(slice()!.events)).toBe(true);
    close();
  });

  it("a reader who has paged up keeps what is held: live appends do not trim it", async () => {
    const { t, daemon, slice } = setup(C);
    const close = await openToResident(daemon, t);
    const p = t.loadEarlier(KEY);
    await settle();
    daemon.answer();
    await p;
    const held = slice()!.events.length; // RESIDENT + one page
    expect(held).toBe(RESIDENT + 500);
    for (let i = 1; i <= 1000; i++) deliver(daemon, C + i);
    expect(slice()!.events).toHaveLength(held + 1000);
    close();
  });

  it("on release the transcript keeps its newest events, marks what it trimmed as held by the host, and clears the paging", async () => {
    const { t, daemon, slice } = setup(C);
    const close = await openToResident(daemon, t);
    const p = t.loadEarlier(KEY);
    await settle();
    daemon.answer();
    await p;
    const newest = cursors(slice()!.events).slice(-RELEASE_KEEP);
    expect(slice()!.extended).toBe(true);
    close();
    expect(cursors(slice()!.events)).toEqual(newest);
    expect(slice()!.hasMore).toBe(true);
    expect(slice()!.extended).toBe(false);
  });

  it("without a loaded snapshot the open is sequential: the page first, then the subscription from its last cursor", async () => {
    const { t, daemon } = setup(null);
    daemon.automatic();
    t.open(KEY);
    await settle();
    expect(daemon.calls).toHaveLength(1);
    expect(daemon.calls[0].before).toBeUndefined();
    expect(daemon.calls[0].max_bytes).toBeUndefined();
    expect(daemon.calls[0].limit).toBe(60);
    expect(daemon.subs).toHaveLength(1);
    expect(daemon.subs[0].from).toBe(N);
  });
});
