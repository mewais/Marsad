import { waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { PendingFile } from "../../src/features/session/attachments";
import { launchWithAttachments } from "../../src/features/newsession/launch";
import type { LaunchRequest, UiEvent } from "../../src/bridge/wire";
import { APP_EVENTS_KEEP, createApp } from "../../src/state";
import { linesOf } from "../../src/bridge/fixture/diagnostics";
import { hold, quietBackend, sessionByName } from "../helpers";

const SECRET = "SECRET-MARKER-9f3a";

async function boot(backend = quietBackend()) {
  localStorage.clear();
  const app = createApp(backend);
  await waitFor(() => expect(Object.keys(app.store.get().sessions).length).toBe(15));
  /** Everything the app has recorded of itself so far, as the shell received it (after the batch of the action has gone out). */
  const sent = async (n: number) => {
    await waitFor(() => expect(backend.diagEvents.length).toBeGreaterThanOrEqual(n));
    return backend.diagEvents;
  };
  return { backend, app, sent };
}

const traced = (backend: ReturnType<typeof quietBackend>, method: string) => backend.requests.filter((r) => r.method === method);
const file = (name: string, size: number): PendingFile => ({ key: name, name, size, mime: "image/png", read: async (_o, n) => new Uint8Array(Math.min(n, size)) });

describe("every user action that sends to a session or launches one is a traced action", () => {
  it("a prompt: one trace id on the request, ui.prompt with its size and never its text, then the outcome of the call", async () => {
    const { backend, app, sent } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    const text = `héllo 🌍 ${SECRET}`;
    expect(await app.actions.sendPrompt(s.key, text)).toBe(true);
    const events = await sent(2);
    const [req] = traced(backend, "session.command");
    expect(req.trace).toMatch(/^u-[0-9a-f]{16}$/);
    expect(events).toEqual([
      { ev: "ui.prompt", trace: req.trace, host: s.host, session: s.id, kind: "prompt", chars: [...text].length, attachments: 0, bytes: 0 },
      { ev: "ui.rpc", trace: req.trace, method: "session.command", ms: expect.any(Number), outcome: "ok" },
    ]);
    // Counted, never kept: nothing the user typed is in anything recorded.
    expect(JSON.stringify([events, app.store.get().appEvents])).not.toContain(SECRET);
    expect(JSON.stringify(events)).not.toContain("héllo");
  });

  it("a prompt with files: counted by number and size, uploaded and timed, the same trace on the command", async () => {
    const { backend, app, sent } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    const files = [file("a.png", 1000), file("b.png", 2500)];
    const ok = await app.actions.sendPrompt(s.key, "with files", {
      count: files.length,
      bytes: 3500,
      upload: async () => files.map((f) => ({ name: f.name, mime: f.mime, size: f.size, path: `/a/${f.name}` })),
    });
    expect(ok).toBe(true);
    const events = await sent(3);
    const [req] = traced(backend, "session.command");
    expect(events.map((e) => e.ev)).toEqual(["ui.prompt", "ui.upload", "ui.rpc"]);
    expect(events[0]).toMatchObject({ trace: req.trace, attachments: 2, bytes: 3500, chars: 10 });
    expect(events[1]).toMatchObject({ trace: req.trace, files: 2, bytes: 3500, outcome: "ok", ms: expect.any(Number) });
    expect((req.params as { command: { attachments: unknown[] } }).command.attachments).toHaveLength(2);
  });

  it("a failed upload is recorded with its outcome and the prompt is not sent", async () => {
    const { backend, app, sent } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    const ok = await app.actions.sendPrompt(s.key, "x", { count: 1, bytes: 10, upload: async () => Promise.reject({ code: "too_large", message: "no" }) });
    expect(ok).toBe(false);
    const events = await sent(2);
    expect(events[1]).toMatchObject({ ev: "ui.upload", outcome: "error", code: "too_large" });
    expect(traced(backend, "session.command")).toHaveLength(0);
  });

  it("slash commands are prompts of kind slash, counted by their arguments; the other commands say which", async () => {
    const { backend, app, sent } = await boot();
    const s = sessionByName(backend, "migrate-auth-middleware");
    await app.actions.command(s.key, { command: "run_slash", name: "review", args: "the diff" });
    await app.actions.command(s.key, { command: "set_model", model: "claude-haiku-4.5" });
    await app.actions.command(s.key, { command: "set_mode", mode: "plan" });
    await app.actions.answerGate(s.key, "p-prisma", { choice: "allow" });
    await app.actions.answerQuestion(s.key, "q-cookies", { answers: { q: [SECRET] } });
    await app.actions.interrupt(s.key);
    const ui = (await sent(12)).filter((e): e is Exclude<UiEvent, { ev: "ui.rpc" }> => e.ev !== "ui.rpc");
    expect(ui.map((e) => (e.ev === "ui.prompt" ? `prompt:${e.kind}:${e.chars}` : e.ev === "ui.command" ? e.command : e.ev))).toEqual(["prompt:slash:8", "set_model", "set_mode", "answer_gate", "answer_question", "interrupt"]);
    expect(JSON.stringify(ui)).not.toContain(SECRET);
    // Each is its own action with its own trace, and the request carries it.
    const reqTraces = traced(backend, "session.command").map((r) => r.trace);
    expect(new Set(reqTraces).size).toBe(6);
    expect(ui.map((e) => (e as { trace: string }).trace)).toEqual(reqTraces);
  });

  it("stopping and continuing a session are traced too", async () => {
    const { backend, app, sent } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    await app.actions.endSession(s.key);
    await app.actions.continueSession(s.key);
    const events = await sent(4);
    expect(events.filter((e) => e.ev === "ui.command").map((e) => (e as { command: string }).command)).toEqual(["stop", "continue"]);
    expect(traced(backend, "sessions.continue")[0].trace).toBeTruthy();
    expect(traced(backend, "sessions.stop")[0].trace).toBe((events.find((e) => e.ev === "ui.command" && e.command === "stop") as { trace: string }).trace);
  });

  it("a launch: ui.launch with what was asked (never the prompt), the trace on the request, the outcome", async () => {
    const { backend, app, sent } = await boot();
    const request: LaunchRequest = { host: "hz", driver: "claude", cwd: "~/src/ledgerline", model: null, mode: null, first_prompt: SECRET, resume: { kind: "latest" }, labels: {}, workstream: null, overrides: { items: [], env: [], disallowed_tools: [] } };
    const reply = await app.actions.launch("ns", request);
    expect(reply?.host).toBe("hz");
    const events = await sent(2);
    const [req] = traced(backend, "sessions.launch");
    expect(events).toEqual([
      { ev: "ui.launch", trace: req.trace, host: "hz", driver: "claude", resume: "continue", first_prompt: true, attachments: 0 },
      { ev: "ui.rpc", trace: req.trace, method: "sessions.launch", ms: expect.any(Number), outcome: "ok" },
    ]);
    expect(JSON.stringify(events)).not.toContain(SECRET);
    // The daemon's own view of the same request: the hub recorded the trace and the session it started.
    expect(linesOf(backend.hubLog).map((l) => JSON.parse(l)).filter((r) => r.trace === req.trace).map((r) => r.ev)).toEqual(["rpc.traced", "launch"]);
  });

  it("a launch with files: one trace for the launch, the upload and the prompt", async () => {
    const { backend, app, sent } = await boot();
    const request: LaunchRequest = { host: "x1", driver: "claude", cwd: "~/src/ledgerline", model: null, mode: null, first_prompt: null, resume: { kind: "new" }, labels: {}, workstream: null, overrides: { items: [], env: [], disallowed_tools: [] } };
    const out = await launchWithAttachments(app.api, request, "see the pictures", [file("a.png", 100)], () => undefined, app.actions.trace());
    expect(out.ok).toBe(true);
    const events = await sent(5);
    const traces = new Set(events.map((e) => (e as { trace?: string }).trace));
    expect(traces.size).toBe(1);
    expect(events.map((e) => e.ev)).toEqual(["ui.launch", "ui.rpc", "ui.upload", "ui.prompt", "ui.rpc"]);
    expect(events[0]).toMatchObject({ first_prompt: true, attachments: 1, resume: "fresh" });
    expect(events[3]).toMatchObject({ kind: "prompt", chars: 16, attachments: 1, bytes: 100 });
    // The uploads' chunks are not traced requests of their own.
    expect(traced(backend, "session.attachment.put").every((r) => r.trace === undefined)).toBe(true);
    expect(traced(backend, "sessions.launch")[0].trace).toBe(traced(backend, "session.command")[0].trace);
  });

  it("the outcome of a call that failed is recorded with its code", async () => {
    const { backend, app, sent } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    const h = hold(backend, "session.command");
    h.failAll({ code: "timeout", message: "the host did not answer" });
    expect(await app.actions.sendPrompt(s.key, "x")).toBe(false);
    const events = await sent(2);
    expect(events.map((e) => e.ev)).toEqual(["ui.prompt", "ui.rpc"]);
    expect(events[1]).toMatchObject({ method: "session.command", outcome: "error", code: "timeout" });
  });

  it("an error from the daemon is a ui.rpc with outcome error and the code", async () => {
    const { backend, app, sent } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    await app.actions.endSession(s.key);
    // A prompt continues an ended session first; the host cannot start the agent again, so it refuses the prompt.
    backend.failNextContinue(s.host, s.id, "the agent could not be started");
    expect(await app.actions.sendPrompt(s.key, "again")).toBe(false);
    const events = await sent(4);
    expect(events.filter((e) => e.ev === "ui.rpc")[1]).toMatchObject({ method: "session.command", outcome: "error", code: "continue_failed" });
  });

  it("a failure that is not an RPC error is an error outcome without a code; a code that is not shaped like one is not passed on", async () => {
    const { backend, app, sent } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    const request = backend.request.bind(backend);
    let failure: unknown = new Error("socket closed");
    backend.request = (<T,>(m: string, p?: unknown, o?: object) => (m === "session.command" ? Promise.reject(failure) : request<T>(m, p, o))) as typeof backend.request;
    await app.actions.sendPrompt(s.key, "a");
    failure = { code: "has spaces and /slashes", message: "x" };
    await app.actions.sendPrompt(s.key, "b");
    const rpc = (await sent(4)).filter((e) => e.ev === "ui.rpc");
    expect(rpc).toEqual([
      { ev: "ui.rpc", trace: expect.any(String), method: "session.command", ms: expect.any(Number), outcome: "error" },
      { ev: "ui.rpc", trace: expect.any(String), method: "session.command", ms: expect.any(Number), outcome: "error" },
    ]);
  });

  it("an action refused before it is sent (the host is not connected) leaves nothing recorded and sends nothing", async () => {
    const { backend, app } = await boot();
    const s = sessionByName(backend, "ws-reconnect-backoff");
    backend.setLink(s.host, { state: "reconnecting", attempt: 1, next_at: { secs_since_epoch: 9_999_999_999, nanos_since_epoch: 0 }, last_error: "x" });
    await waitFor(() => expect(app.store.get().hosts[s.host].link).toMatchObject({ link: "remote", state: { state: "reconnecting" } }));
    expect(await app.actions.sendPrompt(s.key, "x")).toBe(false);
    expect(traced(backend, "session.command")).toHaveLength(0);
    expect(backend.diagEvents).toEqual([]);
    expect(app.store.get().appEvents).toEqual([]);
  });

  it("requests that are not user actions carry no trace and leave no ui.rpc", async () => {
    const { backend, app } = await boot();
    await app.api.hostInfo("hz");
    expect(backend.requests.every((r) => r.trace === undefined)).toBe(true);
    expect(backend.diagEvents).toEqual([]);
  });
});

describe("the app's own log never gets in the way", () => {
  it("events of one moment go to the shell together, after the action that made them", async () => {
    const { backend, app } = await boot();
    const log = vi.spyOn(backend, "diagLog");
    app.actions.record({ ev: "ui.developer_mode", on: true });
    app.actions.record({ ev: "ui.developer_mode", on: false });
    app.actions.record({ ev: "ui.export", stage: "start" });
    expect(log).not.toHaveBeenCalled();
    await waitFor(() => expect(log).toHaveBeenCalledTimes(1));
    expect(log.mock.calls[0][0]).toHaveLength(3);
  });

  it("a shell that refuses or cannot take the events fails nothing: the action completes, and the failures are counted with the reason", async () => {
    const { backend, app } = await boot();
    const s = sessionByName(backend, "docs-search-index");
    backend.diagLog = () => Promise.reject({ code: "invalid_params", message: "1 of 1 diagnostics events refused: a field the event does not declare" });
    expect(await app.actions.sendPrompt(s.key, "x")).toBe(true);
    await waitFor(() => expect(app.store.get().diagLog.failed).toBeGreaterThanOrEqual(1));
    expect(app.store.get().diagLog.error).toContain("refused");
    // Still shown: the events are the app's own record whether or not the shell kept them.
    expect(app.store.get().appEvents.some((e) => e.event.ev === "ui.prompt")).toBe(true);
    // A shell that throws at once is the same.
    const before = app.store.get().diagLog.failed;
    backend.diagLog = () => {
      throw new Error("shell is gone");
    };
    app.actions.record({ ev: "ui.export", stage: "start" });
    await waitFor(() => expect(app.store.get().diagLog.failed).toBe(before + 1));
    expect(app.store.get().diagLog.error).toBe("shell is gone");
  });

  it("the store keeps the newest few hundred, oldest first, each with when", async () => {
    const { app } = await boot();
    for (let i = 0; i < APP_EVENTS_KEEP + 25; i++) app.actions.record({ ev: "ui.rpc", trace: `u-${String(i).padStart(10, "0")}`, method: "m", ms: i, outcome: "ok" });
    const kept = app.store.get().appEvents;
    expect(kept).toHaveLength(APP_EVENTS_KEEP);
    expect((kept[0].event as { ms: number }).ms).toBe(25);
    expect((kept[kept.length - 1].event as { ms: number }).ms).toBe(APP_EVENTS_KEEP + 24);
    expect(kept.every((e) => typeof e.t === "number" && e.t > 0)).toBe(true);
  });
});
