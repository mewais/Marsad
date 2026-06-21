/**
 * GateOverlay — full permission / plan / edit gate modal.
 * Approve or reject-with-reason. Routes back to the session on action.
 */
import { useState } from "react";
import { useVm } from "../bridge/useVm";
import { answerGate } from "../bridge/api";
import { Modal } from "../components/Modal";
import { Button } from "../components/Button";
import { DiffView } from "../components/DiffView";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function approveLabel(kind: string): string {
  if (kind === "plan") return "Approve plan";
  if (kind === "edit") return "Approve change";
  return "Approve & run";
}

function kindBadge(kind: string): string {
  if (kind === "plan") return "Plan";
  if (kind === "edit") return "Edit";
  return "Permission";
}

// ---------------------------------------------------------------------------
// GateOverlay
// ---------------------------------------------------------------------------

export function GateOverlay() {
  const { selectedGate, selectedSession, state, dispatch } = useVm();

  const [rejectOpen, setRejectOpen] = useState(false);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);

  const open = state.screen === "gate" && selectedGate !== null;

  function close() {
    // Return to the session that owns this gate
    if (selectedSession) {
      dispatch({ type: "SELECT_SESSION", id: selectedSession.id });
    } else {
      dispatch({ type: "SET_SCREEN", screen: "overview" });
    }
    setRejectOpen(false);
    setReason("");
  }

  async function handleApprove() {
    if (!selectedSession) return;
    setBusy(true);
    await answerGate({
      session_id: selectedSession.id,
      decision_id: selectedSession.id, // gate decision_id mirrors session id in mock
      allow: true,
    });
    setBusy(false);
    close();
  }

  async function handleReject() {
    if (!selectedSession) return;
    setBusy(true);
    await answerGate({
      session_id: selectedSession.id,
      decision_id: selectedSession.id,
      allow: false,
      deny_reason: reason.trim() || undefined,
    });
    setBusy(false);
    close();
  }

  if (!selectedGate || !selectedSession) {
    // Render an open=false modal so the modal shell still mounts for tests
    return (
      <Modal open={false} onClose={close}>
        <div />
      </Modal>
    );
  }

  const gate = selectedGate;
  const kind = gate.kind; // "permission" | "plan" | "edit"

  // Recover the raw gate payload from the session for rich display
  const raw = selectedSession.gate;
  const permissionData =
    raw && "Permission" in raw ? raw.Permission : null;
  const planData =
    raw && "Plan" in raw ? raw.Plan : null;
  const editData =
    raw && "Edit" in raw ? raw.Edit : null;

  return (
    <Modal open={open} onClose={close} width={640}>
      {/* ===== HEADER ===== */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "18px 22px 14px",
          borderBottom: "1px solid var(--border)",
        }}
      >
        {/* Kind badge */}
        <span
          style={{
            fontSize: 10,
            fontWeight: 700,
            letterSpacing: 0.5,
            textTransform: "uppercase",
            padding: "2px 7px",
            borderRadius: 5,
            background:
              kind === "permission"
                ? "var(--needs-bg)"
                : kind === "edit"
                ? "var(--working-bg)"
                : "var(--done-bg)",
            color:
              kind === "permission"
                ? "var(--needs)"
                : kind === "edit"
                ? "var(--accent2)"
                : "var(--done)",
          }}
          data-testid="gate-kind-badge"
        >
          {kindBadge(kind)}
        </span>

        {/* Gate icon + lead */}
        <span style={{ fontSize: 16 }}>{gate.icon}</span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontWeight: 700, fontSize: 14 }}>{gate.lead}</div>
          <div
            style={{
              fontSize: 11,
              color: "var(--txt3)",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            {selectedSession.name} · {selectedSession.host_id} ·{" "}
            {selectedSession.project}
          </div>
        </div>

        {/* Close ✕ */}
        <button
          onClick={close}
          aria-label="Close"
          style={{
            background: "none",
            border: "none",
            cursor: "pointer",
            fontSize: 18,
            color: "var(--txt3)",
            lineHeight: 1,
            padding: "2px 4px",
          }}
        >
          ✕
        </button>
      </div>

      {/* ===== BODY ===== */}
      <div style={{ padding: "18px 22px" }}>
        {/* ---- PERMISSION variant ---- */}
        {gate.is_permission && permissionData && (
          <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
            {/* Command block */}
            <div>
              <div
                style={{
                  fontSize: 10,
                  fontWeight: 700,
                  letterSpacing: 0.6,
                  textTransform: "uppercase",
                  color: "var(--txt3)",
                  marginBottom: 6,
                }}
              >
                Command
              </div>
              <div
                className="mono"
                data-testid="gate-command"
                style={{
                  background: "var(--panel2)",
                  border: "1px solid var(--border2)",
                  borderRadius: 8,
                  padding: "10px 14px",
                  fontSize: 12.5,
                  color: "var(--txt)",
                  wordBreak: "break-all",
                }}
              >
                {permissionData.command}
              </div>
            </div>

            {/* CWD */}
            <div>
              <div
                style={{
                  fontSize: 10,
                  fontWeight: 700,
                  letterSpacing: 0.6,
                  textTransform: "uppercase",
                  color: "var(--txt3)",
                  marginBottom: 6,
                }}
              >
                Working directory
              </div>
              <div
                className="mono"
                data-testid="gate-cwd"
                style={{ fontSize: 12, color: "var(--txt2)" }}
              >
                {permissionData.cwd}
              </div>
            </div>

            {/* Risk warning */}
            <div
              data-testid="gate-risk"
              style={{
                display: "flex",
                alignItems: "flex-start",
                gap: 10,
                padding: "10px 14px",
                borderRadius: 9,
                background: "var(--needs-bg)",
                border: "1px solid var(--needs)",
              }}
            >
              <span style={{ fontSize: 15, flexShrink: 0 }}>⚠</span>
              <div>
                <div
                  style={{
                    fontWeight: 700,
                    fontSize: 12.5,
                    color: "var(--needs)",
                    marginBottom: 3,
                  }}
                >
                  Risk: {permissionData.risk}
                </div>
                <div style={{ fontSize: 12, color: "var(--txt2)" }}>
                  {permissionData.explanation}
                </div>
              </div>
            </div>
          </div>
        )}

        {/* ---- PLAN variant ---- */}
        {gate.is_plan && planData && (
          <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <div
              style={{
                fontSize: 12.5,
                color: "var(--txt2)",
                fontStyle: "italic",
                marginBottom: 4,
              }}
            >
              {planData.summary}
            </div>
            <div
              data-testid="gate-plan-steps"
              style={{ display: "flex", flexDirection: "column", gap: 8 }}
            >
              {planData.steps.map((step, i) => (
                <div
                  key={i}
                  style={{
                    display: "flex",
                    gap: 12,
                    padding: "10px 14px",
                    borderRadius: 9,
                    background: "var(--panel2)",
                    border: "1px solid var(--border)",
                  }}
                >
                  <span
                    style={{
                      width: 22,
                      height: 22,
                      borderRadius: "50%",
                      background: "var(--accent)",
                      color: "#fff",
                      fontSize: 11,
                      fontWeight: 700,
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                      flexShrink: 0,
                    }}
                  >
                    {i + 1}
                  </span>
                  <div>
                    <div style={{ fontWeight: 700, fontSize: 13 }}>
                      {step.title}
                    </div>
                    <div style={{ fontSize: 12, color: "var(--txt3)", marginTop: 2 }}>
                      {step.detail}
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* ---- EDIT variant ---- */}
        {gate.is_edit && editData && (
          <div
            data-testid="gate-diff"
            style={{
              border: "1px solid var(--border)",
              borderRadius: 9,
              overflow: "hidden",
            }}
          >
            <DiffView rows={gate.diff_rows} filename={editData.file} />
          </div>
        )}
      </div>

      {/* ===== REJECT REASON ===== */}
      {rejectOpen && (
        <div style={{ padding: "0 22px 14px" }}>
          <textarea
            data-testid="reject-reason"
            placeholder="Reason for rejection (optional)"
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            rows={3}
            style={{
              width: "100%",
              boxSizing: "border-box",
              background: "var(--panel2)",
              border: "1px solid var(--border2)",
              borderRadius: 8,
              padding: "10px 12px",
              color: "var(--txt)",
              fontFamily: "inherit",
              fontSize: 12.5,
              resize: "vertical",
              outline: "none",
            }}
          />
        </div>
      )}

      {/* ===== FOOTER ACTIONS ===== */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "14px 22px 18px",
          borderTop: "1px solid var(--border)",
          justifyContent: "flex-end",
        }}
      >
        {/* Cancel */}
        <Button variant="ghost" size="md" onClick={close} disabled={busy}>
          Cancel
        </Button>

        {/* Reject flow */}
        {!rejectOpen ? (
          <Button
            variant="danger"
            size="md"
            data-testid="reject-btn"
            onClick={() => setRejectOpen(true)}
            disabled={busy}
          >
            Reject…
          </Button>
        ) : (
          <Button
            variant="danger"
            size="md"
            data-testid="send-rejection-btn"
            onClick={handleReject}
            disabled={busy}
          >
            Send rejection
          </Button>
        )}

        {/* Approve */}
        <Button
          variant="success"
          size="md"
          data-testid="approve-btn"
          onClick={handleApprove}
          disabled={busy}
        >
          {approveLabel(kind)}
        </Button>
      </div>
    </Modal>
  );
}
