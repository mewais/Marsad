import { waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Api } from "../../src/bridge/api";
import type { DiagTailEvent } from "../../src/bridge/wire";
import { createApp, createAppStore, TAIL_KEEP, type TailSpec } from "../../src/state";
import { createTails } from "../../src/state/diagnostics";
import { createFeeds } from "../../src/state/feeds";
import { quietBackend, sessionByName } from "../helpers";

async function boot() {
  localStorage.clear();
  const backend = quietBackend();
  const app = createApp(backend);
  await waitFor(() => expect(Object.keys(app.store.get().sessions).length).toBe(15));
  const topics = () => backend.subscriptions.filter((s) => s.topic.startsWith("session.trail:") || s.topic === "diag.tail");
  return { backend, app, topics };
}

describe("session trails are held only while a panel has them open", () => {
  it("loads the newest page, then follows it live from the last cursor; the last release closes it and forgets the data", async () => {
    const { backend, app, topics } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    expect(topics()).toEqual([]);
    const release = app.trails.open(s.key);
    expect(app.store.get().trails[s.key]).toMatchObject({ loading: true, items: [] });
    await waitFor(() => expect(topics()).toHaveLength(1));
    expect(topics()[0]).toMatchObject({ topic: `session.trail:${s.id}`, host: s.host });
    const slice = app.store.get().trails[s.key];
    expect(slice.loading).toBe(false);
    expect(slice.items.length).toBeGreaterThan(3);
    expect(slice.items.map((i) => i.cursor)).toEqual([...slice.items.map((i) => i.cursor)].sort((a, b) => a - b));
    // A prompt sent now arrives on the live subscription, after what was loaded.
    const before = slice.items.length;
    await app.actions.sendPrompt(s.key, "go on");
    await waitFor(() => expect(app.store.get().trails[s.key].items.some((i) => i.ev === "input.applied" && i.status === "sent" && i.chars === 5)).toBe(true));
    const kinds = app.store.get().trails[s.key].items.slice(before).map((i) => i.ev);
    expect(kinds.slice(0, 5)).toEqual(["input.applied", "input.written", "phase", "input.acknowledged", "turn.started"]);
    release();
    expect(topics()).toEqual([]);
    expect(app.store.get().trails[s.key]).toBeUndefined();
    expect(app.store.get().feedErrors[`trail:${s.key}`]).toBeUndefined();
  });

  it("two panels share one subscription; it closes with the second release", async () => {
    const { backend, app, topics } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    const a = app.trails.open(s.key);
    const b = app.trails.open(s.key);
    await waitFor(() => expect(topics()).toHaveLength(1));
    a();
    a();
    expect(topics()).toHaveLength(1);
    b();
    expect(topics()).toEqual([]);
  });

  it("released before the page arrives: nothing is subscribed afterwards", async () => {
    const { backend, app, topics } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    app.trails.open(s.key)();
    await new Promise((r) => setTimeout(r, 20));
    expect(topics()).toEqual([]);
    expect(app.store.get().trails[s.key]).toBeUndefined();
  });

  it("pages back through a long trail: older entries are prepended, contiguous, until none remain", async () => {
    const { backend, app } = await boot();
    const s = sessionByName(backend, "upgrade-tauri-2-5");
    app.trails.open(s.key);
    await waitFor(() => expect(app.store.get().trails[s.key]?.loading).toBe(false));
    const first = app.store.get().trails[s.key];
    expect(first.items).toHaveLength(200);
    expect(first.hasMore).toBe(true);
    await app.trails.loadEarlier(s.key);
    const next = app.store.get().trails[s.key];
    expect(next.items.length).toBeGreaterThan(200);
    expect(next.items[next.items.length - first.items.length].cursor).toBe(first.items[0].cursor);
    expect(next.hasMore).toBe(false);
    expect(next.items[0].ev).toBe("process.spawned");
    const cursors = next.items.map((i) => i.cursor);
    expect(new Set(cursors).size).toBe(cursors.length);
    expect(cursors).toEqual([...cursors].sort((a, b) => a - b));
  });

  it("a page that cannot be read says why, and a retry reads it", async () => {
    const { backend, app, topics } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    const request = backend.request.bind(backend);
    let fail = true;
    backend.request = (<T,>(m: string, p?: unknown, o?: object) => (fail && m === "session.trail.page" ? Promise.reject({ code: "timeout", message: "the host did not answer" }) : request<T>(m, p, o))) as typeof backend.request;
    app.trails.open(s.key);
    await waitFor(() => expect(app.store.get().trails[s.key]?.error).toBe("the host did not answer"));
    expect(topics()).toEqual([]);
    fail = false;
    app.trails.retry(s.key);
    await waitFor(() => expect(topics()).toHaveLength(1));
    expect(app.store.get().trails[s.key]).toMatchObject({ loading: false, error: undefined });
    expect(app.store.get().trails[s.key].items.length).toBeGreaterThan(0);
  });

  it("a subscription that ends is recorded with its reason and started again from the last cursor, without repeating anything", async () => {
    const { backend, app, topics } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    app.trails.open(s.key);
    await waitFor(() => expect(topics()).toHaveLength(1));
    const n = app.store.get().trails[s.key].items.length;
    backend.endSubscriptions(`session.trail:${s.id}`, { code: "lagged", message: "too slow" });
    expect(app.store.get().feedErrors[`trail:${s.key}`]).toBe("too slow");
    expect(topics()).toEqual([]);
    app.trails.retry(s.key);
    await waitFor(() => expect(topics()).toHaveLength(1));
    expect(app.store.get().feedErrors[`trail:${s.key}`]).toBeUndefined();
    expect(app.store.get().trails[s.key].items).toHaveLength(n);
  });
});

