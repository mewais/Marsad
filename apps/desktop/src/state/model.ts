import type { HubLinkInfo } from "../bridge/backend";
import type { DriverDescriptor } from "../ui/DriverBadge";
import type { DiagTailParams, DriverInventory, ExportStatus, GitSummary, HostId, NotificationPrefs, SeenMark, SessionEvent, SessionKey, SessionSnapshot, TrailItem, UiEvent, WorkstreamView } from "../bridge/wire";
import type { DiagRecord, TailKeep } from "../lib/diagRecord";
import { createStore } from "../lib/store";
import type { HostState } from "./hosts";

/** One list of a transcript's events as loaded here. */
export interface EventList {
  /** Oldest first; contiguous from `events[0]` to the newest the list holds. */
  events: SessionEvent[];
  /** Older events exist on the host. */
  hasMore: boolean;
  loading: boolean;
  error?: string;
}

/**
 * A transcript is two lists, never merged: what the session itself did (`events`, with the live edge), and above it the
 * conversation's earlier history a resumed session brought along (`earlier`: the history items, in the conversation's order).
 * The daemon keeps them apart (their cursors are not comparable); so does the store.
 */
export interface TranscriptSlice extends EventList {
  earlier: EventList;
}

/** Older events exist above what a transcript shows: in the session's own events, or (once those are all here) in the earlier history. */
export const hasOlder = (s: TranscriptSlice | undefined): boolean => !!s && (s.hasMore || s.earlier.hasMore);

/** A session's trail as loaded here (Developer mode's Diagnostics tab); present only while that panel is open. */
export interface TrailSlice {
  /** Oldest first, in the host's order; contiguous from `items[0]` to the live edge. */
  items: TrailItem[];
  /** Older entries exist on the host. */
  hasMore: boolean;
  loading: boolean;
  error?: string;
}

/** A `diag.tail` subscription: which daemon, what the daemon is asked for, what is kept of it. */
export interface TailSpec {
  /** Names the subscription (callers that want the same data use the same id). */
  id: string;
  /** The host whose daemon serves it; omitted: the hub (this computer's own daemon). */
  host?: HostId;
  filter: DiagTailParams;
  keep?: TailKeep;
}

/** What a `diag.tail` subscription has delivered; present only while its panel is open. */
export interface TailSlice {
  /** `waiting`: subscribed, nothing delivered yet. */
  state: "waiting" | "live";
  /** Oldest first, in the daemon's order; the newest `TAIL_KEEP` that matched. */
  records: DiagRecord[];
  /** Records of the daemon that matched and were dropped to stay within `TAIL_KEEP`. */
  trimmed: number;
  /** Lines the daemon sent that are not diagnostics records. */
  unreadable: number;
}

/** Where starting an export is. `awaiting`: the hub accepted it (its id) and has not reported it on the topic yet. */
export type ExportStart = { state: "idle" } | { state: "starting" } | { state: "awaiting"; id: string } | { state: "refused"; reason: string };

/** Saving a finished export where the user chooses: not an act of the hub, a copy the desktop shell makes. */
export type ExportSave = { state: "saving" } | { state: "saved"; path: string } | { state: "cancelled" } | { state: "failed"; reason: string };

/**
 * The hub's diagnostics exports as the Settings card shows them; present only while the card is
 * open. `waiting`: subscribed, nothing delivered yet (nothing is known, not even whether one runs).
 */
export interface ExportsSlice {
  state: "waiting" | "live";
  /** Every export the hub reports (a running one and the last finished ones), as it sent them. */
  list: ExportStatus[];
  start: ExportStart;
  /** What became of each finished export's "Save…", by its file name. */
  saves: Record<string, ExportSave>;
}

/** An event the app recorded of its own acts (`ui.*`), with when, by this computer's clock. */
export interface AppEventRec {
  t: number;
  event: UiEvent;
}

/** What the app's own diagnostics log could not take. */
export interface DiagLogState {
  /** Events the shell refused or that could not be handed to it. */
  failed: number;
  /** Why the last one failed. */
  error?: string;
}

export const APP_EVENTS_KEEP = 300;
export const TAIL_KEEP = 1000;

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
  /** The last `APP_EVENTS_KEEP` things the app recorded of its own acts, oldest first. */
  appEvents: AppEventRec[];
  diagLog: DiagLogState;
  /** Session trails, tails of daemon logs: only what an open Diagnostics panel is subscribed to. */
  trails: Record<SessionKey, TrailSlice>;
  tails: Record<string, TailSlice>;
  /** The diagnostics exports, while the Settings card has them open. */
  exports: ExportsSlice | null;
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
    appEvents: [],
    diagLog: { failed: 0 },
    trails: {},
    tails: {},
    exports: null,
    ...partial,
  };
}

export type AppStore = ReturnType<typeof createAppStore>;
export const createAppStore = (partial?: Partial<AppState>) => createStore<AppState>(initialState(partial));
