import { beforeEach, describe, expect, it, vi } from "vitest";
import { TauriBackend } from "../../src/bridge/tauri";

/** What the shell would send over one subscription's channel, and the commands it was asked. */
const shell = vi.hoisted(() => {
  const state = {
    channels: [] as { onmessage: (m: unknown) => void }[],
    calls: [] as { cmd: string; args: unknown }[],
    connection: { daemon: "connecting" } as { daemon: string; daemon_error?: string },
    listeners: new Map<string, (e: { payload: unknown }) => void>(),
    subscribeError: null as unknown,
    unlistened: 0,
    /** When set, `connection_state` answers only once it resolves (a slow read). */
    gate: null as Promise<void> | null,
  };
  return state;
});

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: (m: unknown) => void = () => undefined;
    constructor() {
      shell.channels.push(this);
    }
  },
  invoke: async (cmd: string, args?: unknown) => {
    shell.calls.push({ cmd, args });
    if (cmd === "connection_state") {
      await shell.gate;
      return shell.connection;
    }
    if (cmd === "rpc_subscribe") {
      if (shell.subscribeError) throw shell.subscribeError;
      return 7;
    }
    return null;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, cb: (e: { payload: unknown }) => void) => {
    shell.listeners.set(name, cb);
    return () => {
      shell.unlistened++;
      shell.listeners.delete(name);
    };
  },
}));

const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  shell.channels.length = 0;
  shell.calls.length = 0;
  shell.listeners.clear();
  shell.connection = { daemon: "connecting" };
  shell.subscribeError = null;
  shell.unlistened = 0;
  shell.gate = null;
});

describe("TauriBackend: the app's link to its local daemon", () => {
  it("reads connection_state at start (with the reason) and follows the connection event", async () => {
    shell.connection = { daemon: "offline", daemon_error: "cannot start marsad-agent" };
    const seen: unknown[] = [];
    const off = new TauriBackend().watchHubLink((l) => seen.push(l));
    await settle();
    expect(seen).toEqual([{ state: "offline", error: "cannot start marsad-agent" }]);
    shell.listeners.get("connection")!({ payload: { daemon: "reconnecting", daemon_error: "connection refused", hosts: [] } });
    shell.listeners.get("connection")!({ payload: { daemon: "connected", hosts: [] } });
    expect(seen.slice(1)).toEqual([{ state: "reconnecting", error: "connection refused" }, { state: "connected" }]);
    off();
    expect(shell.unlistened).toBe(1);
  });

  it("an event that arrives while the state is being read is newer: the read does not overwrite it", async () => {
    let release!: () => void;
    shell.gate = new Promise<void>((r) => (release = r));
    const seen: unknown[] = [];
    new TauriBackend().watchHubLink((l) => seen.push(l));
    // The listener is registered before the state is read; an event lands while the (stale) read is still out.
    await vi.waitFor(() => expect(shell.listeners.has("connection")).toBe(true));
    shell.listeners.get("connection")!({ payload: { daemon: "connected", hosts: [] } });
    release();
    await settle();
    expect(seen).toEqual([{ state: "connected" }]);
  });

  it("reconnects through the shell's reconnect_daemon command", async () => {
    await new TauriBackend().reconnectHub();
    expect(shell.calls).toContainEqual({ cmd: "reconnect_daemon", args: undefined });
  });
});

describe("TauriBackend: subscriptions", () => {
  it("passes the topic's parameters (diag.tail's filter) to the shell", async () => {
    new TauriBackend().subscribe("diag.tail", { host: "hz", from: "b1.7", params: { session: "s1", traces: ["u-0123456789abcdef"] } }, () => undefined);
    await vi.waitFor(() => expect(shell.calls.filter((c) => c.cmd === "rpc_subscribe")).toHaveLength(1));
    expect(shell.calls.find((c) => c.cmd === "rpc_subscribe")!.args).toMatchObject({ topic: "diag.tail", host: "hz", from: "b1.7", params: { session: "s1", traces: ["u-0123456789abcdef"] } });
  });

  it("a topic without parameters is subscribed without any", async () => {
    new TauriBackend().subscribe("fleet", {}, () => undefined);
    await vi.waitFor(() => expect(shell.calls.filter((c) => c.cmd === "rpc_subscribe")).toHaveLength(1));
    expect("params" in (shell.calls.find((c) => c.cmd === "rpc_subscribe")!.args as object)).toBe(false);
  });

  it("forwards events, and an `end` message reaches onEnd with its error (it used to be dropped)", async () => {
    const events: unknown[] = [];
    const ends: unknown[] = [];
    new TauriBackend().subscribe("fleet", {}, (e) => events.push(e), (e) => ends.push(e));
    await vi.waitFor(() => expect(shell.channels).toHaveLength(1));
    await settle();
    const ch = shell.channels[0];
    ch.onmessage({ type: "link", state: "connected" });
    ch.onmessage({ type: "event", cursor: "1", payload: { kind: "reset" } });
    ch.onmessage({ type: "end", error: { code: "not_hub", message: "this daemon is not a hub" } });
    expect(events).toEqual([{ kind: "reset" }]);
    expect(ends).toEqual([{ code: "not_hub", message: "this daemon is not a hub" }]);
  });

  it("an end without an error is still an end", async () => {
    const ends: unknown[] = [];
    new TauriBackend().subscribe("fleet", {}, () => undefined, (e) => ends.push(e));
    await vi.waitFor(() => expect(shell.channels).toHaveLength(1));
    shell.channels[0].onmessage({ type: "end" });
    expect(ends).toEqual([null]);
  });

  it("a subscription the shell cannot start ends with its error instead of an unhandled rejection", async () => {
    shell.subscribeError = { code: "transport", message: "ipc failed" };
    const ends: unknown[] = [];
    new TauriBackend().subscribe("fleet", {}, () => undefined, (e) => ends.push(e));
    await vi.waitFor(() => expect(ends).toHaveLength(1));
    expect(ends[0]).toEqual({ code: "transport", message: "ipc failed" });
  });

  it("nothing is delivered after the subscription was closed", async () => {
    const events: unknown[] = [];
    const ends: unknown[] = [];
    const off = new TauriBackend().subscribe("fleet", {}, (e) => events.push(e), (e) => ends.push(e));
    await vi.waitFor(() => expect(shell.channels).toHaveLength(1));
    await settle();
    off();
    shell.channels[0].onmessage({ type: "event", cursor: "1", payload: 1 });
    shell.channels[0].onmessage({ type: "end" });
    expect(events).toEqual([]);
    expect(ends).toEqual([]);
    expect(shell.calls).toContainEqual({ cmd: "rpc_unsubscribe", args: { id: 7 } });
  });
});
