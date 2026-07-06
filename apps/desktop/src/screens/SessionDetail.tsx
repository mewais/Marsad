/**
 * SessionDetail screen — Level-3 "Session" header (title + state pill, mono
 * driver/host/branch/context meta row, "‹ Back" nav) + tab bar
 * (Activity · Changes · Config · Usage · Terminal) + active tab content.
 *
 * Workstreams redesign SLICE D — header polish to docs/design/DESIGN_HANDOFF.md
 * §"LEVEL 3 — Session".
 */
import { useVm } from "../bridge/useVm";
import type { Tab } from "../bridge/types";
import { stopSession, resumeSession, removeSession, capsForHostId } from "../bridge/api";
import { Badge } from "../components/Badge";
import { ActivityTab } from "./tabs/Activity";
import { ChangesTab } from "./tabs/Changes";
import { ConfigTab } from "./tabs/Config";
import { UsageTab } from "./tabs/Usage";
import { TerminalTab } from "./tabs/Terminal";

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
// Driver pill colours — mirrors Board.tsx / graph/GraphScreen.tsx's
// `driverColor` map (kept local here rather than shared since those files
// belong to other redesign slices this one must not touch).
// ---------------------------------------------------------------------------

const DRIVER_COLOR: Record<string, string> = {
  claude: "#d97757",
  codex: "#3fb6a8",
  gemini: "#9b8cf0",
};

/** driver hex -> ~12% tint background, matching the design tokens' "driver
 *  pills use the hex as text + border, over a ~12% tint as background". */
