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
  /** The host's files are reachable from this machine (open with the system app / reveal are offered). */
  localFiles: boolean;
  /** Scroll a text preview to this 1-based line (a `path:line` mention). */
  line?: number | null;
  /** The target is a folder: shown as a simple listing, each entry opened by its own click. */
  dir?: boolean;
  /** WSL distro of the host (the shell translates its Linux paths to Windows paths). */
  distro?: string | null;
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

// ── one-time confirm before opening well-known secret locations ─────────────

const confirmed = new Set<string>();
let pendingSecret: { path: string; done: (ok: boolean) => void } | null = null;
const secretSubs = new Set<() => void>();

/** Ask the user (once per path per run) before a secret location is opened; resolves true to go on. */
export function confirmSecret(path: string): Promise<boolean> {
  if (confirmed.has(path)) return Promise.resolve(true);
  pendingSecret?.done(false);
  return new Promise((res) => {
    pendingSecret = {
      path,
      done: (ok) => {
        if (ok) confirmed.add(path);
        pendingSecret = null;
        secretSubs.forEach((f) => f());
        res(ok);
      },
    };
    secretSubs.forEach((f) => f());
  });
}

export function usePendingSecret() {
  return useSyncExternalStore(
    (f) => {
      secretSubs.add(f);
      return () => secretSubs.delete(f);
    },
    () => pendingSecret,
  );
}

/** Test hook. */
export function resetSecretConfirms() {
  confirmed.clear();
  pendingSecret = null;
}
