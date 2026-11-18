import type { DriverDescriptor } from "../ui/DriverBadge";
import type { HostId, HostInfo, HostLink, SessionEvent, SessionKey, SessionSnapshot } from "../bridge/types";
import { createStore } from "../lib/store";

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
  hostOrder: HostId[];
  links: Record<HostId, HostLink>;
  hostInfo: Record<HostId, HostInfo>;
  /** Whether the first full session list of a host has arrived. */
  hostLoaded: Record<HostId, boolean>;
  drivers: Record<string, DriverDescriptor>;
  driverOrder: string[];
  sessions: Record<SessionKey, SessionSnapshot>;
  transcripts: Record<SessionKey, TranscriptSlice>;
  /** ms timestamp up to which the human has seen each session's finished turns. */
  seen: Record<SessionKey, number>;
  /** "Finished since you looked at": time of the last mark-all-seen (or first open). */
  lookedAt: number;
  /** In-flight and just-sent actions, keyed by `kind:...`. */
  pending: Record<string, PendingState>;
}

export function initialState(partial: Partial<AppState> = {}): AppState {
  return {
    hostOrder: [],
    links: {},
    hostInfo: {},
    hostLoaded: {},
    drivers: {},
    driverOrder: [],
    sessions: {},
    transcripts: {},
    seen: {},
    lookedAt: Date.now(),
    pending: {},
    ...partial,
  };
}

export type AppStore = ReturnType<typeof createAppStore>;
export const createAppStore = (partial?: Partial<AppState>) => createStore<AppState>(initialState(partial));
