import { act, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { createStore, shallowEqual, useStore } from "../../src/lib/store";
import { AppProvider, createApp, renderCounts, selectGroups, NO_FILTERS, useSessionView, useRenderProbe } from "../../src/state";
import { InboxRow } from "../../src/features/inbox/InboxRow";
import { memoryHistory, RouterProvider } from "../../src/router";
import { applyFleet, applyWorkstreams } from "../../src/state/controller";
import type { FleetEvent, WorkstreamsEvent } from "../../src/bridge/wire";
import { quietBackend, sessionByName } from "../helpers";

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
    const release = app.transcripts.open(s.key);
    await waitFor(() => expect(app.store.get().transcripts[s.key]?.events.length).toBe(60));
    expect(app.store.get().transcripts[s.key].hasMore).toBe(true);
    await app.transcripts.loadEarlier(s.key);
    expect(app.store.get().transcripts[s.key].events.length).toBeGreaterThan(60);
    const n = app.store.get().transcripts[s.key].events.length;
    await backend.request("session.command", { id: s.id, command: { command: "prompt", text: "hi" } }, { host: s.host });
    await waitFor(() => expect(app.store.get().transcripts[s.key].events.length).toBeGreaterThan(n));
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