function driverTint(hex: string): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r},${g},${b},0.14)`;
}

// ---------------------------------------------------------------------------
// SessionDetail
// ---------------------------------------------------------------------------

export function SessionDetail() {
  const {
    state,
    dispatch,
    selectedSession,
    selectedDetail,
    selectedGate,
    goToGraph,
    openBoard,
  } = useVm();

  if (!selectedDetail || !selectedSession) {
    return (
      <div style={{ padding: 24, color: "var(--txt3)" }}>
        No session selected.
      </div>
    );
  }

  const { name, host_label, branch, context_pct } = selectedDetail;
  const { id: sessionId, state: sessionState, changed_files, host_id } = selectedSession;
  const activeTab = state.tab;

  // Driver pill — the session's host's capabilities entry carries the
  // driver that's actually running it (dynamic-capabilities.md §3.6; there's
  // no driver picker yet so a host maps to exactly one driver today, same
  // assumption Config.tsx's model picker already makes).
  const driver = capsForHostId(state.capabilities, host_id)?.driver;
  const driverColor = driver ? DRIVER_COLOR[driver] : undefined;

  // "‹ Back" — label = the parent workstream's name when this session was
  // reached via a graph node (state.fromGraph); otherwise the generic
  // "Workstreams" (board's single-session shortcut, or no nav context yet).
  // Click navigates back to that graph, or to the board.
  const wsCard = state.wsId ? state.board?.all.find((c) => c.id === state.wsId) : undefined;
  const backLabel =
    state.fromGraph && state.wsId
      ? (wsCard?.name ?? state.graph?.name ?? state.wsId)
      : "Workstreams";

  function handleBack() {
    if (state.fromGraph && state.wsId) {
      void goToGraph(state.wsId);
    } else {
      openBoard();
    }
  }

  function switchTab(tab: Tab) {
    dispatch({ type: "SET_TAB", tab });
  }

  function openGate() {
    dispatch({ type: "SET_SCREEN", screen: "gate" });
  }

  function handleStop() {
    stopSession(sessionId);
  }

  function handleResume() {
    resumeSession(sessionId);
  }

  function handleRemove() {
    // Delete kills the session (backend removes it). Confirm first — it's
    // destructive and can't be undone.
    const ok =
      typeof window === "undefined" || typeof window.confirm !== "function"
        ? true
        : window.confirm(`Delete session "${name}"? This cannot be undone.`);
    if (!ok) return;
    removeSession(sessionId);
  }

  // A not-live session (user-stopped, restored on relaunch, or process gone) is
  // surfaced as "Stopped" — it offers Resume + Remove instead of Stop + Restart.
  const isStopped = sessionState === "Stale" || sessionState === "Detached";

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
                  fontSize: "calc(18px * var(--fs,1))",
                  fontWeight: 700,
                  margin: 0,
                  letterSpacing: -0.2,
                }}
              >
                {name}
              </h1>
              <Badge state={sessionState} />
            </div>

            {/* Meta row: driver pill (bordered) + host + branch + context */}
            <div
              className="mono"
              style={{
                display: "flex",
                alignItems: "center",
                gap: 13,
                marginTop: 7,
                color: "var(--txt3)",
                fontSize: "calc(11.5px * var(--fs,1))",
                flexWrap: "wrap",
                rowGap: 5,
              }}
            >
              {driver && driverColor && (
                <span
                  data-testid="session-driver-pill"
                  style={{
                    fontWeight: 700,
                    padding: "2px 8px",
                    borderRadius: 6,
                    color: driverColor,
                    background: driverTint(driverColor),
                    border: `1px solid ${driverColor}`,
                  }}
                >
                  {driver}
                </span>
              )}
              <span>▪ {host_label}</span>
              <span>⎇ {branch}</span>
              <span style={{ color: context_pct >= 75 ? "var(--needs)" : "var(--txt3)" }}>
                ctx {Math.round(context_pct)}%
              </span>
            </div>
          </div>

          {/* Action buttons */}
          <div style={{ display: "flex", gap: 7, flexShrink: 0, alignItems: "flex-start" }}>
            {/* Back — label = parent workstream name when arrived from a
                multi-session graph, else "Workstreams" (design's Level-3
                header nav; see docs/design/DESIGN_HANDOFF.md §"LEVEL 3"). */}
            <button
              data-testid="session-back-btn"
              onClick={handleBack}
              style={{
                fontFamily: "inherit",
                fontWeight: 600,
                fontSize: "calc(13px * var(--fs,1))",
                padding: "7px 13px",
                borderRadius: 8,
                border: "1px solid var(--border2)",
                background: "var(--panel2)",
                color: "var(--txt2)",
                cursor: "pointer",
                whiteSpace: "nowrap",
              }}
            >
              ‹ {backLabel}
            </button>

            {/* Answer gate — visible only when session has a pending gate */}
            {selectedGate && (
              <button
                data-testid="answer-gate-btn"
                onClick={openGate}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 700,
                  fontSize: "calc(14.5px * var(--fs,1))",
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

            {/* Lifecycle: running ⇒ Pause; stopped ⇒ Resume; Delete always. */}
            {isStopped ? (
              <button
                data-testid="resume-btn"
                onClick={handleResume}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 600,
                  fontSize: "calc(14.5px * var(--fs,1))",
                  padding: "7px 12px",
                  borderRadius: 8,
                  border: "1px solid var(--border2)",
                  background: "var(--panel2)",
                  color: "var(--txt2)",
                  cursor: "pointer",
                }}
              >
                ▶ Resume
              </button>
            ) : (
              <button
                data-testid="pause-btn"
                onClick={handleStop}
                style={{
                  fontFamily: "inherit",
                  fontWeight: 600,
                  fontSize: "calc(14.5px * var(--fs,1))",
                  padding: "7px 12px",
                  borderRadius: 8,
                  border: "1px solid var(--border2)",
                  background: "var(--panel2)",
                  color: "var(--txt2)",
                  cursor: "pointer",
                }}
              >
                ⏸ Pause
              </button>
            )}

            <button
              data-testid="remove-btn"
              onClick={handleRemove}
              style={{
                fontFamily: "inherit",
                fontWeight: 600,
                fontSize: "calc(14.5px * var(--fs,1))",
                padding: "7px 12px",
                borderRadius: 8,
                border: "1px solid var(--error)",
                background: "var(--error-bg)",
                color: "var(--error)",
                cursor: "pointer",
              }}
            >
              ✕ Delete
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
                  fontSize: "calc(12.5px * var(--fs,1))",
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
                      fontSize: "calc(12.5px * var(--fs,1))",
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