describe("daemon log tails", () => {
  const spec = (o: Partial<TailSpec> = {}): TailSpec => ({ id: "t1", filter: {}, ...o });

  it("subscribes on the daemon it names with the filter it asks for, keeps what arrives in order, and goes when released", async () => {
    const { backend, app, topics } = await boot();
    const release = app.tails.open(spec({ id: "x", host: "hz", filter: { ev_prefix: "daemon." } }));
    expect(app.store.get().tails.x).toMatchObject({ state: "waiting", records: [] });
    await waitFor(() => expect(app.store.get().tails.x.state).toBe("live"));
    expect(topics()).toEqual([{ topic: "diag.tail", host: "hz", params: { ev_prefix: "daemon." } }]);
    const recs = app.store.get().tails.x.records;
    expect(recs.map((r) => r.ev)).toEqual(["daemon.start", "daemon.ready"]);
    // A record written now arrives live.
    const log = (backend as unknown as { hostLog(id: string): { emit(ev: string, f: object): void } }).hostLog("hz");
    log.emit("daemon.shutdown", { why: "request" });
    await waitFor(() => expect(app.store.get().tails.x.records.map((r) => r.ev)).toEqual(["daemon.start", "daemon.ready", "daemon.shutdown"]));
    release();
    expect(topics()).toEqual([]);
    expect(app.store.get().tails.x).toBeUndefined();
  });

  it("the app checks every record against the filter itself: a transport that drops the filter shows the same", async () => {
    const { backend, app } = await boot();
    backend.tailFilters = false;
    app.tails.open(spec({ id: "y", host: "hz", filter: { ev_prefix: "daemon." } }));
    await waitFor(() => expect(app.store.get().tails.y?.state).toBe("live"));
    expect(app.store.get().tails.y.records.map((r) => r.ev)).toEqual(["daemon.start", "daemon.ready"]);
    // The daemon's whole ring did arrive; only what matches is kept.
    const log = (backend as unknown as { hostLog(id: string): { emit(ev: string, f: object): void } }).hostLog("hz");
    log.emit("rpc.traced", { trace: "u-1234567890123456", method: "m", hops: 0, ms: 1, outcome: "ok" });
    log.emit("daemon.shutdown", { why: "request" });
    await waitFor(() => expect(app.store.get().tails.y.records).toHaveLength(3));
    expect(app.store.get().tails.y.records.map((r) => r.ev)).toEqual(["daemon.start", "daemon.ready", "daemon.shutdown"]);
  });

  it("keeps only the records about one host, by its host field or its link label, and only the kinds asked for", async () => {
    const { app } = await boot();
    app.tails.open(spec({ id: "w", keep: { about: "win" } }));
    await waitFor(() => expect(app.store.get().tails.w?.state).toBe("live"));
    const recs = app.store.get().tails.w.records;
    expect(recs.length).toBeGreaterThan(10);
    expect(recs.every((r) => r.fields.host === "win" || r.fields.link === "hosts:win")).toBe(true);
    expect(recs.some((r) => r.ev === "link.attempt_end")).toBe(true);
    expect(recs.some((r) => r.ev === "boot.failed")).toBe(true);
    app.tails.open(spec({ id: "v", keep: { about: "win", events: ["boot."] } }));
    await waitFor(() => expect(app.store.get().tails.v?.state).toBe("live"));
    expect(app.store.get().tails.v.records.every((r) => r.ev.startsWith("boot."))).toBe(true);
  });

  it("a subscription that ends is recorded and resumes from the last record, so nothing is repeated or missed", async () => {
    const { backend, app, topics } = await boot();
    const log = (backend as unknown as { hostLog(id: string): { emit(ev: string, f: object): void } }).hostLog("hz");
    app.tails.open(spec({ id: "r", host: "hz", filter: {} }));
    await waitFor(() => expect(app.store.get().tails.r?.state).toBe("live"));
    const n = app.store.get().tails.r.records.length;
    backend.endSubscriptions("diag.tail", { code: "lagged", message: "too slow" });
    expect(app.store.get().feedErrors["tail:r"]).toBe("too slow");
    // Written while it was not subscribed:
    log.emit("daemon.shutdown", { why: "request" });
    app.tails.retry();
    await waitFor(() => expect(topics()).toHaveLength(1));
    await waitFor(() => expect(app.store.get().tails.r.records).toHaveLength(n + 1));
    expect(app.store.get().tails.r.records[n].ev).toBe("daemon.shutdown");
    expect(app.store.get().feedErrors["tail:r"]).toBeUndefined();
  });

  it("keeps the newest TAIL_KEEP records and counts what it dropped; lines that are not records are counted, not shown", async () => {
    const { backend, app } = await boot();
    const log = (backend as unknown as { hostLog(id: string): { emit(ev: string, f: object): void } }).hostLog("hz");
    app.tails.open(spec({ id: "k", host: "hz" }));
    await waitFor(() => expect(app.store.get().tails.k?.state).toBe("live"));
    const start = app.store.get().tails.k.records.length;
    for (let i = 0; i < TAIL_KEEP + 10; i++) log.emit("session.retry", { session: "s", attempt: i });
    await waitFor(() => expect(app.store.get().tails.k.trimmed).toBe(start + 10));
    const k = app.store.get().tails.k;
    expect(k.records).toHaveLength(TAIL_KEEP);
    expect(k.records[k.records.length - 1].fields.attempt).toBe(TAIL_KEEP + 9);
  });

  it("lines that are not records are counted, not shown, and never stop the tail", async () => {
    const store = createAppStore();
    const good = JSON.stringify({ b: "b", seq: 1, t: 5, m: 0, src: "hub", lvl: "info", ev: "boot.ok", host: "hz", n: 1, ms: 3 });
    let deliver: (e: DiagTailEvent) => void = () => undefined;
    const api = { subscribe: { diagTail: (_h: unknown, _f: unknown, _from: unknown, cb: (e: DiagTailEvent) => void) => ((deliver = cb), () => undefined) } } as unknown as Api;
    const tails = createTails(store, api, createFeeds(store));
    tails.open(spec({ id: "u" }));
    deliver({ kind: "reset", records: ["not json", "[1]", good, "{\"ev\":\"x\"}"] });
    expect(store.get().tails.u).toMatchObject({ state: "live", unreadable: 3 });
    expect(store.get().tails.u.records.map((r) => r.ev)).toEqual(["boot.ok"]);
    deliver({ kind: "record", line: "garbage" });
    deliver({ kind: "record", line: JSON.stringify({ b: "b", seq: 2, t: 6, m: 0, src: "hub", lvl: "info", ev: "boot.ok", host: "hz", n: 2, ms: 3 }) });
    expect(store.get().tails.u).toMatchObject({ unreadable: 4 });
    expect(store.get().tails.u.records.map((r) => r.fields.n)).toEqual([1, 2]);
    // An event of a kind this build does not know changes nothing.
    deliver({ kind: "from_the_future" } as unknown as DiagTailEvent);
    expect(store.get().tails.u.records).toHaveLength(2);
  });

  it("two panels asking for the same tail share one subscription", async () => {
    const { app, topics } = await boot();
    const a = app.tails.open(spec({ id: "s" }));
    const b = app.tails.open(spec({ id: "s" }));
    await waitFor(() => expect(topics()).toHaveLength(1));
    a();
    expect(topics()).toHaveLength(1);
    b();
    expect(topics()).toEqual([]);
  });

  it("a host that is not connected delivers nothing: the tail waits", async () => {
    const { app } = await boot();
    app.tails.open(spec({ id: "d", host: "win" }));
    await new Promise((r) => setTimeout(r, 20));
    expect(app.store.get().tails.d).toMatchObject({ state: "waiting", records: [] });
  });
});
