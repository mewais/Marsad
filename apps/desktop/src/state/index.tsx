import { createContext, useContext, useEffect, useMemo } from "react";
import type { Backend } from "../bridge/backend";
import type { HostId, SessionKey } from "../bridge/types";
import { shallowEqual, useStore } from "../lib/store";
import { createActions, type Actions } from "./actions";
import { connectStore } from "./controller";
import { createAppStore, type AppState, type AppStore } from "./model";
import { loadSeen } from "./seen";
import { selectCounts, selectGroups, selectHostLinks, selectView, groupsEqual, type Counts, type Filters, type Groups } from "./selectors";
import { createTranscripts, type Transcripts } from "./transcript";
import type { SessionView } from "./domain";
import type { DriverDescriptor } from "../ui/DriverBadge";
import type { HostLink } from "../bridge/types";
import { FixtureBackend } from "../bridge/fixture/FixtureBackend";

export * from "./model";
export * from "./selectors";
export * from "./domain";
export * from "./actions";
export * from "./debug";

export interface AppContextValue {
  store: AppStore;
  backend: Backend;
  actions: Actions;
  transcripts: Transcripts;
  localHost: HostId;
}

const Ctx = createContext<AppContextValue | null>(null);

export function createApp(backend: Backend, partial?: Partial<AppState>): AppContextValue & { dispose: () => void } {
  const persisted = loadSeen();
  const fx = backend instanceof FixtureBackend ? backend.initialSeen() : null;
  const store = createAppStore({
    ...(persisted ? { seen: persisted.seen, lookedAt: persisted.lookedAt } : fx ? { seen: fx.seen, lookedAt: fx.lookedAt } : {}),
    ...partial,
  });
  const dispose = connectStore(store, backend);
  return {
    store,
    backend,
    actions: createActions(store, backend),
    transcripts: createTranscripts(store, backend),
    localHost: backend.connection.get().hosts[0]?.id ?? "local",
    dispose,
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

export const useCounts = (): Counts => useAppState(selectCounts, shallowEqual);
export const useHostLinks = (): HostLink[] => useAppState(selectHostLinks, (a, b) => a.length === b.length && a.every((x, i) => x === b[i]));
export const useDriver = (id: string): DriverDescriptor | undefined => useAppState((s) => s.drivers[id]);
export const useHostLink = (id: HostId): HostLink | undefined => useAppState((s) => s.links[id]);
export const usePending = (k: string) => useAppState((s) => s.pending[k]);

/** Open a session's transcript (history + live) while the component is mounted. */
export function useTranscriptOpen(key: SessionKey) {
  const { transcripts } = useApp();
  useEffect(() => transcripts.open(key), [transcripts, key]);
}

export function useActions() {
  const { actions } = useApp();
  return useMemo(() => actions, [actions]);
}
