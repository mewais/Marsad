import type { IconName } from "./Icon";
import type { PhaseKind } from "../bridge/wire";

/** Everything the UI shows for a session's state: word, icon, colour class. One place. */
export type StatusKey = "needs" | "failed" | "done" | "working" | "idle";

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
};

export const STATUS_ORDER: StatusKey[] = ["needs", "failed", "done", "working", "idle"];

/** Phase kind to status. `unseenFinish` marks an idle/exited session whose last turn the human has not looked at. */
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
    case "exited":
      return unseenFinish ? "done" : "idle";
  }
}

/** The word shown for a phase when it is more specific than the status label. */
export function phaseWord(phase: PhaseKind, status: StatusKey): string {
  if (phase === "starting") return "Starting";
  if (phase === "lost") return "Lost";
  if (phase === "exited" && status === "idle") return "Ended";
  return STATUS[status].label;
}
