import { Icon } from "./Icon";
import { STATUS, type StatusKey } from "./status";

export interface StatusPillProps {
  status: StatusKey;
  /** Overrides the word, e.g. "Waiting for you · 1 permission". */
  label?: string;
  plain?: boolean;
}

/** Word + icon + colour, never colour alone. */
export function StatusPill({ status, label, plain }: StatusPillProps) {
  const m = STATUS[status];
  return (
    <span className={`st ${status}${plain ? " plain" : ""}`} data-status={status}>
      <Icon name={m.icon} size="sm" spin={status === "working"} />
      {label ?? m.label}
    </span>
  );
}
