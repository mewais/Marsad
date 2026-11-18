import type { Backend, RequestOptions, SubscribeOptions, Unsubscribe } from "./backend";

/**
 * What the shell sends on a subscription's channel (src-tauri/src/bridge.rs `SubMessage`):
 * every event with its cursor, the daemon link's state when it changes (the shell resumes
 * from the last cursor by itself), and a final `end`.
 */
type SubMessage =
  | { type: "event"; cursor: string; payload: unknown }
  | { type: "link"; state: "connected" | "connecting" | "reconnecting" | "offline" }
  | { type: "end"; error?: { code: string; message: string; data?: unknown } };

/**
 * Real backend: the two primitives mapped onto the Tauri shell (`rpc`, `rpc_subscribe`,
 * `rpc_unsubscribe`). Method/topic names and payloads are the wire types in ./wire.ts.
 */
export class TauriBackend implements Backend {
  async request<T>(method: string, params?: unknown, opts: RequestOptions = {}): Promise<T> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>("rpc", { method, params: params ?? null, host: opts.host ?? null });
  }

  onNavigate(cb: (target: { host: string; session: string }) => void): Unsubscribe {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/event").then(({ listen }) =>
      listen<{ host: string; session: string }>("navigate", (e) => cb(e.payload)).then((un) => {
        stop = un;
        if (cancelled) un();
      }),
    );
    return () => {
      cancelled = true;
      stop?.();
    };
  }

  subscribe(topic: string, opts: SubscribeOptions, onEvent: (e: unknown) => void): Unsubscribe {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      const { Channel, invoke } = await import("@tauri-apps/api/core");
      const channel = new Channel<SubMessage>();
      channel.onmessage = (m) => {
        if (m.type === "event") onEvent(m.payload);
      };
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
