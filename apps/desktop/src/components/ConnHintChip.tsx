import type { CSSProperties } from "react";
import type { ConnHint } from "../bridge/connectivity";

/**
 * Small pill surfacing a session's connectivity ("Reconnecting" / "Disconnected"
 * / "Unstable"). This is NOT a lifecycle badge — the session stays Resumed while
 * its link is down; this only says "we've temporarily lost / degraded contact".
 * Amber + pulsing for reconnecting/unstable (transient), red + solid for
 * disconnected (contact lost), matching the host-status colours used elsewhere.
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
  const label = hint === "Unstable" ? "Unstable" : hint;
  const pulse = hint === "Reconnecting" || hint === "Unstable";
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
    <span data-testid={`conn-hint-${hint.toLowerCase()}`} style={style} title={`Connection ${hint.toLowerCase()}`}>
      {/* pulse dot for reconnecting/unstable; solid for disconnected */}
      <span
        data-pulse={pulse ? "true" : "false"}
        style={{
          width: 6,
          height: 6,
          borderRadius: "50%",
          background: color,
          display: "inline-block",
          flexShrink: 0,
        }}
      />
      {label}
    </span>
  );
}
