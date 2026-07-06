/**
 * NewWorkstreamModal — name + goal, then a two-path choice (SLICE E).
 *
 * "Single session" is the fast lane: create the (empty) workstream, then
 * open the node/add-agent modal with `isFirstNode: true` — Launch there goes
 * straight to Level-3 (docs/design/DESIGN_HANDOFF.md §Modals & overlays).
 * "Build it out" creates the (empty) workstream and drops straight into its
 * graph — the design-mode canvas itself (＋ Add agent, Confirm & launch) is
 * owned by the Graph slice; this modal's job stops at getting there.
 */
import React, { useState } from "react";
import { useVm } from "../bridge/useVm";
import { Modal } from "../components/Modal";

type Path = "quick" | "build" | null;

const sectionLabel: React.CSSProperties = {
  fontSize: "calc(11.5px * var(--fs,1))",
  fontWeight: 700,
  textTransform: "uppercase",
  letterSpacing: 0.5,
  color: "var(--txt3)",
  marginBottom: 6,
};

const textInputStyle: React.CSSProperties = {
  width: "100%",
  boxSizing: "border-box",
  background: "var(--panel2)",
  border: "1px solid var(--border2)",
  borderRadius: 9,
  padding: "10px 12px",
  color: "var(--txt)",
  fontFamily: "inherit",
  fontSize: "calc(14.5px * var(--fs,1))",
  outline: "none",
};

