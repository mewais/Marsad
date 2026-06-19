/**
 * Workspaces screen — switcher + manager.
 * Placeholder stub — D02 screen-agent will implement the full layout.
 */
import { useVm } from "../bridge/useVm";

export function Workspaces() {
  const { workspaces } = useVm();
  return (
    <div style={{ padding: "22px 26px", color: "var(--txt)" }}>
      <h1 style={{ fontSize: 21, fontWeight: 700, margin: "0 0 8px", letterSpacing: -0.3 }}>
        Workspaces
      </h1>
      <p style={{ color: "var(--txt3)", margin: 0 }}>
        {workspaces.length} workspaces — Workspaces placeholder (D02 agent fills this)
      </p>
    </div>
  );
}
