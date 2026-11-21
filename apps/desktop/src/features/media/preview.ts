import { useSyncExternalStore } from "react";
import type { HostId, SessionId } from "../../bridge/wire";
import type { Target } from "./fetch";

/** One file to preview. */
export interface PreviewRequest {
  host: HostId;
  session: SessionId;
  target: Target;
  name: string;
  mime?: string | null;
  size?: number | null;
  /** The session's host is this machine (open with the system app / reveal are offered). */
  local: boolean;
}

let current: PreviewRequest | null = null;
const subs = new Set<() => void>();

/** Open the preview panel (one at a time; a new request replaces the open one). */
export function openPreview(r: PreviewRequest) {
  current = r;
  subs.forEach((f) => f());
}

export function closePreview() {
  current = null;
  subs.forEach((f) => f());
}

export function usePreview(): PreviewRequest | null {
  return useSyncExternalStore(
    (f) => {
      subs.add(f);
      return () => subs.delete(f);
    },
    () => current,
  );
}
