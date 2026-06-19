/**
 * SessionDetail screen — activity transcript, tabs (Activity/Changes/Config/Usage/Terminal).
 * Placeholder stub — D02 screen-agent will implement the full layout.
 */
import { useVm } from "../bridge/useVm";

export function SessionDetail() {
  const { selectedDetail } = useVm();
  if (!selectedDetail) {
    return (
      <div style={{ padding: 24, color: "var(--txt3)" }}>No session selected.</div>
    );
  }
  return (
    <div style={{ padding: 24, color: "var(--txt)" }}>
      <h1 style={{ fontSize: 18, fontWeight: 700, margin: "0 0 8px" }}>
        {selectedDetail.name}
      </h1>
      <p style={{ color: "var(--txt3)", margin: 0 }}>
        SessionDetail placeholder (D02 agent fills this)
      </p>
    </div>
  );
}
