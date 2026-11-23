import type { HostId, UiEvent } from "./wire";

export type Unsubscribe = () => void;

export interface RequestOptions {
  /** Which machine's daemon serves it (host services). Omitted = the hub (the app's own machine). */
  host?: HostId;
  /**
   * The id of the user action this request serves (`newTraceId()`, `u-` + 16 hex). It travels with
   * the request through the hub to the host that serves it, and every hop records it, so an action
   * can be followed end to end in the diagnostics. Not for `desktop.*` methods (answered by the
   * shell itself, no hop to trace). An id that is not one is refused by the shell.
   */
  trace?: string;
}

/** The most events the shell takes in one `diag_log` call (`diagLog` splits longer lists). */
export const DIAG_BATCH_MAX = 100;

export interface SubscribeOptions extends RequestOptions {
  /** Resume cursor (opaque string for hub/host topics, decimal u64 for session event and pty topics). */
  from?: string | number;
  /**
   * The topic's parameters (`diag.tail`'s filter). Only topics that take parameters are given
   * any. A consumer that needs the result to match them must check it itself: a transport that
   * does not carry them delivers the unfiltered topic.
   */
  params?: unknown;
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
  /**
   * Record events in the app's own diagnostics log (the desktop shell; a browser has none and
   * ignores them). Resolves once the shell has taken them (it never waits for the disk); rejects
   * when it refused one, which is a bug of the caller. Lists longer than `DIAG_BATCH_MAX` are split.
   */
  diagLog(events: readonly UiEvent[]): Promise<void>;
  /** The app's link to the local daemon: delivers the current state at once, then every change. */
  watchHubLink(cb: (link: HubLinkInfo) => void): Unsubscribe;
  /** Connect to the local daemon again now (skips the backoff, and the parked state after a failure). */
  reconnectHub(): Promise<void>;
  /** This app's own version (the shell's, or the package's in a browser). */
  appVersion(): Promise<string>;
  /** Desktop shell only: after a notification brings the app forward, the session it was about. */
  onNavigate?(cb: (target: { host: HostId; session: string }) => void): Unsubscribe;
}
