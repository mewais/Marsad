import type { DriverDescriptor } from "../ui/DriverBadge";
import type { DriverInventory, GitStatus, HostId, NotificationPrefs, SeenMark, SessionEvent, SessionKey, SessionSnapshot } from "../bridge/wire";
import type { Workstream } from "../bridge/provisional/workstreams";
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

export interface AppState {
  /** The first `fleet` reset has arrived. */
  fleetReady: boolean;
  fleetError?: string;
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
  /** Latest time the human marked something seen (for "finished since you looked at"). */
  lookedAt: number;
  /** In-flight and just-sent actions, keyed by `kind:...`. */
  pending: Record<string, PendingState>;
  /** `git.status` per session (branch, diffstat), fetched lazily. `null` = not a repo / failed. */
  git: Record<SessionKey, GitStatus | null>;
  workstreams: Workstream[];
  workstreamsLoaded: boolean;
  notifications: NotificationPrefs | null;
  settings: Record<string, unknown>;
}

export function initialState(partial: Partial<AppState> = {}): AppState {
  return {
    fleetReady: false,
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
    workstreams: [],
    workstreamsLoaded: false,
    notifications: null,
    settings: {},
    ...partial,
  };
}

export type AppStore = ReturnType<typeof createAppStore>;
export const createAppStore = (partial?: Partial<AppState>) => createStore<AppState>(initialState(partial));
