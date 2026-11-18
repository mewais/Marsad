import type { SessionKey } from "../bridge/types";

const KEY = "marsad.seen.v1";

export interface SeenPersisted {
  seen: Record<SessionKey, number>;
  lookedAt: number;
}

export function loadSeen(): SeenPersisted | null {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? (JSON.parse(raw) as SeenPersisted) : null;
  } catch {
    return null;
  }
}

export function saveSeen(p: SeenPersisted) {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    /* storage unavailable: markers last for this run only */
  }
}
