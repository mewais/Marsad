import type { CSSProperties } from "react";
import type { EndReason } from "../bridge/types";
import { endReasonText } from "../bridge/types";

/**
 * Small pill for why a turn ended, so a card is never a blank Error/Done. A user
 * Cancel is calm/neutral (it's deliberate, not a failure); a real error
 * (MaxTurns / ExecutionError) is red. Text mirrors `endReasonText`.
 */
export function EndReasonChip({
  reason,
  size = "sm",
}: {
  reason: EndReason;
  size?: "sm" | "xs";
}) {
  const isCancel = reason === "Cancelled";
  const color = isCancel ? "var(--txt2)" : "var(--error)";
  const bg = isCancel ? "var(--panel2)" : "var(--error-bg)";
  const fontSize = size === "xs" ? "calc(9.5px * var(--fs,1))" : "calc(10px * var(--fs,1))";
  const style: CSSProperties = {
    display: "inline-flex",
    alignItems: "center",
    fontWeight: 600,
    fontSize,
    padding: "2px 8px",
    borderRadius: 6,
    color,
    background: bg,
    border: isCancel ? "1px solid var(--border2)" : `1px solid ${color}`,
    whiteSpace: "nowrap",
  };
  return (
    <span data-testid={`end-reason-${reason.toLowerCase()}`} style={style}>
      {isCancel ? "Cancelled" : endReasonText(reason)}
    </span>
  );
}
