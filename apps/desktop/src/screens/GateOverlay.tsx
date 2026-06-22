/**
 * GateOverlay — full permission / plan / edit gate modal.
 * Approve or reject-with-reason. Routes back to the session on action.
 */
import { useState, useEffect } from "react";
import { useVm } from "../bridge/useVm";
import { answerGate } from "../bridge/api";
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

  // Close on Escape
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

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

  if (!open || !selectedGate || !selectedSession) {
    // Render nothing visible — keep mount for test compatibility
    return (
      <div
        style={{ display: "none" }}
        data-testid="gate-overlay-hidden"
        aria-hidden="true"
      />
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

  // The route ID shown in the footer (mirrors decision_id / session id)
  const routeId = selectedSession.id;

  return (
    /* ── Backdrop ── */
    <div
      onClick={close}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(5,7,10,0.62)",
        backdropFilter: "blur(3px)",
        WebkitBackdropFilter: "blur(3px)",
        zIndex: 60,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: 32,
      }}
    >
      {/* ── Panel ── */}
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: "100%",
          maxWidth: 740,
          maxHeight: "88vh",
          background: "var(--panel)",
          border: "1px solid var(--border2)",
          borderRadius: 18,
          boxShadow: "var(--shadow)",
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          animation: "slideup 0.18s ease",
        }}
      >
        {/* ===== HEADER ===== */}
        <div
          style={{
            padding: "18px 22px",
            borderBottom: "1px solid var(--border)",
            display: "flex",
            alignItems: "flex-start",
            gap: 13,
            flexShrink: 0,
          }}
        >
          {/* Gate icon box */}
          <div
            style={{
              width: 40,
              height: 40,
              borderRadius: 11,
              background: "var(--needs-bg)",
              border: "1px solid var(--needs)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: 18,
              color: "var(--needs)",
              flexShrink: 0,
            }}
          >
            {gate.icon}
          </div>

          {/* Title block */}
          <div style={{ flex: 1, minWidth: 0 }}>
            {/* Kind badge + host label row */}
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 9,
                marginBottom: 6,
              }}
            >
              <span
                style={{
                  fontSize: 10,
                  fontWeight: 700,
                  letterSpacing: 0.4,
                  padding: "2px 7px",
                  borderRadius: 5,
                  color: "var(--needs)",
                  background: "var(--needs-bg)",
                  textTransform: "uppercase",
                }}
                data-testid="gate-kind-badge"
              >
                {kindBadge(kind)}
              </span>
              <span
                className="mono"
                style={{ fontSize: 10.5, color: "var(--txt3)" }}
              >
                {selectedSession.host_id}
              </span>
            </div>

            {/* Gate title */}
            <h2
              style={{
                fontSize: 16,
                fontWeight: 700,
                margin: "0 0 1px",
                letterSpacing: -0.2,
                color: "var(--txt)",
              }}
            >
              {gate.lead}
            </h2>

            {/* Session · project */}
            <div
              className="mono"
              style={{ fontSize: 11, color: "var(--txt3)" }}
            >
              {selectedSession.name} · {selectedSession.project}
            </div>
          </div>

          {/* Close button */}
          <button
            onClick={close}
            aria-label="Close"
            style={{
              width: 30,
              height: 30,
              borderRadius: 8,
              border: "1px solid var(--border)",
              background: "var(--panel2)",
              color: "var(--txt2)",
              cursor: "pointer",
              fontSize: 14,
              flexShrink: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              lineHeight: 1,
            }}
          >
            ✕
          </button>
        </div>

        {/* ===== SCROLLABLE BODY ===== */}
        <div
          style={{
            flex: 1,
            overflowY: "auto",
            padding: "20px 22px",
          }}
        >
          {/* ---- PERMISSION variant ---- */}
          {gate.is_permission && permissionData && (
            <div style={{ display: "flex", flexDirection: "column", gap: 0 }}>
              {/* Explanation */}
              <div
                style={{
                  fontSize: 12.5,
                  color: "var(--txt2)",
                  marginBottom: 11,
                  lineHeight: 1.55,
                }}
              >
                {permissionData.explanation}
              </div>

              {/* Command block */}
              <div
                style={{
                  borderRadius: 11,
                  border: "1px solid var(--border2)",
                  overflow: "hidden",
                  marginBottom: 14,
                }}
              >
                {/* Block header */}
                <div
                  style={{
                    padding: "8px 13px",
                    background: "var(--panel2)",
                    borderBottom: "1px solid var(--border)",
                    fontSize: 10.5,
                    fontWeight: 700,
                    letterSpacing: 0.5,
                    textTransform: "uppercase",
                    color: "var(--txt3)",
                  }}
                >
                  Command to run
                </div>

                {/* $ command */}
                <pre
                  className="mono"
                  data-testid="gate-command"
                  style={{
                    margin: 0,
                    padding: 14,
                    fontSize: 13,
                    color: "var(--accent2)",
                    background: "var(--bg)",
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-all",
                  }}
                >
                  $ {permissionData.command}
                </pre>

                {/* cwd */}
                <div
                  className="mono"
                  data-testid="gate-cwd"
                  style={{
                    padding: "8px 14px",
                    fontSize: 10.5,
                    color: "var(--txt3)",
                    borderTop: "1px solid var(--border)",
                  }}
                >
                  cwd: {permissionData.cwd}
                </div>
              </div>

              {/* Risk warning — single-line amber banner */}
              <div
                data-testid="gate-risk"
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "10px 13px",
                  borderRadius: 9,
                  background: "var(--needs-bg)",
                  border: "1px solid var(--needs)",
                  fontSize: 12,
                  color: "var(--needs)",
                }}
              >
                ⚠ {permissionData.risk}
              </div>
            </div>
          )}

          {/* ---- PLAN variant ---- */}
          {gate.is_plan && planData && (
            <div style={{ display: "flex", flexDirection: "column", gap: 0 }}>
              {/* Summary */}
              <div
                style={{
                  fontSize: 13,
                  color: "var(--txt)",
                  lineHeight: 1.6,
                  marginBottom: 16,
                }}
              >
                {planData.summary}
              </div>

              {/* Steps */}
              <div
                data-testid="gate-plan-steps"
                style={{ display: "flex", flexDirection: "column", gap: 9 }}
              >
                {planData.steps.map((step, i) => (
                  <div
                    key={i}
                    style={{
                      display: "flex",
                      gap: 12,
                      padding: "13px 14px",
                      borderRadius: 11,
                      background: "var(--panel2)",
                      border: "1px solid var(--border)",
                    }}
                  >
                    {/* Step number circle — accent style */}
                    <span
                      style={{
                        width: 23,
                        height: 23,
                        borderRadius: "50%",
                        background: "var(--accent-bg)",
                        color: "var(--accent2)",
                        fontWeight: 800,
                        fontSize: 11.5,
                        display: "flex",
                        alignItems: "center",
                        justifyContent: "center",
                        flexShrink: 0,
                      }}
                    >
                      {i + 1}
                    </span>

                    <div>
                      <div
                        style={{
                          fontWeight: 600,
                          fontSize: 13,
                          marginBottom: 2,
                          color: "var(--txt)",
                        }}
                      >
                        {step.title}
                      </div>
                      <div
                        style={{
                          fontSize: 11.5,
                          color: "var(--txt3)",
                          lineHeight: 1.5,
                        }}
                      >
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
            <div style={{ display: "flex", flexDirection: "column", gap: 0 }}>
              {/* Summary */}
              <div
                style={{
                  fontSize: 12.5,
                  color: "var(--txt2)",
                  marginBottom: 13,
                  lineHeight: 1.55,
                }}
              >
                {gate.lead}
              </div>

              {/* Diff block */}
              <div
                data-testid="gate-diff"
                style={{
                  borderRadius: 11,
                  border: "1px solid var(--border2)",
                  overflow: "hidden",
                }}
              >
                <DiffView rows={gate.diff_rows} filename={editData.file} />
              </div>
            </div>
          )}

          {/* ---- REJECT REASON (inside scrollable body) ---- */}
          {rejectOpen && (
            <div style={{ marginTop: 15 }}>
              <div
                style={{
                  fontSize: 11,
                  fontWeight: 700,
                  color: "var(--error)",
                  marginBottom: 7,
                  textTransform: "uppercase",
                  letterSpacing: 0.4,
                }}
              >
                Reason for rejection (sent back to the session)
              </div>
              <textarea
                data-testid="reject-reason"
                rows={2}
                placeholder="e.g. Don't run migrations against prod — target the staging DB instead."
                value={reason}
                onChange={(e) => setReason(e.target.value)}
                style={{
                  width: "100%",
                  boxSizing: "border-box",
                  background: "var(--panel2)",
                  border: "1px solid var(--error)",
                  borderRadius: 9,
                  padding: 11,
                  color: "var(--txt)",
                  fontFamily: "inherit",
                  fontSize: 12.5,
                  outline: "none",
                  resize: "vertical",
                }}
              />
            </div>
          )}
        </div>

        {/* ===== FOOTER ===== */}
        <div
          style={{
            flexShrink: 0,
            padding: "15px 22px",
            borderTop: "1px solid var(--border)",
            display: "flex",
            alignItems: "center",
            gap: 10,
            background: "var(--panel)",
          }}
        >
          {/* Route info */}
          <div
            className="mono"
            style={{ fontSize: 10.5, color: "var(--txt3)", flex: 1 }}
          >
            Decision routes only to this session · {routeId}
          </div>

          {/* ---- Reject-open state: Cancel + Send rejection ---- */}
          {rejectOpen ? (
            <>
              <button
                onClick={() => setRejectOpen(false)}
                disabled={busy}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 600,
                  fontSize: 12.5,
                  padding: "9px 15px",
                  borderRadius: 9,
                  border: "1px solid var(--border2)",
                  background: "var(--panel2)",
                  color: "var(--txt2)",
                  cursor: busy ? "not-allowed" : "pointer",
                  opacity: busy ? 0.6 : 1,
                }}
              >
                Cancel
              </button>
              <button
                data-testid="send-rejection-btn"
                onClick={handleReject}
                disabled={busy}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 700,
                  fontSize: 12.5,
                  padding: "9px 18px",
                  borderRadius: 9,
                  border: "none",
                  background: "var(--error)",
                  color: "#fff",
                  cursor: busy ? "not-allowed" : "pointer",
                  opacity: busy ? 0.6 : 1,
                }}
              >
                Send rejection
              </button>
            </>
          ) : (
            <>
              {/* ---- Normal state: Reject… + Approve ---- */}
              <button
                data-testid="reject-btn"
                onClick={() => setRejectOpen(true)}
                disabled={busy}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 600,
                  fontSize: 12.5,
                  padding: "9px 15px",
                  borderRadius: 9,
                  border: "1px solid var(--border2)",
                  background: "var(--panel2)",
                  color: "var(--txt2)",
                  cursor: busy ? "not-allowed" : "pointer",
                  opacity: busy ? 0.6 : 1,
                }}
              >
                Reject…
              </button>
              <button
                data-testid="approve-btn"
                onClick={handleApprove}
                disabled={busy}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 700,
                  fontSize: 12.5,
                  padding: "9px 20px",
                  borderRadius: 9,
                  border: "none",
                  background: "var(--done)",
                  color: "#0e1116",
                  cursor: busy ? "not-allowed" : "pointer",
                  opacity: busy ? 0.6 : 1,
                }}
              >
                {approveLabel(kind)}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
