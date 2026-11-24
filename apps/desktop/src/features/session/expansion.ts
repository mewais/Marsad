import { createContext, useCallback, useContext, useState, useSyncExternalStore } from "react";

/**
 * Which expandable rows of one transcript are open, by row id. It lives outside the rows: a row that scrolls out of view (or any
 * list that remounts) comes back in the state the reader left it, so the expansion is not the row's own React state.
 * `setAll` is the transcript-wide mode (expand or collapse everything); a row the reader toggled after that keeps its own state.
 */
export interface Expansion {
  isOpen(id: string): boolean;
  set(id: string, open: boolean): void;
  setAll(open: boolean): void;
  subscribe(listener: () => void): () => void;
}

export function createExpansion(): Expansion {
  let all = false;
  const open = new Map<string, boolean>();
  const listeners = new Set<() => void>();
  const emit = () => listeners.forEach((l) => l());
  return {
    isOpen: (id) => open.get(id) ?? all,
    set(id, value) {
      if (open.get(id) === value) return;
      open.set(id, value);
      emit();
    },
    setAll(value) {
      all = value;
      open.clear();
      emit();
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => void listeners.delete(listener);
    },
  };
}

/**
 * The expansion of each transcript, by its key (the same key the transcript's list is opened by). The registry outlives the list
 * on screen, so leaving a session and coming back keeps what was open.
 */
const transcripts = new Map<string, Expansion>();

export function expansionFor(transcript: string): Expansion {
  let e = transcripts.get(transcript);
  if (!e) transcripts.set(transcript, (e = createExpansion()));
  return e;
}

/** The transcript a row belongs to. Without one (a row shown on its own) the row keeps its own expansion. */
export const ExpansionCtx = createContext<Expansion | null>(null);

/** Whether the row `id` is open, and how to toggle it. */
export function useOpenRow(id: string): [boolean, () => void] {
  const shared = useContext(ExpansionCtx);
  const [own] = useState(createExpansion);
  const store = shared ?? own;
  const open = useSyncExternalStore(store.subscribe, () => store.isOpen(id));
  const toggle = useCallback(() => store.set(id, !store.isOpen(id)), [store, id]);
  return [open, toggle];
}
