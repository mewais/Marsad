import type { CSSProperties } from "react";
import type { ConnHint } from "../bridge/connectivity";

/**
 * Small pill surfacing a session's HOST connectivity ("Reconnecting" /
 * "Disconnected"). This is NOT a lifecycle badge — the session stays Resumed
 * while its host link is down; this only says "we've temporarily lost contact".
 * Amber for reconnecting (transient, hopeful), red for disconnected (contact
 * lost), matching the host-status colours used elsewhere.
 */
export function ConnHintChip({
  hint,
  size = "sm",
}: {
  hint: ConnHint;
  size?: "sm" | "xs";
}) {
  const color = hint === "Disconnected" ? "var(--error)" : "var(--needs)";
  const bg = hint === "Disconnected" ? "var(--error-bg)" : "var(--needs-bg)";
  const fontSize = size === "xs" ? "calc(9.5px * var(--fs,1))" : "calc(10.5px * var(--fs,1))";
  const style: CSSProperties = {
    display: "inline-flex",
    alignItems: "center",
    gap: 4,
    fontWeight: 700,
    fontSize,
    padding: "1px 7px",
    borderRadius: 6,
    color,
    background: bg,
    border: `1px solid ${color}`,
    whiteSpace: "nowrap",
  };
  return (
    <span data-testid={`conn-hint-${hint.toLowerCase()}`} style={style} title={`Host ${hint.toLowerCase()}`}>
      {/* pulse dot for reconnecting; solid for disconnected */}
      <span
        data-pulse={hint === "Reconnecting" ? "true" : "false"}
        style={{
          width: 6,
          height: 6,
          borderRadius: "50%",
          background: color,
          display: "inline-block",
          flexShrink: 0,
        }}
      />
      {hint}
    </span>
  );
}
