import { createContext, useContext, useEffect } from "react";
import type { Backend } from "../bridge/backend";
import { createApi, type Api } from "../bridge/api";
import type { HostId, SessionKey } from "../bridge/wire";
import { shallowEqual, useStore } from "../lib/store";
import { createActions, type Actions } from "./actions";
import { connectStore } from "./controller";
import { createAppStore, type AppState, type AppStore } from "./model";
import { selectCounts, selectGroups, selectHost, selectHosts, selectView, groupsEqual, type Counts, type Filters, type Groups } from "./selectors";
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
}

const Ctx = createContext<AppContextValue | null>(null);

export function createApp(backend: Backend, partial?: Partial<AppState>): AppContextValue & { dispose: () => void } {
  const store = createAppStore(partial);
  const api = createApi(backend);
  const dispose = connectStore(store, api);
  return { store, backend, api, actions: createActions(store, api), transcripts: createTranscripts(store, api), dispose };
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

/** Open a session's transcript (history + live) while the component is mounted. */
export function useTranscriptOpen(key: SessionKey) {
  const { transcripts } = useApp();
  useEffect(() => transcripts.open(key), [transcripts, key]);
}

export function useActions(): Actions {
  return useApp().actions;
}
