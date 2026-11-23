import type { HubLinkInfo } from "../bridge/backend";
import type { DriverDescriptor } from "../ui/DriverBadge";
import type { DriverInventory, GitSummary, HostId, NotificationPrefs, SeenMark, SessionEvent, SessionKey, SessionSnapshot, WorkstreamView } from "../bridge/wire";
import { createStore } from "../lib/store";
import type { HostState } from "./hosts";

export interface TranscriptSlice {
  /** Oldest first; contiguous from `events[0]` to the live edge. */
  events: SessionEvent[];
  /** Older events exist on the host. */
  hasMore: boolean;
  loading: boolean;
  error?: string;
}

export type PendingState = "pending" | "sent";

/** The app's own link to its local daemon (what the shell reports). */
export type HubLink = HubLinkInfo;

/** A document the hub holds, as loaded here: never shown as its default until it has arrived. */
export type Load = { state: "loading" } | { state: "ready" } | { state: "error"; error: string };
export type LoadKey = "settings" | "notifications" | "seen";
export const LOADING: Load = { state: "loading" };
export const READY: Load = { state: "ready" };

export interface AppState {
  /** The first `fleet` reset has arrived. */
  fleetReady: boolean;
  /** The link between this app and its local daemon. Everything below is only as live as this. */
  hub: HubLink;
  /**
   * Subscriptions that ended for good (the daemon refused the topic, or it closed), by name
   * (`fleet`, `workstreams`, `inventory:<host>`, `events:<session>`), with the reason.
   */
  feedErrors: Record<string, string>;
  /** Where the stored documents below are: loading, loaded, or failed with the reason. */
  loads: Record<LoadKey, Load>;
  hostOrder: HostId[];
  hosts: Record<HostId, HostState>;
  /** `drivers.inventory` per host (subscribed while the host is connected). */
  inventory: Record<HostId, DriverInventory[]>;
  /** Presentation of every driver seen in an inventory or a session (derived from the id). */
  drivers: Record<string, DriverDescriptor>;
  driverOrder: string[];
  sessions: Record<SessionKey, SessionSnapshot>;
  transcripts: Record<SessionKey, TranscriptSlice>;
  /** `store.seen.*` markers. */
  seen: Record<SessionKey, SeenMark>;
  /** Valid once `loads.seen` is ready. Latest time the human marked something seen (for "finished since you looked at"). */
  lookedAt: number;
  /** In-flight and just-sent actions, keyed by `kind:...`. */
  pending: Record<string, PendingState>;
  /** The hub's background git summaries (fleet topic), per host and working directory. */
  git: Record<HostId, Record<string, GitSummary>>;
  /** The hub's `workstreams` topic: every workstream with live nodes, edges and tasks. */
  workstreams: Record<string, WorkstreamView>;
  wsOrder: string[];
  workstreamsLoaded: boolean;
  /** Valid once `loads.notifications` is ready. */
  notifications: NotificationPrefs | null;
  /** Valid once `loads.settings` is ready; written only from the daemon's replies. */
  settings: Record<string, unknown>;
}

export function initialState(partial: Partial<AppState> = {}): AppState {
  return {
    fleetReady: false,
    hub: { state: "connecting" },
    feedErrors: {},
    loads: { settings: LOADING, notifications: LOADING, seen: LOADING },
    hostOrder: [],
    hosts: {},
    inventory: {},
    drivers: {},
    driverOrder: [],
    sessions: {},
    transcripts: {},
    seen: {},
    lookedAt: 0,
    pending: {},
    git: {},
    workstreams: {},
    wsOrder: [],
    workstreamsLoaded: false,
    notifications: null,
    settings: {},
    ...partial,
  };
}

export type AppStore = ReturnType<typeof createAppStore>;
export const createAppStore = (partial?: Partial<AppState>) => createStore<AppState>(initialState(partial));
