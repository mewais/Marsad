import { act, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { createStore, shallowEqual, useStore } from "../../src/lib/store";
import { admits, AppProvider, createApp, MAIN, renderCounts, selectGroups, NO_FILTERS, useSessionView, useRenderProbe } from "../../src/state";
import { RESIDENT } from "../../src/state/transcript";
import { InboxRow } from "../../src/features/inbox/InboxRow";
import { memoryHistory, RouterProvider } from "../../src/router";
import { applyFleet, applyWorkstreams } from "../../src/state/controller";
import type { FleetEvent, WorkstreamsEvent } from "../../src/bridge/wire";
import { quietBackend, sessionByName } from "../helpers";
import { cursorsOf, journal } from "../historyGate";

async function boot() {
  localStorage.clear();
  const backend = quietBackend();
  const app = createApp(backend);
  await waitFor(() => expect(Object.keys(app.store.get().sessions).length).toBe(15));
  return { backend, app };
}

describe("store", () => {
  it("useStore re-renders only when the selected slice changes", () => {
    const store = createStore({ a: 1, b: 1 });
    let renders = 0;
    function A() {
      renders++;
      return <span>{useStore(store, (s) => s.a)}</span>;
    }
    render(<A />);
    act(() => store.set((s) => ({ ...s, b: 2 })));
    expect(renders).toBe(1);
    act(() => store.set((s) => ({ ...s, a: 2 })));
    expect(renders).toBe(2);
    expect(shallowEqual({ x: 1 }, { x: 1 })).toBe(true);
  });
});

describe("session slices", () => {
  it("loads all hosts, keyed by hostId/sessionId, and learns drivers from drivers.inventory", async () => {
    const { app } = await boot();
    const keys = Object.keys(app.store.get().sessions);
    expect(keys.every((k) => /^(x1|hz|win)\/.+/.test(k))).toBe(true);
    await waitFor(() => expect(app.store.get().driverOrder).toEqual(["claude", "codex", "opencode", "kilo"]));
  });

  it("an update to one session does not re-render another session's row", async () => {
    const { app, backend } = await boot();
    const a = sessionByName(backend, "pdf-renderer-worker");
    const b = sessionByName(backend, "csv-schema-worker");
    renderCounts.clear();
    render(
      <AppProvider value={app}>
        <RouterProvider history={memoryHistory()}>
          <InboxRow sessionKey={a.key} selected={false} onSelect={() => {}} onOpen={() => {}} />
          <InboxRow sessionKey={b.key} selected={false} onSelect={() => {}} onOpen={() => {}} />
        </RouterProvider>
      </AppProvider>,
    );
    // Git summaries come with the fleet (no request per row).
    await waitFor(() => expect(app.store.get().git[a.host]?.[a.snap.cwd!]).toBeDefined());
    expect(backend.requests.some((r) => r.method === "git.status")).toBe(false);
    await act(async () => new Promise((r) => setTimeout(r, 10)));
    const ra = renderCounts.get(`row:${a.key}`)!;
    const rb = renderCounts.get(`row:${b.key}`)!;
    // The host publishes a new snapshot for b only: its agent was working and is interrupted. (A prompt to a
    // busy agent is only queued, which changes nothing a row shows.)
    await act(async () => {
      await backend.request("session.command", { id: b.id, command: { command: "interrupt" } }, { host: b.host });
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(renderCounts.get(`row:${b.key}`)!).toBeGreaterThan(rb);
    expect(renderCounts.get(`row:${a.key}`)).toBe(ra);
  });

  it("selectors group by status with the needs-you sessions first", async () => {
    const { app } = await boot();
    const g = selectGroups(app.store.get(), NO_FILTERS);
    expect(g.needs.length).toBe(4);
    expect(g.failed.length).toBe(2);
    expect(g.done.length).toBe(3);
    expect(g.working.length).toBe(2);
    expect(g.idle.length).toBe(4);
  });

  it("loads transcripts lazily, paged, then follows live events", async () => {
    const { app, backend } = await boot();
    const s = sessionByName(backend, "upgrade-tauri-2-5");
    expect(app.store.get().transcripts[s.key]).toBeUndefined();
    const own = cursorsOf(journal(backend, s.host, s.id), (e) => admits(MAIN, e));
    const total = own.length;
    const want = Math.min(total, RESIDENT);
    const release = app.transcripts.open(s.key);
    const t = () => app.store.get().transcripts[s.key];
    await waitFor(() => expect(t()?.events.length).toBe(want));
    await waitFor(() => expect(t()?.hasMore).toBe(total > RESIDENT));
    expect(t().events.map((e) => e.cursor)).toEqual(own.slice(total - want));
    // The first page is 60 events; each later page is 500 (both fit the byte budget for these events); the pager stops when the list is full.
    const pages = backend.requests.filter((r) => r.method === "session.history" && !(r.params as { earlier?: boolean }).earlier);
    expect(pages).toHaveLength(1 + Math.ceil(Math.max(0, want - 60) / 500));
    // Nothing older is asked for while the session's own events are not all in: the earlier list stays empty.
    expect(t().earlier.events).toHaveLength(0);
    // With the session paged in, "load earlier" is a no-op for the session's own events (only the earlier list can grow).
    const before = t().events.map((e) => e.cursor);
    await app.transcripts.loadEarlier(s.key);
    expect(t().events.map((e) => e.cursor)).toEqual(before);
    // A live event lands after the newest one, and the list follows it exactly.
    await backend.request("session.command", { id: s.id, command: { command: "prompt", text: "hi" } }, { host: s.host });
    const live = cursorsOf(journal(backend, s.host, s.id), (e) => admits(MAIN, e));
    await waitFor(() => expect(t().events[t().events.length - 1].cursor).toBe(live[live.length - 1]));
    expect(t().events.map((e) => e.cursor)).toEqual(live.slice(live.length - t().events.length));
    release();
    app.dispose();
  });

  it("the resident list is capped at RESIDENT: the pager stops at exactly that many, and 'load earlier' then adds one page", async () => {
    // A session with more own events than the resident cap: pushed into the fixture before the app opens (its snapshot covers them).
    localStorage.clear();
    const backend = quietBackend();
    const s = sessionByName(backend, "upgrade-tauri-2-5");
    const events = journal(backend, s.host, s.id);
    const last = events[events.length - 1].cursor;
    const BULK = RESIDENT + 500;
    for (let i = 1; i <= BULK; i++) events.push({ cursor: last + i, t: Date.now(), kind: "agent", event: { type: "assistant_delta", text: `bulk ${i}`, message_id: "m-bulk" } });
    const app = createApp(backend);
    await waitFor(() => expect(app.store.get().sessions[s.key]).toBeDefined());
    const own = cursorsOf(events, (e) => admits(MAIN, e));
    const total = own.length;
    expect(total).toBeGreaterThan(RESIDENT);
    const t = () => app.store.get().transcripts[s.key];
    const release = app.transcripts.open(s.key);
    await waitFor(() => expect(t()?.events.length).toBe(RESIDENT));
    expect(t().hasMore).toBe(true);
    expect(t().events.map((e) => e.cursor)).toEqual(own.slice(total - RESIDENT));
    // The pager asked for exactly the pages that fill the cap: 60, then 500s, the last one sized to the room left.
    const own_pages = backend.requests.filter((r) => r.method === "session.history" && !(r.params as { earlier?: boolean }).earlier);
    expect(own_pages).toHaveLength(1 + Math.ceil((RESIDENT - 60) / 500));
    expect(t().events.length).toBe(RESIDENT);
    // "Load earlier" pages the next 500 of the session's own events, exactly.
    await app.transcripts.loadEarlier(s.key);
    expect(t().events.map((e) => e.cursor)).toEqual(own.slice(total - RESIDENT - 500));
    expect(t().hasMore).toBe(true);
    release();
    app.dispose();
  });

  it("useSessionView is stable for unrelated changes", async () => {
    const { app } = await boot();
    const key = Object.keys(app.store.get().sessions)[0];
    let renders = 0;
    function V() {
      useRenderProbe("v");
      renders++;
      return <span>{useSessionView(key)?.name}</span>;
    }
    render(<AppProvider value={app}><V /></AppProvider>);
    act(() => app.store.set((s) => ({ ...s, lookedAt: s.lookedAt + 1 })));
    expect(renders).toBe(1);
    expect(screen.getByText(/./)).toBeInTheDocument();
  });
});

describe("events of a kind this build does not know", () => {
  it("a fleet event of an unknown kind leaves the state untouched and warns once per kind", async () => {
    const { app } = await boot();
    const before = app.store.get();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const odd = { kind: "from_the_future", host: "x1" } as unknown as FleetEvent;
    expect(applyFleet(before, odd)).toBe(before);
    expect(applyFleet(before, odd)).toBe(before);
    expect(applyFleet(before, { kind: "another_one" } as unknown as FleetEvent)).toBe(before);
    expect(applyFleet(before, undefined as unknown as FleetEvent)).toBe(before);
    expect(warn.mock.calls.map((c) => c[0])).toEqual([expect.stringContaining('"from_the_future"'), expect.stringContaining('"another_one"'), expect.stringContaining('"undefined"')]);
    expect(Object.values(before.sessions).every((x) => x !== undefined)).toBe(true);
    // A workstreams event of an unknown kind is not mistaken for a removal.
    expect(applyWorkstreams(before, { kind: "renamed", id: "x" } as unknown as WorkstreamsEvent)).toBe(before);
    warn.mockRestore();
  });

  it("the store keeps working when the hub sends one", async () => {
    const { app, backend } = await boot();
    const n = Object.keys(app.store.get().sessions).length;
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    // Straight through the subscription the app uses.
    const sub = (backend as unknown as { fleetSubs: Set<(e: unknown) => void> }).fleetSubs;
    await act(async () => sub.forEach((f) => f({ kind: "never_heard_of_it" })));
    expect(Object.keys(app.store.get().sessions)).toHaveLength(n);
    expect(app.store.get().hostOrder.every((h) => app.store.get().hosts[h] !== undefined)).toBe(true);
    warn.mockRestore();
  });
});
