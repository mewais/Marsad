import type { IconName } from "./Icon";
import type { PhaseKind } from "../bridge/wire";

/** Everything the UI shows for a session's state: word, icon, colour class. One place. */
export type StatusKey = "needs" | "failed" | "done" | "working" | "idle" | "ended";

export interface StatusMeta {
  label: string;
  icon: IconName;
  /** Group heading, plural-agnostic. */
  group: string;
}

export const STATUS: Record<StatusKey, StatusMeta> = {
  needs: { label: "Needs you", icon: "needs", group: "Needs you" },
  failed: { label: "Failed", icon: "failed", group: "Failed" },
  done: { label: "Finished", icon: "done", group: "Finished, not seen yet" },
  working: { label: "Working", icon: "working", group: "Working" },
  idle: { label: "Idle", icon: "idle", group: "Idle" },
  ended: { label: "Ended", icon: "stop", group: "Ended" },
};

export const STATUS_ORDER: StatusKey[] = ["needs", "failed", "done", "working", "idle", "ended"];

/**
 * Phase kind to status. `unseenFinish` marks an idle/exited session whose last turn the human has not looked at:
 * it stays under "Finished" until seen, ended or not. An ended (`exited`) session is its own status, apart from idle;
 * `lost` (the daemon lost contact with the process) stays with `failed`.
 */
export function phaseToStatus(phase: PhaseKind, unseenFinish = false): StatusKey {
  switch (phase) {
    case "needs_you":
      return "needs";
    case "failed":
    case "lost":
      return "failed";
    case "starting":
    case "working":
      return "working";
    case "idle":
      return unseenFinish ? "done" : "idle";
    case "exited":
      return unseenFinish ? "done" : "ended";
  }
}
