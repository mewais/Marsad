import { useCallback, useRef, useSyncExternalStore } from "react";

export interface Store<S> {
  get(): S;
  set(next: S | ((prev: S) => S)): void;
  subscribe(cb: () => void): () => void;
}

export function createStore<S>(initial: S): Store<S> {
  let state = initial;
  const subs = new Set<() => void>();
  return {
    get: () => state,
    set(next) {
      const value = typeof next === "function" ? (next as (p: S) => S)(state) : next;
      if (Object.is(value, state)) return;
      state = value;
      subs.forEach((s) => s());
    },
    subscribe(cb) {
      subs.add(cb);
      return () => subs.delete(cb);
    },
  };
}

export function shallowEqual<T>(a: T, b: T): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || !a || !b) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  if (ka.length !== Object.keys(b).length) return false;
  return ka.every((k) => Object.is((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]));
}

/**
 * Subscribe a component to a slice of a store. Re-renders only when `select(state)` changes
 * according to `eq` (default `Object.is`), so one session updating does not re-render rows that
 * select other sessions.
 */
export function useStore<S, T>(
  store: Store<S>,
  select: (s: S) => T,
  eq: (a: T, b: T) => boolean = Object.is,
): T {
  const selRef = useRef(select);
  selRef.current = select;
  const cache = useRef<{ s: S; sel: (s: S) => T; v: T } | null>(null);
  const getSnapshot = useCallback(() => {
    const s = store.get();
    const prev = cache.current;
    if (prev && prev.s === s && prev.sel === selRef.current) return prev.v;
    const v = selRef.current(s);
    if (prev && eq(prev.v, v)) {
      cache.current = { s, sel: selRef.current, v: prev.v };
      return prev.v;
    }
    cache.current = { s, sel: selRef.current, v };
    return v;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store]);
  return useSyncExternalStore(store.subscribe, getSnapshot, getSnapshot);
}
