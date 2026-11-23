import { createContext, useContext, useEffect, useMemo } from "react";
import type { Backend } from "../bridge/backend";
import { createApi, type Api } from "../bridge/api";
import type { HostId, SessionKey } from "../bridge/wire";
import { shallowEqual, useStore } from "../lib/store";
import { createActions, type Actions } from "./actions";
import { connectStore } from "./controller";
import { createDiagLog, createTracer, withRpcOutcomes } from "./diaglog";
import { createFeeds } from "./feeds";
import { createAppStore, type AppState, type AppStore } from "./model";
import { reachBlock, selectCounts, selectDeveloperMode, selectGroups, selectHost, selectHosts, selectStaleHosts, selectView, groupsEqual, type Counts, type Filters, type Groups } from "./selectors";
import { createTails, createTrails, type Tails, type Trails } from "./diagnostics";
import { createTranscripts, type Transcripts } from "./transcript";
import type { SessionView } from "./domain";
import type { HostView } from "./hosts";
import type { DriverDescriptor } from "../ui/DriverBadge";

export * from "./model";
export * from "./selectors";
export * from "./domain";
export * from "./actions";
export * from "./debug";
export * from "./hosts";

export interface AppContextValue {
  store: AppStore;
  backend: Backend;
  api: Api;
  actions: Actions;
  transcripts: Transcripts;
  /** Developer mode's session trails and daemon log tails: subscribed only while a panel has them open. */
  trails: Trails;
  tails: Tails;
}

const Ctx = createContext<AppContextValue | null>(null);

export function createApp(backend: Backend, partial?: Partial<AppState>): AppContextValue & { dispose: () => void } {
  const store = createAppStore(partial);
  const diag = createDiagLog(store, backend);
  // Every request that carries a trace has its outcome recorded as it passes through here.
  const api = createApi(withRpcOutcomes(backend, diag.log));
  const feeds = createFeeds(store);
  const controller = connectStore(store, api, feeds);
  return {
    store,
    backend,
    api,
    actions: createActions(store, api, controller, { trace: createTracer(diag.log), log: diag.log }),
    transcripts: createTranscripts(store, api, feeds),
    trails: createTrails(store, api, feeds),
    tails: createTails(store, api, feeds),
    dispose: controller.dispose,
  };
}

export function AppProvider({ value, children }: { value: AppContextValue; children: React.ReactNode }) {
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useApp(): AppContextValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("AppProvider missing");
  return v;
}

export const useApi = (): Api => useApp().api;
/** The api when rendered inside the app, null for a bare component tree (tests, previews). */
export const useOptionalApi = (): Api | null => useContext(Ctx)?.api ?? null;

export function useAppState<T>(select: (s: AppState) => T, eq?: (a: T, b: T) => boolean): T {
  return useStore(useApp().store, select, eq);
}

/** One session's view; re-renders only when that session's visible fields change. */
export function useSessionView(key: SessionKey): SessionView | undefined {
  return useAppState((s) => selectView(s, key), shallowEqual);
}

export function useGroups(filters: Filters): Groups {
  return useAppState((s) => selectGroups(s, filters), groupsEqual);
}

const sameList = <T,>(a: T[], b: T[]) => a.length === b.length && a.every((x, i) => x === b[i]);
export const useCounts = (): Counts => useAppState(selectCounts, shallowEqual);
export const useHosts = (): HostView[] => useAppState(selectHosts, sameList);
export const useHost = (id: HostId): HostView | undefined => useAppState((s) => selectHost(s, id));
export const useDriver = (id: string): DriverDescriptor | undefined => useAppState((s) => s.drivers[id]);
export const usePending = (k: string) => useAppState((s) => s.pending[k]);
/** Why requests that need the daemon (and `host`, when given) cannot be made now, or `null`. Disable the control with it. */
export const useReach = (host?: HostId): string | null => useAppState((s) => reachBlock(s, host));
/** `useReach` for something about a session: also off while what is shown of it is last-known (the host has not reported yet). */
export function useSessionReach(v: Pick<SessionView, "host" | "stale"> | undefined): string | null {
  const reach = useReach(v?.host);
  return reach ?? (v?.stale ? "This is the last known state of the session; wait for its host to report." : null);
}
/** Hosts whose data is last-known rather than live. */
export function useStaleHosts(): ReadonlySet<HostId> {
  const key = useAppState(selectStaleHosts);
  return useMemo(() => new Set(key ? key.split("|") : []), [key]);
}

/** Whether Developer mode is on: the hub's setting, once the hub has told us (never assumed before). */
export const useDeveloperMode = (): boolean => useAppState(selectDeveloperMode);

/** Open a session's transcript (history + live) while the component is mounted. */
export function useTranscriptOpen(key: SessionKey) {
  const { transcripts } = useApp();
  useEffect(() => transcripts.open(key), [transcripts, key]);
}

export function useActions(): Actions {
  return useApp().actions;
}
