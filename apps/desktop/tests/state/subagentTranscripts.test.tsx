import { act, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { BACKGROUND_SPEC } from "../../src/bridge/fixture/background";
import { SPECS } from "../../src/bridge/fixture/sessions";
import type { SessionEvent } from "../../src/bridge/wire";
import { createApp, MAIN, scopeKey, subagentScope } from "../../src/state";
import { hold, quietBackend, sessionByName } from "../helpers";

const BILLING = { call_id: "c-bill", agent_id: "agent-bill-1" };
const text = (t: string, id: string) => ({ type: "assistant_delta" as const, text: t, message_id: id });

async function boot() {
  localStorage.clear();
  const backend = quietBackend({ specs: [...SPECS, BACKGROUND_SPEC] });
  const app = createApp(backend);
  const s = sessionByName(backend, "route-auth-audit");
  await waitFor(() => expect(app.store.get().sessions[s.key]).toBeDefined());
  return { backend, app, s, work: backend.work(s.host, s.id) };
}
const slice = (app: Awaited<ReturnType<typeof boot>>["app"], key: string, c = "c-bill") => app.store.get().transcripts[scopeKey(key, subagentScope(c))];
const subscriptions = (b: Awaited<ReturnType<typeof boot>>["backend"]) => b.subscriptions.filter((x) => x.topic.startsWith("session.events:"));
const texts = (events: readonly SessionEvent[] = []) => events.flatMap((e) => (e.kind === "agent" && e.event.type === "scoped" && e.event.event.type === "assistant_delta" ? [e.event.event.text] : []));

describe("a subagent's transcript in the store", () => {
  it("is its scoped history, then what the session's one subscription delivers for it; the session's own never holds it", async () => {
    const { app, s, work, backend } = await boot();
    const close = app.transcripts.open(s.key, subagentScope("c-bill"));
    await waitFor(() => expect(slice(app, s.key)?.loading).toBe(false));
    expect(texts(slice(app, s.key).events)[0]).toMatch(/billing module/);
    await waitFor(() => expect(subscriptions(backend)).toHaveLength(1));
    act(() => work.scoped(BILLING, text("a fresh finding", "z1")));
    await waitFor(() => expect(texts(slice(app, s.key).events)).toContain("a fresh finding"));
    // Not the session's own transcript, which has its own slice, nor another subagent's.
    expect(texts(app.store.get().transcripts[s.key].events)).toEqual([]);
    expect(app.store.get().transcripts[s.key].events.every((e) => e.kind !== "agent" || e.event.type !== "scoped")).toBe(true);
    close();
  });

  it("events that arrive while its history is being fetched are kept in order, without a gap and without a repeat", async () => {
    const { app, s, work, backend } = await boot();
    app.transcripts.open(s.key, MAIN);
    await waitFor(() => expect(app.store.get().transcripts[s.key]?.loading).toBe(false));
    await waitFor(() => expect(subscriptions(backend)).toHaveLength(1));
    const h = hold(backend, "session.history");
    const close = app.transcripts.open(s.key, subagentScope("c-bill"));
    await waitFor(() => expect(h.pending()).toBe(1));
    act(() => {
      work.scoped(BILLING, text("while loading 1", "w1"));
      work.scoped(BILLING, text("while loading 2", "w2"));
    });
    // The fixture serves the page when it is released: it already holds both, and the live copies are the same events.
    await act(async () => h.release());
    await waitFor(() => expect(slice(app, s.key)?.loading).toBe(false));
    const got = texts(slice(app, s.key).events);
    expect(got.filter((t) => t.startsWith("while loading"))).toEqual(["while loading 1", "while loading 2"]);
    const cursors = slice(app, s.key).events.map((e) => e.cursor);
    expect(cursors).toEqual([...new Set(cursors)].sort((a, b) => a - b));
    // Something that happens after the page was read still arrives.
    act(() => work.scoped(BILLING, text("after", "w3")));
    await waitFor(() => expect(texts(slice(app, s.key).events).slice(-1)[0]).toBe("after"));
    close();
  });

  it("an event newer than the page that was read, delivered before the page arrives, is appended after it", async () => {
    const { app, s, work, backend } = await boot();
    app.transcripts.open(s.key, MAIN);
    await waitFor(() => expect(subscriptions(backend)).toHaveLength(1));
    // The daemon answers with the page as it is now; the answer reaches the app after a newer line has already been delivered live.
    const request = backend.request.bind(backend);
    let answer: (() => void) | undefined;
    backend.request = (<T,>(m: string, params?: unknown, opts?: object) => {
      const p = request<T>(m, params, opts);
      if (m !== "session.history" || (params as { scope?: { kind: string } }).scope?.kind !== "subagent") return p;
      return new Promise<T>((resolve, reject) => {
        answer = () => p.then(resolve, reject);
      });
    }) as typeof backend.request;
    const close = app.transcripts.open(s.key, subagentScope("c-bill"));
    await waitFor(() => expect(answer).toBeDefined());
    await act(async () => new Promise((r) => setTimeout(r, 5))); // the daemon has answered; the page is fixed
    act(() => work.scoped(BILLING, text("newer than the page", "n1")));
    await waitFor(() => expect(app.store.get().transcripts[s.key].events.length).toBeGreaterThan(0));
    expect(slice(app, s.key)?.loading).toBe(true);
    await act(async () => answer!());
    await waitFor(() => expect(slice(app, s.key)?.loading).toBe(false));
    expect(texts(slice(app, s.key).events).slice(-1)[0]).toBe("newer than the page");
    const cursors = slice(app, s.key).events.map((e) => e.cursor);
    expect(cursors).toEqual([...new Set(cursors)].sort((a, b) => a - b));
    close();
  });

  it("is kept only while it is open: closing frees it and the session's subscription goes when its last user does", async () => {
    const { app, s, backend } = await boot();
    const closeMain = app.transcripts.open(s.key, MAIN);
    const closeSub = app.transcripts.open(s.key, subagentScope("c-bill"));
    const again = app.transcripts.open(s.key, subagentScope("c-bill"));
    await waitFor(() => expect(slice(app, s.key)?.loading).toBe(false));
    expect(subscriptions(backend)).toHaveLength(1);
    closeSub();
    expect(slice(app, s.key)).toBeDefined(); // still open once
    again();
    expect(slice(app, s.key)).toBeUndefined();
    expect(subscriptions(backend)).toHaveLength(1); // the session's own transcript still uses it
    closeMain();
    expect(subscriptions(backend)).toHaveLength(0);
    // Open again: loaded afresh, not from what was left.
    const reopened = app.transcripts.open(s.key, subagentScope("c-bill"));
    await waitFor(() => expect(slice(app, s.key)?.loading).toBe(false));
    reopened();
  });

  it("two subagents are two transcripts", async () => {
    const { app, s, work } = await boot();
    const a = app.transcripts.open(s.key, subagentScope("c-bill"));
    const b = app.transcripts.open(s.key, subagentScope("c-inv"));
    await waitFor(() => expect(slice(app, s.key, "c-inv")?.loading).toBe(false));
    expect(texts(slice(app, s.key, "c-inv").events).join(" ")).toMatch(/invoice route/);
    expect(texts(slice(app, s.key, "c-inv").events).join(" ")).not.toMatch(/billing module/);
    act(() => work.scoped({ call_id: "c-inv" }, text("invoice only", "q1")));
    await waitFor(() => expect(texts(slice(app, s.key, "c-inv").events)).toContain("invoice only"));
    expect(texts(slice(app, s.key, "c-bill").events)).not.toContain("invoice only");
    a();
    b();
  });

  it("when the session's events cannot be read, it says so instead of showing nothing", async () => {
    const { app, s, backend } = await boot();
    const h = hold(backend, "session.history");
    h.failAll({ code: "not_found", message: "no such session", data: null });
    const close = app.transcripts.open(s.key, subagentScope("c-bill"));
    await waitFor(() => expect(slice(app, s.key)?.error).toBeTruthy());
    expect(slice(app, s.key).events).toEqual([]);
    close();
  });
});
