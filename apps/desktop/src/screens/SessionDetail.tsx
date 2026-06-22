/**
 * SessionDetail screen — header (name, state badge, host/branch/model, action buttons)
 * + tab bar (Activity · Changes · Config · Usage · Terminal) + active tab content.
 */
import { useVm } from "../bridge/useVm";
import type { Tab } from "../bridge/types";
import { stopSession, resumeSession } from "../bridge/api";
import { Badge } from "../components/Badge";
import { ActivityTab } from "./tabs/Activity";
import { ChangesTab } from "./tabs/Changes";
import { ConfigTab } from "./tabs/Config";
import { UsageTab } from "./tabs/Usage";
import { TerminalTab } from "./tabs/Terminal";
import { Branch, Pause, Restart } from "../components/Icon";

// ---------------------------------------------------------------------------
// Tab bar definition
// ---------------------------------------------------------------------------

const TABS: { id: Tab; label: string }[] = [
  { id: "Activity", label: "Activity" },
  { id: "Changes", label: "Changes" },
  { id: "Config", label: "Config" },
  { id: "Usage", label: "Usage" },
  { id: "Terminal", label: "Terminal" },
];

// ---------------------------------------------------------------------------
// SessionDetail
// ---------------------------------------------------------------------------

export function SessionDetail() {
  const { state, dispatch, selectedSession, selectedDetail, selectedGate } =
    useVm();

  if (!selectedDetail || !selectedSession) {
    return (
      <div style={{ padding: 24, color: "var(--txt3)" }}>
        No session selected.
      </div>
    );
  }

  const { name, host_label, branch, model, ctx_warn, context_pct } =
    selectedDetail;
  const { id: sessionId, state: sessionState, changed_files } = selectedSession;
  const activeTab = state.tab;

  function switchTab(tab: Tab) {
    dispatch({ type: "SET_TAB", tab });
  }

  function openGate() {
    dispatch({ type: "SET_SCREEN", screen: "gate" });
  }

  function handleStop() {
    stopSession(sessionId);
  }

  function handleRestart() {
    resumeSession(sessionId);
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        overflow: "hidden",
      }}
    >
      {/* ------------------------------------------------------------------ */}
      {/* Header                                                              */}
      {/* ------------------------------------------------------------------ */}
      <div
        style={{
          padding: "16px 24px 0",
          borderBottom: "1px solid var(--border)",
          flexShrink: 0,
        }}
      >
        {/* Top row: name + badge + action buttons */}
        <div style={{ display: "flex", alignItems: "flex-start", gap: 14 }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            {/* Name + badge */}
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                flexWrap: "wrap",
              }}
            >
              <h1
                data-testid="session-name"
                style={{
                  fontSize: 18,
                  fontWeight: 700,
                  margin: 0,
                  letterSpacing: -0.2,
                }}
              >
                {name}
              </h1>
              <Badge state={sessionState} />
            </div>

            {/* Meta row: host / branch / project / model */}
            <div
              className="mono"
              style={{
                display: "flex",
                alignItems: "center",
                gap: 0,
                marginTop: 7,
                color: "var(--txt3)",
                fontSize: 11.5,
                flexWrap: "wrap",
                rowGap: 4,
              }}
            >
              <span>
                {sessionState === "Working" || sessionState === "Starting"
                  ? "●"
                  : "○"}{" "}
                {host_label}
              </span>
              <span style={{ margin: "0 8px", opacity: 0.4 }}>·</span>
              <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}><Branch size={12} /> {branch}</span>
              <span style={{ margin: "0 8px", opacity: 0.4 }}>·</span>
              <span>{selectedSession.project}</span>
              <span style={{ margin: "0 8px", opacity: 0.4 }}>·</span>
              <span>{model}</span>
              {ctx_warn && (
                <>
                  <span style={{ margin: "0 8px", opacity: 0.4 }}>·</span>
                  <span style={{ color: "var(--needs)" }}>
                    ⚠ {context_pct}% ctx
                  </span>
                </>
              )}
            </div>
          </div>

          {/* Action buttons */}
          <div style={{ display: "flex", gap: 7, flexShrink: 0 }}>
            {/* Answer gate — visible only when session has a pending gate */}
            {selectedGate && (
              <button
                data-testid="answer-gate-btn"
                onClick={openGate}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 700,
                  fontSize: 12,
                  padding: "7px 13px",
                  borderRadius: 8,
                  border: "1px solid var(--needs)",
                  background: "var(--needs-bg)",
                  color: "var(--needs)",
                  cursor: "pointer",
                }}
              >
                Answer gate
              </button>
            )}

            <button
              data-testid="stop-btn"
              onClick={handleStop}
              style={{
                fontFamily: "inherit",
                fontWeight: 600,
                fontSize: 12,
                padding: "7px 12px",
                borderRadius: 8,
                border: "1px solid var(--border2)",
                background: "var(--panel2)",
                color: "var(--txt2)",
                cursor: "pointer",
                display: "flex",
                alignItems: "center",
                gap: 5,
              }}
            >
              <Pause size={13} /> Stop
            </button>

            <button
              data-testid="restart-btn"
              onClick={handleRestart}
              style={{
                fontFamily: "inherit",
                fontWeight: 600,
                fontSize: 12,
                padding: "7px 12px",
                borderRadius: 8,
                border: "1px solid var(--border2)",
                background: "var(--panel2)",
                color: "var(--txt2)",
                cursor: "pointer",
                display: "flex",
                alignItems: "center",
                gap: 5,
              }}
            >
              <Restart size={13} /> Restart
            </button>
          </div>
        </div>

        {/* Tab bar */}
        <div style={{ display: "flex", gap: 2, marginTop: 14 }}>
          {TABS.map((t) => {
            const isActive = activeTab === t.id;
            // Show file count badge on Changes tab
            const showCount = t.id === "Changes" && changed_files > 0;
            return (
              <button
                key={t.id}
                data-testid={`tab-${t.id}`}
                onClick={() => switchTab(t.id)}
                style={{
                  fontFamily: "inherit",
                  fontSize: 12.5,
                  fontWeight: 600,
                  padding: "9px 14px",
                  border: "none",
                  background: "none",
                  cursor: "pointer",
                  borderBottom: isActive
                    ? "2px solid var(--accent)"
                    : "2px solid transparent",
                  color: isActive ? "var(--txt)" : "var(--txt3)",
                  display: "flex",
                  alignItems: "center",
                  gap: 4,
                }}
              >
                {t.label}
                {showCount && (
                  <span
                    style={{
                      marginLeft: 5,
                      fontSize: 10,
                      padding: "1px 5px",
                      borderRadius: 5,
                      background: "var(--panel2)",
                      color: "var(--txt3)",
                    }}
                  >
                    {changed_files}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </div>

      {/* ------------------------------------------------------------------ */}
      {/* Active tab content                                                  */}
      {/* ------------------------------------------------------------------ */}
      <div style={{ flex: 1, overflow: "hidden", display: "flex", flexDirection: "column" }}>
        {activeTab === "Activity" && (
          <ActivityTab sessionId={sessionId} onOpenGate={openGate} />
        )}
        {activeTab === "Changes" && <ChangesTab />}
        {activeTab === "Config" && <ConfigTab />}
        {activeTab === "Usage" && <UsageTab />}
        {activeTab === "Terminal" && <TerminalTab sessionId={sessionId} />}
      </div>
    </div>
  );
}
