/**
 * GateOverlay — permission/plan/edit gate, approve/reject-with-reason.
 * Placeholder stub — D02 screen-agent will implement the full layout.
 */
import { useVm } from "../bridge/useVm";

export function GateOverlay() {
  const { selectedGate } = useVm();
  if (!selectedGate) {
    return (
      <div style={{ padding: 24, color: "var(--txt3)" }}>No pending gate.</div>
    );
  }
  return (
    <div style={{ padding: 24, color: "var(--txt)" }}>
      <h2 style={{ fontSize: 18, fontWeight: 700, margin: "0 0 8px" }}>
        Gate: {selectedGate.kind}
      </h2>
      <p style={{ color: "var(--txt3)", margin: 0 }}>
        GateOverlay placeholder (D02 agent fills this)
      </p>
    </div>
  );
}
