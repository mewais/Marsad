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
 * The app's own link to its local daemon (the hub), as the desktop shell reports it: `connecting`
 * (dialing, starting the daemon if it is not running), `connected`, `reconnecting` (with the
 * last error) or `offline` (gave up; with the reason when there is one).
 */
export interface HubLinkInfo {
  state: "connecting" | "connected" | "reconnecting" | "offline";
  error?: string;
}

/** Why a subscription ended for good (`null`: the topic ended without an error). */
export type SubscriptionEnd = { code: string; message: string; data?: unknown } | null;

/**
 * The whole surface between the UI and the daemon: two primitives. Method and topic names are the
 * ones in marsad-proto / wasit-runtime (see ./wire.ts); payloads are the generated wire types.
 * Components go through the typed layer in ./api.ts. Nothing here blocks the UI: callers fire a
 * request, show a pending state and handle the promise.
 */
export interface Backend {
  request<T = unknown>(method: string, params?: unknown, opts?: RequestOptions): Promise<T>;
  /**
   * `onEnd` is called once when the subscription ends and will deliver nothing more (the daemon
   * refused the topic, the topic ended, or the shell could not start it). A subscription that is
   * merely waiting for a reconnect has not ended: the shell resumes it by itself.
   */
  subscribe(topic: string, opts: SubscribeOptions, onEvent: (event: unknown) => void, onEnd?: (end: SubscriptionEnd) => void): Unsubscribe;
  /** The app's link to the local daemon: delivers the current state at once, then every change. */
  watchHubLink(cb: (link: HubLinkInfo) => void): Unsubscribe;
  /** Connect to the local daemon again now (skips the backoff, and the parked state after a failure). */
  reconnectHub(): Promise<void>;
  /** This app's own version (the shell's, or the package's in a browser). */
  appVersion(): Promise<string>;
  /** Desktop shell only: after a notification brings the app forward, the session it was about. */
  onNavigate?(cb: (target: { host: HostId; session: string }) => void): Unsubscribe;
}
