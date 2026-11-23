import type { Backend, HubLinkInfo, RequestOptions, SubscribeOptions, SubscriptionEnd, Unsubscribe } from "./backend";

/**
 * What the shell sends on a subscription's channel (src-tauri/src/bridge.rs `SubMessage`):
 * every event with its cursor, the daemon link's state when it changes (the shell resumes
 * from the last cursor by itself), and a final `end`.
 */
type SubMessage =
  | { type: "event"; cursor: string; payload: unknown }
  | { type: "link"; state: "connected" | "connecting" | "reconnecting" | "offline" }
  | { type: "end"; error?: { code: string; message: string; data?: unknown } };

/** `connection_state` and the `connection` event (src-tauri/src/connection.rs `ConnectionState`; the host list is not used here). */
interface ConnectionState {
  daemon: HubLinkInfo["state"];
  daemon_error?: string;
}

const hubLink = (c: ConnectionState): HubLinkInfo => ({ state: c.daemon, ...(c.daemon_error ? { error: c.daemon_error } : {}) });

/** What a rejected `invoke` carries: the shell's `BridgeError`, or a bare message. */
const asEnd = (e: unknown): NonNullable<SubscriptionEnd> =>
  e && typeof e === "object" && "message" in e ? (e as NonNullable<SubscriptionEnd>) : { code: "transport", message: String(e) };

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

  /** The link to the local daemon: the shell's `connection` events, with `connection_state` as the starting point. */
  watchHubLink(cb: (link: HubLinkInfo) => void): Unsubscribe {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const { listen } = await import("@tauri-apps/api/event");
      // Listen first: an event that arrives while the state is being read is newer than the read.
      let changed = false;
      const un = await listen<ConnectionState>("connection", (e) => {
        changed = true;
        if (!cancelled) cb(hubLink(e.payload));
      });
      stop = un;
      if (cancelled) return un();
      try {
        const now = await invoke<ConnectionState>("connection_state");
        if (!changed && !cancelled) cb(hubLink(now));
      } catch (e) {
        if (!changed && !cancelled) cb({ state: "offline", error: asEnd(e).message });
      }
    })();
    return () => {
      cancelled = true;
      stop?.();
    };
  }

  async appVersion(): Promise<string> {
    const { getVersion } = await import("@tauri-apps/api/app");
    return getVersion();
  }

  async reconnectHub(): Promise<void> {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("reconnect_daemon");
  }

  subscribe(topic: string, opts: SubscribeOptions, onEvent: (e: unknown) => void, onEnd?: (end: SubscriptionEnd) => void): Unsubscribe {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      const { Channel, invoke } = await import("@tauri-apps/api/core");
      const channel = new Channel<SubMessage>();
      channel.onmessage = (m) => {
        if (cancelled) return;
        if (m.type === "event") onEvent(m.payload);
        else if (m.type === "end") onEnd?.(m.error ?? null);
        // `link` messages: the app's link is followed through `watchHubLink`.
      };
      const from = opts.from == null ? null : String(opts.from);
      try {
        const id = await invoke<number>("rpc_subscribe", { topic, host: opts.host ?? null, from, channel });
        stop = () => void invoke("rpc_unsubscribe", { id }).catch(() => undefined);
        if (cancelled) stop();
      } catch (e) {
        // The shell could not even start it: that is an end too, never a silent one.
        if (!cancelled) onEnd?.(asEnd(e));
      }
    })();
    return () => {
      cancelled = true;
      stop?.();
    };
  }
}
