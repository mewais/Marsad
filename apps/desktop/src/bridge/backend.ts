import type { HostId, HostLink } from "./types";

export type Unsubscribe = () => void;

export interface RequestOptions {
  /** Which machine to ask. Omitted = the app's own (local) host. */
  host?: HostId;
}

export interface SubscribeOptions extends RequestOptions {
  /** Resume cursor (opaque string for host topics, decimal u64 for session event topics). */
  from?: string | number;
}

/** What the UI knows about the connections to every host. Not a wire type. */
export interface ConnectionState {
  hosts: HostLink[];
}

/**
 * The whole surface between the UI and the runtime: two primitives plus connection state.
 * Methods and topics are the names in wire.rs (`session.command`, `host.sessions`, ...);
 * payloads are the types in ./types.ts. Nothing here blocks the UI: callers fire a request,
 * show a pending state and handle the promise.
 */
export interface Backend {
  request<T = unknown>(method: string, params?: unknown, opts?: RequestOptions): Promise<T>;
  subscribe(
    topic: string,
    opts: SubscribeOptions,
    onEvent: (event: unknown) => void,
  ): Unsubscribe;
  connection: {
    get(): ConnectionState;
    subscribe(cb: (s: ConnectionState) => void): Unsubscribe;
  };
}
