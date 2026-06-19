/**
 * Overview screen — command center with stat tiles, gate grid, active rows, idle grid.
 * Placeholder stub — D02 screen-agent will implement the full layout.
 */
import { useVm } from "../bridge/useVm";

export function Overview() {
  const { counts } = useVm();
  return (
    <div style={{ padding: "22px 26px", color: "var(--txt)" }}>
      <h1 style={{ fontSize: 21, fontWeight: 700, margin: "0 0 8px", letterSpacing: -0.3 }}>
        Command center
      </h1>
      <p style={{ color: "var(--txt3)", margin: 0 }}>
        {counts.total} sessions — Overview screen placeholder (D02 agent fills this)
      </p>
    </div>
  );
}
