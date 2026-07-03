/**
 * WaslGateModal — the real handoff-approval modal (X5), replacing the interim
 * onOpenGate placeholder (ui-app-wiring.md:44-47).
 *
 * When a Wasl node surfaces a Tier-3 handoff gate (agent A wants to consult /
 * command / transfer to agent B), Marsad shows it here and RELAYS the human's
 * Approve/Deny. Marsad is a window for the human decision — it never
 * auto-approves and emits no decision without an explicit click.
 *
 * Renders nothing when there are no pending Wasl gates (optionality: a Marsad
 * with no Wasl node connected shows no Wasl chrome at all).
 */
import { useState } from "react";
import { useVm } from "../bridge/useVm";

export function WaslGateModal() {
  const { waslGates, answerWasl } = useVm();
  const [busy, setBusy] = useState(false);

  if (waslGates.length === 0) return null;

  // One gate at a time — the oldest pending gate is the front of the queue.
  const gate = waslGates[0];

  async function decide(outcome: "approved" | "denied") {
    if (busy) return;
    setBusy(true);
    try {
      await answerWasl(gate.host_id, gate.gate_id, outcome);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      data-testid="wasl-gate-modal"
      role="dialog"
      aria-label="Wasl handoff approval"
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.45)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
      }}
    >
      <div
        style={{
          width: 420,
          maxWidth: "90vw",
          background: "var(--panel)",
          border: "1px solid var(--border)",
          borderRadius: 14,
          padding: 22,
          boxShadow: "0 18px 50px rgba(0,0,0,0.4)",
        }}
      >
        <div
          style={{
            fontSize: "calc(12px * var(--fs,1))",
            textTransform: "uppercase",
            letterSpacing: 1,
            color: "var(--txt3)",
          }}
        >
          Wasl handoff · {gate.kind}
        </div>
        <div
          data-testid="wasl-gate-parties"
          style={{ fontWeight: 800, fontSize: "calc(17px * var(--fs,1))", marginTop: 6 }}
        >
          {gate.from} → this host
        </div>
        <div style={{ color: "var(--txt2)", marginTop: 10 }}>{gate.summary}</div>
        {gate.preview && (
          <div
            className="mono"
            data-testid="wasl-gate-preview"
            style={{
              marginTop: 10,
              padding: "8px 12px",
              background: "var(--panel2)",
              border: "1px solid var(--border)",
              borderRadius: 8,
              fontSize: "calc(13px * var(--fs,1))",
              color: "var(--txt2)",
              maxHeight: 140,
              overflow: "auto",
            }}
          >
            {gate.preview}
          </div>
        )}
        {waslGates.length > 1 && (
          <div style={{ marginTop: 10, color: "var(--txt3)", fontSize: "calc(12px * var(--fs,1))" }}>
            +{waslGates.length - 1} more pending
          </div>
        )}
        <div style={{ display: "flex", gap: 10, marginTop: 18, justifyContent: "flex-end" }}>
          <button
            data-testid="wasl-gate-deny"
            disabled={busy}
            onClick={() => decide("denied")}
            style={{
              padding: "8px 18px",
              borderRadius: 8,
              border: "1px solid var(--border)",
              background: "transparent",
              color: "var(--txt)",
              cursor: busy ? "default" : "pointer",
            }}
          >
            Deny
          </button>
          <button
            data-testid="wasl-gate-approve"
            disabled={busy}
            onClick={() => decide("approved")}
            style={{
              padding: "8px 18px",
              borderRadius: 8,
              border: "none",
              background: "var(--accent)",
              color: "#fff",
              fontWeight: 700,
              cursor: busy ? "default" : "pointer",
            }}
          >
            Approve
          </button>
        </div>
      </div>
    </div>
  );
}