export function NewWorkstreamModal() {
  const { state, dispatch, createWorkstream, goToGraph, openNodeModal, markDraftWorkstream } =
    useVm();

  const open = state.screen === "new-workstream";

  const [name, setName] = useState("");
  const [goal, setGoal] = useState("");
  const [path, setPath] = useState<Path>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function close() {
    dispatch({ type: "SET_SCREEN", screen: "board" });
  }

  async function handleNext() {
    if (!path || busy) return;
    setBusy(true);
    setError(null);
    const wsName = name.trim() || "Untitled workstream";
    const wsGoal = goal.trim();
    const id = `ws-${Date.now()}`;
    try {
      await createWorkstream(id, wsName, wsGoal);
      // BUG #2 — the workstream is empty until an agent is launched. Mark it as
      // a draft so cancelling the node modal / navigating back to the Board
      // discards the orphan instead of leaving it on the Board.
      markDraftWorkstream(id);
      if (path === "quick") {
        openNodeModal({ wsId: id, wsName, isFirstNode: true });
      } else {
        await goToGraph(id);
      }
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  }

  const footnote = error
    ? `Couldn't create the workstream: ${error}`
    : path === "quick"
      ? "Opens the agent setup next"
      : path === "build"
        ? "Opens an empty canvas to add agents"
        : "Choose how to start";

  const nextLabel =
    path === "quick" ? "Configure agent →" : path === "build" ? "Create & build →" : "Continue";

  return (
    <Modal open={open} onClose={close} width={560}>
      {/* ===== HEADER ===== */}
      <div
        style={{
          padding: "18px 22px",
          borderBottom: "1px solid var(--border)",
          display: "flex",
          alignItems: "center",
          gap: 12,
        }}
      >
        <div
          style={{
            width: 34,
            height: 34,
            borderRadius: 9,
            background: "linear-gradient(135deg, var(--accent), var(--accent2))",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "#fff",
            fontWeight: 800,
            fontSize: "calc(18px * var(--fs,1))",
            flexShrink: 0,
          }}
        >
          ＋
        </div>
        <div style={{ flex: 1 }}>
          <h2
            style={{
              fontSize: "calc(18px * var(--fs,1))",
              fontWeight: 700,
              margin: 0,
              letterSpacing: -0.2,
              color: "var(--txt)",
            }}
          >
            New workstream
          </h2>
          <div style={{ fontSize: "calc(13px * var(--fs,1))", color: "var(--txt3)" }}>
            A workstream is one or more agent sessions across your hosts
          </div>
        </div>
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
            fontSize: "calc(15px * var(--fs,1))",
            flexShrink: 0,
          }}
        >
          ✕
        </button>
      </div>

      {/* ===== BODY ===== */}
      <div
        style={{
          padding: "20px 22px",
          display: "flex",
          flexDirection: "column",
          gap: 16,
        }}
      >
        <div>
          <div style={sectionLabel}>Name</div>
          <input
            data-testid="nw-name-input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Release v2.5"
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            name="nw-name-nofill"
            style={textInputStyle}
          />
        </div>

        <div>
          <div style={sectionLabel}>
            Goal{" "}
            <span style={{ opacity: 0.7, textTransform: "none", fontWeight: 500 }}>
              · optional
            </span>
          </div>
          <input
            data-testid="nw-goal-input"
            value={goal}
            onChange={(e) => setGoal(e.target.value)}
            placeholder="What is this workstream trying to achieve?"
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            name="nw-goal-nofill"
            style={textInputStyle}
          />
        </div>

        <div style={{ ...sectionLabel, marginTop: 2 }}>How do you want to start?</div>
        <div style={{ display: "flex", gap: 11 }}>
          {/* Single session */}
          <div
            data-testid="nw-path-quick"
            onClick={() => setPath("quick")}
            style={{
              flex: 1,
              padding: 15,
              borderRadius: 12,
              cursor: "pointer",
              border: `1px solid ${path === "quick" ? "var(--accent)" : "var(--border2)"}`,
              background: path === "quick" ? "var(--accent-bg)" : "var(--panel2)",
            }}
          >
            <div style={{ fontSize: "calc(19px * var(--fs,1))", marginBottom: 7 }}>◆</div>
            <div style={{ fontWeight: 700, fontSize: "calc(14px * var(--fs,1))", marginBottom: 3 }}>
              Single session
            </div>
            <div style={{ fontSize: "calc(12.5px * var(--fs,1))", color: "var(--txt3)", lineHeight: 1.4 }}>
              One agent. Configure and launch right now.
            </div>
          </div>

          {/* Build it out */}
          <div
            data-testid="nw-path-build"
            onClick={() => setPath("build")}
            style={{
              flex: 1,
              padding: 15,
              borderRadius: 12,
              cursor: "pointer",
              border: `1px solid ${path === "build" ? "var(--accent)" : "var(--border2)"}`,
              background: path === "build" ? "var(--accent-bg)" : "var(--panel2)",
            }}
          >
            <div style={{ fontSize: "calc(19px * var(--fs,1))", marginBottom: 7 }}>⛓</div>
            <div style={{ fontWeight: 700, fontSize: "calc(14px * var(--fs,1))", marginBottom: 3 }}>
              Build it out
            </div>
            <div style={{ fontSize: "calc(12.5px * var(--fs,1))", color: "var(--txt3)", lineHeight: 1.4 }}>
              Multiple agents on a canvas you wire together.
            </div>
          </div>
        </div>
      </div>

      {/* ===== FOOTER ===== */}
      <div
        style={{
          padding: "15px 22px",
          borderTop: "1px solid var(--border)",
          display: "flex",
          alignItems: "center",
          gap: 10,
        }}
      >
        <div
          data-testid="nw-footnote"
          className="mono"
          style={{
            fontSize: "calc(12px * var(--fs,1))",
            color: error ? "var(--error)" : "var(--txt3)",
            flex: 1,
          }}
        >
          {footnote}
        </div>
        <button
          onClick={close}
          disabled={busy}
          style={{
            fontFamily: "inherit",
            fontWeight: 600,
            fontSize: "calc(14px * var(--fs,1))",
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
          data-testid="nw-next-btn"
          onClick={handleNext}
          disabled={!path || busy}
          style={{
            fontFamily: "inherit",
            fontWeight: 700,
            fontSize: "calc(14px * var(--fs,1))",
            padding: "9px 18px",
            borderRadius: 9,
            border: "none",
            background: "var(--accent)",
            color: "#fff",
            cursor: !path || busy ? "not-allowed" : "pointer",
            opacity: !path || busy ? 0.5 : 1,
          }}
        >
          {nextLabel}
        </button>
      </div>
    </Modal>
  );
}
