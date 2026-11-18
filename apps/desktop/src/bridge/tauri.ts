import type { Backend, ConnectionState, Unsubscribe } from "./backend";

/**
 * Real backend: the two primitives mapped onto the Tauri shell. Cannot run in a browser or in CI
 * here; the Rust side will expose one `rpc` command and an event channel per subscription.
 * Method/topic names and payloads are the wire types in ./types.ts.
 */
export class TauriBackend implements Backend {
  private state: ConnectionState = { hosts: [] };
  private subs = new Set<(s: ConnectionState) => void>();

  constructor() {
    void this.watchConnections();
  }

  async request<T>(method: string, params?: unknown, opts: { host?: string } = {}): Promise<T> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("rpc", { method, params: params ?? null, host: opts.host ?? null });
  }

  subscribe(topic: string, opts: { host?: string; from?: string | number }, onEvent: (e: unknown) => void): Unsubscribe {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      const { Channel, invoke } = await import("@tauri-apps/api/core");
      const channel = new Channel<unknown>();
      channel.onmessage = onEvent;
      const id = await invoke<number>("rpc_subscribe", { topic, host: opts.host ?? null, from: opts.from ?? null, channel });
      stop = () => void invoke("rpc_unsubscribe", { id });
      if (cancelled) stop();
    })();
    return () => {
      cancelled = true;
      stop?.();
    };
  }

  connection = {
    get: () => this.state,
    subscribe: (cb: (s: ConnectionState) => void): Unsubscribe => {
      this.subs.add(cb);
      return () => this.subs.delete(cb);
    },
  };

  private async watchConnections() {
    const { listen } = await import("@tauri-apps/api/event");
    await listen<ConnectionState>("connection", (e) => {
      this.state = e.payload;
      this.subs.forEach((s) => s(this.state));
    });
  }
}
