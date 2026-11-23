import { beforeEach, describe, expect, it, vi } from "vitest";
import { DIAG_BATCH_MAX, type UiEvent } from "../../src/bridge/backend";
import { createApi, newTraceId } from "../../src/bridge/api";
import { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { TauriBackend } from "../../src/bridge/tauri";

/** The commands the shell was asked, and what it answers. */
const shell = vi.hoisted(() => ({
  calls: [] as { cmd: string; args: Record<string, unknown> }[],
  /** Answers by command; a thrown value is a rejected `invoke`. */
  answer: (_cmd: string, _args: Record<string, unknown>): unknown => null,
}));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {},
  invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
    shell.calls.push({ cmd, args });
    return shell.answer(cmd, args);
  },
}));

const prompt = (n: number): UiEvent => ({ ev: "ui.prompt", trace: "u-0123456789abcdef", host: "local", session: `s-${n}`, kind: "prompt", chars: n, attachments: 0, bytes: 0 });

beforeEach(() => {
  shell.calls.length = 0;
  shell.answer = () => null;
});

describe("traces", () => {
  it("a new trace id is `u-` and 16 hex digits from the platform's random source", () => {
    const spy = vi.spyOn(crypto, "getRandomValues").mockImplementation(((a: Uint8Array) => {
      a.set([0x00, 0x0a, 0xff, 0x10, 0x01, 0xab, 0xcd, 0xef]);
      return a;
    }) as typeof crypto.getRandomValues);
    expect(newTraceId()).toBe("u-000aff1001abcdef");
    expect(spy).toHaveBeenCalledOnce();
    spy.mockRestore();
    const a = newTraceId();
    const b = newTraceId();
    expect(a).toMatch(/^u-[0-9a-f]{16}$/);
    expect(a).not.toBe(b);
  });

  it("rpc carries the trace to the shell, and null when there is none", async () => {
    const b = new TauriBackend();
    await b.request("session.command", { id: "s" }, { host: "gpu", trace: "u-0123456789abcdef" });
    await b.request("store.settings.get");
    expect(shell.calls).toEqual([
      { cmd: "rpc", args: { method: "session.command", params: { id: "s" }, host: "gpu", trace: "u-0123456789abcdef" } },
      { cmd: "rpc", args: { method: "store.settings.get", params: null, host: null, trace: null } },
    ]);
  });

  it("a typed call passes its options, trace included, through to the backend", async () => {
    const backend = new FixtureBackend({ live: false, latencyMs: 0, speed: 0 });
    const trace = newTraceId();
    await backend.request("host.info", undefined, { host: "x1", trace });
    await backend.request("daemon.info");
    expect(backend.requests.map((r) => r.trace)).toEqual([trace, undefined]);
    backend.dispose();
  });
});

describe("diagLog", () => {
  it("sends the events to the shell's diag_log command", async () => {
    const events: UiEvent[] = [prompt(1), { ev: "ui.render_error", component: "Transcript" }, { ev: "ui.export", stage: "saved" }];
    await new TauriBackend().diagLog(events);
    expect(shell.calls).toEqual([{ cmd: "diag_log", args: { events } }]);
  });

  it("sends nothing for no events", async () => {
    await new TauriBackend().diagLog([]);
    expect(shell.calls).toEqual([]);
  });

  it("splits a long list at the shell's limit, in order", async () => {
    const events = Array.from({ length: 2 * DIAG_BATCH_MAX + 50 }, (_, i) => prompt(i));
    await new TauriBackend().diagLog(events);
    expect(DIAG_BATCH_MAX).toBe(100);
    expect(shell.calls.map((c) => (c.args.events as UiEvent[]).length)).toEqual([100, 100, 50]);
    expect(shell.calls.flatMap((c) => c.args.events as UiEvent[])).toEqual(events);
  });

  it("rejects with the shell's error when it refuses an event (the caller's bug is not swallowed)", async () => {
    shell.answer = () => {
      throw { code: "invalid_params", message: "1 of 1 diagnostics events refused: a field the event does not declare" };
    };
    await expect(new TauriBackend().diagLog([prompt(1)])).rejects.toMatchObject({ code: "invalid_params" });
  });

  it("the api's diagLog is the backend's", async () => {
    await createApi(new TauriBackend()).diagLog([prompt(1)]);
    expect(shell.calls.map((c) => c.cmd)).toEqual(["diag_log"]);
  });

  it("the fixture records what was sent, as sent", async () => {
    const backend = new FixtureBackend({ live: false, latencyMs: 0, speed: 0 });
    const api = createApi(backend);
    const e = prompt(1);
    await api.diagLog([e]);
    await api.diagLog([{ ev: "ui.developer_mode", on: true }]);
    expect(backend.diagEvents).toEqual([e, { ev: "ui.developer_mode", on: true }]);
    // A copy: what the UI mutates afterwards is not what was recorded.
    (e as { chars: number }).chars = 99;
    expect((backend.diagEvents[0] as { chars: number }).chars).toBe(1);
    backend.dispose();
  });
});

describe("exportSave", () => {
  it("asks the shell's desktop.export_save for the export by its file name", async () => {
    shell.answer = () => ({ path: "/home/me/Downloads/marsad-diag.ndjson.gz" });
    const r = await createApi(new TauriBackend()).desktop.exportSave("marsad-diag.ndjson.gz");
    expect(r).toEqual({ path: "/home/me/Downloads/marsad-diag.ndjson.gz" });
    expect(shell.calls).toEqual([{ cmd: "rpc", args: { method: "desktop.export_save", params: { id: "marsad-diag.ndjson.gz" }, host: null, trace: null } }]);
  });

  it("a cancelled dialog answers {saved: false}", async () => {
    shell.answer = () => ({ saved: false });
    expect(await createApi(new TauriBackend()).desktop.exportSave("x.gz")).toEqual({ saved: false });
  });

  it("the fixture saves to Downloads, or reports a cancel, and records the request", async () => {
    const backend = new FixtureBackend({ live: false, latencyMs: 0, speed: 0 });
    const api = createApi(backend);
    expect(await api.desktop.exportSave("e.gz")).toEqual({ path: "/home/me/Downloads/e.gz" });
    backend.exportSaveAnswer = () => ({ saved: false });
    expect(await api.desktop.exportSave("e.gz")).toEqual({ saved: false });
    expect(backend.requests.filter((r) => r.method === "desktop.export_save").map((r) => r.params)).toEqual([{ id: "e.gz" }, { id: "e.gz" }]);
    backend.dispose();
  });
});
