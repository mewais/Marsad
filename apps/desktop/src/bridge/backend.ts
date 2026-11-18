import type { HostId } from "./wire";

export type Unsubscribe = () => void;

export interface RequestOptions {
  /** Which machine's daemon serves it (host services). Omitted = the hub (the app's own machine). */
  host?: HostId;
}

export interface SubscribeOptions extends RequestOptions {
  /** Resume cursor (opaque string for hub/host topics, decimal u64 for session event and pty topics). */
  from?: string | number;
}

/**
 * The whole surface between the UI and the daemon: two primitives. Method and topic names are the
 * ones in marsad-proto / wasit-runtime (see ./wire.ts); payloads are the generated wire types.
 * Components go through the typed layer in ./api.ts. Nothing here blocks the UI: callers fire a
 * request, show a pending state and handle the promise.
 */
export interface Backend {
  request<T = unknown>(method: string, params?: unknown, opts?: RequestOptions): Promise<T>;
  subscribe(topic: string, opts: SubscribeOptions, onEvent: (event: unknown) => void): Unsubscribe;
  /** Desktop shell only: after a notification brings the app forward, the session it was about. */
  onNavigate?(cb: (target: { host: HostId; session: string }) => void): Unsubscribe;
}
