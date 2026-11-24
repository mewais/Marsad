import { act, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { EARLIER_SPECS, historyRounds } from "../../src/bridge/fixture/earlier";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { SPECS } from "../../src/bridge/fixture/sessions";
import type { HistoryRequest, SessionEvent } from "../../src/bridge/wire";
import { admits, createApp, hasOlder, MAIN, scopeKey, subagentScope, type TranscriptSlice } from "../../src/state";
import { quietBackend, sessionByName } from "../helpers";
import { cursorsOf, holdHistory, journal } from "../historyGate";

const COMPLETE = "resumed-invoice-rounding";
const LOADING = "resumed-stripe-webhook";
const MISSING = "resumed-orphan-notes";
/** The same resumed conversation with a session of its own long enough to need several pages. */
const LONG = { ...EARLIER_SPECS[0], n: 60, name: "resumed-long", longHistory: true };

async function boot(name: string, extra: typeof SPECS = []) {
  localStorage.clear();
  const backend = quietBackend({ specs: [...SPECS, ...EARLIER_SPECS, ...extra] });
  const app = createApp(backend);
  const s = sessionByName(backend, name);
  await waitFor(() => expect(app.store.get().sessions[s.key]).toBeDefined());
  return { backend, app, s, earlier: backend.earlier(s.host, s.id) };
}
type Booted = Awaited<ReturnType<typeof boot>>;

const slice = (app: Booted["app"], key: string, scope = MAIN): TranscriptSlice => app.store.get().transcripts[scopeKey(key, scope)];
const asks = (b: FixtureBackend) => b.requests.filter((r) => r.method === "session.history").map((r) => r.params as HistoryRequest);
const earlierAsks = (b: FixtureBackend) => asks(b).filter((r) => r.earlier);
const cursors = (events: readonly SessionEvent[]) => events.map((e) => e.cursor);
const ascending = (events: readonly SessionEvent[]) => cursors(events).every((c, i, all) => i === 0 || c > all[i - 1]);
const isHistory = (e: SessionEvent) => e.kind === "agent" && e.event?.type === "history";
const info = (app: Booted["app"], key: string) => app.store.get().sessions[key].earlier;

describe("a resumed conversation's transcript in the store: two lists, never merged", () => {
  it("the earlier list is read with `earlier: true` pages, newest first, once the session's own events are here; the lists hold different things", async () => {
    const { app, s, backend } = await boot(COMPLETE);
    expect(info(app, s.key).state).toEqual({ kind: "loaded", complete: true });
    app.transcripts.open(s.key);
    await waitFor(() => expect(slice(app, s.key)?.earlier.events.length).toBe(60));
    const t = slice(app, s.key);
    expect(t.hasMore).toBe(false);
    expect(t.earlier.hasMore).toBe(true);
    expect(t.earlier.events.every(isHistory)).toBe(true);
    expect(t.events.some(isHistory)).toBe(false);
    expect(ascending(t.earlier.events) && ascending(t.events)).toBe(true);
    // Newest first (nothing before it), and the live page asked first.
    const [live, first] = asks(backend);
    expect(live.earlier).toBeFalsy();
    expect(first).toMatchObject({ earlier: true, limit: 60, scope: { kind: "main" } });
    expect(first.before).toBeUndefined();
    // They are never merged by cursor: the earlier items' cursors are not compared with the live ones (here some are above them).
    expect(t.earlier.events[t.earlier.events.length - 1].cursor).toBeGreaterThan(t.events[1].cursor);
  });

  it("pages up through the earlier list with `before`, and holds exactly the items of its scope the daemon counts", async () => {
    const { app, s, backend, earlier } = await boot(COMPLETE);
    app.transcripts.open(s.key);
    await waitFor(() => expect(slice(app, s.key)?.earlier.events.length).toBe(60));
    // The daemon's count is of every item; the session's own transcript leaves out what its subagents said.
    expect(info(app, s.key).items).toBe(earlier.items().length);
    const total = earlier.items().filter((e) => admits(MAIN, e)).length;
    expect(total).toBeLessThan(info(app, s.key).items);
    while (slice(app, s.key).earlier.hasMore) await app.transcripts.loadEarlier(s.key);
    const t = slice(app, s.key);
    expect(t.earlier.events).toHaveLength(total);
    expect(ascending(t.earlier.events)).toBe(true);
    expect(new Set(cursors(t.earlier.events)).size).toBe(total);
    const paged = earlierAsks(backend).filter((r) => r.before !== undefined);
    expect(paged.length).toBeGreaterThan(0);
    // Each page ends where the one after it begins: `before` is the oldest cursor held.
    expect(paged[0].before).toBe(cursors(slice(app, s.key).earlier.events)[total - 60]);
    expect(hasOlder(t)).toBe(false);
  });

  it("one control, one direction: no earlier event while the session is incomplete, checked at every page the pager is let through", async () => {
    const { app, s, backend } = await boot(LONG.name, [LONG]);
    const own = cursorsOf(journal(backend, s.host, s.id), (e) => admits(MAIN, e));
    // Every session.history is held: the test releases one page at a time and looks at the store in between.
    const gate = holdHistory(backend);
    app.transcripts.open(s.key);
    const t = () => slice(app, s.key);
    // What has been asked for so far, answered or held, in order.
    const asked = () => [...gate.answered().map((a) => a.params), ...gate.pending()];
    for (;;) {
      await waitFor(() => expect(gate.pending()).toHaveLength(1));
      const next = gate.pending()[0];
      if (next.earlier) break;
      // The session's own events are not all here: nothing of the earlier history is asked for, held, or in the store.
      expect(asked().some((p) => p.earlier)).toBe(false);
      expect(t().earlier.events).toHaveLength(0);
      if (t().events.length) {
        // The held page reads just before the oldest event in the store, and the store holds exactly the newest of the session's events.
        expect(next.before).toBe(t().events[0].cursor);
        expect(t().events.map((e) => e.cursor)).toEqual(own.slice(own.length - t().events.length));
        expect(t().hasMore).toBe(true);
        expect(t().earlier.hasMore).toBe(false);
      }
      await gate.release();
    }
    // Only when the session's own events are all in is the earlier list asked for.
    expect(t().hasMore).toBe(false);
    expect(t().events.map((e) => e.cursor)).toEqual(own);
    expect(t().earlier.events).toHaveLength(0);
    await gate.release();
    await waitFor(() => expect(t().earlier.events).toHaveLength(60));
    // Order of the asks: every session page first, then exactly one earlier page.
    const order = asked().map((p) => (p.earlier ? "earlier" : "live"));
    expect(order[order.length - 1]).toBe("earlier");
    expect(order.slice(0, -1).every((k) => k === "live")).toBe(true);
    expect(order.filter((k) => k === "earlier")).toHaveLength(1);
    expect(t().events.some(isHistory)).toBe(false);
    gate.open();
  });

  it("a session that began its conversation here has no earlier list and is never asked for one", async () => {
    const { app, backend } = await boot(COMPLETE);
    const plain = sessionByName(backend, "csv-schema-worker");
    await waitFor(() => expect(app.store.get().sessions[plain.key]).toBeDefined());
    expect(info(app, plain.key).state).toEqual({ kind: "none" });
    app.transcripts.open(plain.key);
    await waitFor(() => expect(slice(app, plain.key)?.loading).toBe(false));
    expect(earlierAsks(backend)).toHaveLength(0);
    expect(slice(app, plain.key).earlier).toEqual({ events: [], hasMore: false, loading: false });
  });

  it("a history that cannot be had is never asked for; there is nothing to ask for", async () => {
    const { app, s, backend } = await boot(MISSING);
    app.transcripts.open(s.key);
    await waitFor(() => expect(slice(app, s.key)?.loading).toBe(false));
    expect(info(app, s.key)).toMatchObject({ state: { kind: "unavailable", reason: { kind: "source_missing" } }, items: 0 });
    expect(earlierAsks(backend)).toHaveLength(0);
  });
});

describe("the earlier list follows the loading, as the daemon reports it", () => {
  it("what has arrived can be read, and each record that arrives is joined to it: no gap, no repeat, the same order", async () => {
    const { app, s, backend, earlier } = await boot(LOADING);
    app.transcripts.open(s.key);
    await waitFor(() => expect(slice(app, s.key)?.earlier.events.length).toBe(40));
    expect(info(app, s.key).state).toEqual({ kind: "loading" });
    const before = cursors(slice(app, s.key).earlier.events);
    act(() => void earlier.step());
    await waitFor(() => expect(slice(app, s.key).earlier.events.length).toBe(80));
    expect(info(app, s.key)).toMatchObject({ items: 80, state: { kind: "loading" } });
    const got = slice(app, s.key).earlier.events;
    expect(cursors(got).slice(0, 40)).toEqual(before);
    expect(ascending(got)).toBe(true);
    expect(cursors(got)).toEqual(cursors(earlier.items()));
    // Only the newest were asked for.
    expect(earlierAsks(backend).every((r) => r.before === undefined)).toBe(true);
  });

  it("the end of the loading changes the state and the gaps, and asks for nothing: the items are the same", async () => {
    const { app, s, backend, earlier } = await boot(LOADING);
    app.transcripts.open(s.key);
    await waitFor(() => expect(slice(app, s.key)?.earlier.events.length).toBe(40));
    act(() => void earlier.step());
    act(() => void earlier.step());
    await waitFor(() => expect(slice(app, s.key).earlier.events.length).toBe(120));
    const n = earlierAsks(backend).length;
    act(() => void earlier.step());
    await waitFor(() => expect(info(app, s.key).state).toEqual({ kind: "loaded", complete: false }));
    expect(info(app, s.key).gaps).toMatchObject({ omitted: 120, shortened: 2, unrecognised: 3 });
    expect(earlierAsks(backend)).toHaveLength(n);
    expect(slice(app, s.key).earlier.events).toHaveLength(120);
  });

  it("more arrives than a page holds: the pages between are read, so the list stays contiguous", async () => {
    const { app, s, backend, earlier } = await boot(LOADING);
    app.transcripts.open(s.key);
    await waitFor(() => expect(slice(app, s.key)?.earlier.events.length).toBe(40));
    const asked = earlierAsks(backend).length;
    act(() => earlier.add(historyRounds(26, Date.now() - 3_600_000)));
    await waitFor(() => expect(slice(app, s.key).earlier.events.length).toBe(170));
    const got = slice(app, s.key).earlier.events;
    expect(cursors(got)).toEqual(cursors(earlier.items()));
    expect(earlierAsks(backend).length - asked).toBeGreaterThanOrEqual(3);
    expect(slice(app, s.key).earlier.hasMore).toBe(false);
  });

  it("a failure is said and not retried by itself; the reader's own request (or the next progress) asks again", async () => {
    const { app, s, backend, earlier } = await boot(LOADING);
    let failing = true;
    const request = backend.request.bind(backend);
    backend.request = (<T,>(method: string, params?: unknown, opts?: object) =>
      method === "session.history" && (params as HistoryRequest).earlier && failing ? Promise.reject({ code: "io", message: "the link dropped" }) : request<T>(method, params, opts)) as typeof backend.request;
    app.transcripts.open(s.key);
    await waitFor(() => expect(slice(app, s.key)?.earlier.error).toBe("the link dropped"));
    const n = asks(backend).length;
    // Other changes to the store do not ask again.
    act(() => app.store.set((st) => ({ ...st, lookedAt: st.lookedAt + 1 })));
    await app.transcripts.loadEarlier(s.key, MAIN).catch(() => undefined);
    failing = false;
    await app.transcripts.loadEarlier(s.key);
    await waitFor(() => expect(slice(app, s.key).earlier.events).toHaveLength(40));
    expect(slice(app, s.key).earlier.error).toBeUndefined();
    expect(asks(backend).length).toBeGreaterThan(n);
    void earlier;
  });

  it("an import that fails keeps what arrived and says why", async () => {
    const { app, s, earlier } = await boot(LOADING);
    app.transcripts.open(s.key);
    await waitFor(() => expect(slice(app, s.key)?.earlier.events.length).toBe(40));
    act(() => earlier.fail({ kind: "failed", detail: "disk full" }));
    await waitFor(() => expect(info(app, s.key).state).toMatchObject({ kind: "unavailable", reason: { kind: "failed", detail: "disk full" } }));
    expect(slice(app, s.key).earlier.events).toHaveLength(40);
  });
});

describe("a subagent's earlier history", () => {
  it("is the earlier items that hold its lines, read with the scope, above its live part", async () => {
    const { app, s, backend } = await boot(COMPLETE);
    const close = app.transcripts.open(s.key, subagentScope("h-sub"));
    const tk = scopeKey(s.key, subagentScope("h-sub"));
    await waitFor(() => expect(app.store.get().transcripts[tk]?.earlier.events.length).toBe(4));
    const t = app.store.get().transcripts[tk];
    expect(t.events).toEqual([]);
    expect(t.earlier.events.map((e) => (e.kind === "agent" && e.event?.type === "history" && e.event.event.type === "scoped" ? e.event.event.event.type : "?"))).toEqual(["assistant_delta", "tool_call_started", "tool_call_finished", "assistant_delta"]);
    expect(earlierAsks(backend).some((r) => r.scope?.kind === "subagent" && r.scope.call_id === "h-sub")).toBe(true);
    // The session's own transcript holds none of it.
    app.transcripts.open(s.key);
    await waitFor(() => expect(slice(app, s.key)?.earlier.events.length).toBeGreaterThan(0));
    expect(slice(app, s.key).earlier.events.some((e) => e.kind === "agent" && e.event?.type === "history" && e.event.event.type === "scoped")).toBe(false);
    close();
    expect(app.store.get().transcripts[tk]).toBeUndefined();
  });
});
