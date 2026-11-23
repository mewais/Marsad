import { Icon } from "./Icon";
import { STATUS, type StatusKey } from "./status";

export interface StatusPillProps {
  status: StatusKey;
  /** Overrides the word, e.g. "Waiting for you · 1 permission". */
  label?: string;
  plain?: boolean;
  /** Where the session was when it was last heard from, not what it is doing: dimmed, nothing animates. */
  stale?: boolean;
}

/** Word + icon + colour, never colour alone. */
export function StatusPill({ status, label, plain, stale }: StatusPillProps) {
  const m = STATUS[status];
  return (
    <span className={`st ${status}${plain ? " plain" : ""}${stale ? " stale" : ""}`} data-status={status} data-stale={stale ? "" : undefined} title={stale ? "Last known: this is not being reported live right now" : undefined}>
      <Icon name={m.icon} size="sm" spin={status === "working" && !stale} />
      {label ?? m.label}
    </span>
  );
}
