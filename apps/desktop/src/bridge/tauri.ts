import type { Backend, RequestOptions, SubscribeOptions, Unsubscribe } from "./backend";

/**
 * Real backend: the two primitives mapped onto the Tauri shell (`rpc`, `rpc_subscribe`,
 * `rpc_unsubscribe`). Method/topic names and payloads are the wire types in ./wire.ts.
 */
export class TauriBackend implements Backend {
  async request<T>(method: string, params?: unknown, opts: RequestOptions = {}): Promise<T> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("rpc", { method, params: params ?? null, host: opts.host ?? null });
  }

  subscribe(topic: string, opts: SubscribeOptions, onEvent: (e: unknown) => void): Unsubscribe {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      const { Channel, invoke } = await import("@tauri-apps/api/core");
      const channel = new Channel<unknown>();
      channel.onmessage = onEvent;
      const from = opts.from == null ? null : String(opts.from);
      const id = await invoke<number>("rpc_subscribe", { topic, host: opts.host ?? null, from, channel });
      stop = () => void invoke("rpc_unsubscribe", { id });
      if (cancelled) stop();
    })();
    return () => {
      cancelled = true;
      stop?.();
    };
  }
}
