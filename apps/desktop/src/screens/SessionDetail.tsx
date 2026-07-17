/**
 * SessionDetail screen — Level-3 "Session" header (title + state pill, mono
 * driver/host/branch/context meta row, "‹ Back" nav) + tab bar
 * (Activity · Changes · Config · Usage · Terminal) + active tab content.
 *
 * Workstreams redesign SLICE D — header polish to docs/design/DESIGN_HANDOFF.md
 * §"LEVEL 3 — Session".
 */
import type { CSSProperties } from "react";
import { useVm } from "../bridge/useVm";
import type { Tab } from "../bridge/types";
import { stopSession, resumeSession, removeSession } from "../bridge/api";
import { Badge } from "../components/Badge";
import { ConnHintChip } from "../components/ConnHintChip";
import { canResume, sessionConnHint } from "../bridge/sessionStatus";
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
  opencode: "#f0a050",
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

  const { name, host_label, branch, context_pct, model } = selectedDetail;
  const { id: sessionId, state: sessionState, changed_files } = selectedSession;
  const activeTab = state.tab;

  // Driver pill — the SESSION carries its own driver (SessionView.driver, from
  // the persisted record). A host can expose several drivers, so deriving from
  // the host's capabilities is arbitrary (capsForHostId returns whichever entry
  // iterates first — codex/gemini/claude non-deterministically). Use the
  // session's real driver; default to "claude" if an old view lacks the field.
  const driver = selectedSession.driver ?? "claude";
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

  async function handleRemove() {
    // Delete kills the session (backend removes it). Confirm first — it's
    // destructive and can't be undone.
    const ok =
      typeof window === "undefined" || typeof window.confirm !== "function"
        ? true
        : window.confirm(`Delete session "${name}"? This cannot be undone.`);
    if (!ok) return;
    await removeSession(sessionId);
    // Navigate away so we don't stay on a dead session screen.
    openBoard();
  }

  // Two-axis lifecycle: the Pause↔Resume affordance is driven by LIVENESS, not
  // the activity phase. A live (Resumed) session — even one that has finished a
  // turn (Done) — is running on its host and offers Pause. A not-live (Paused)
  // session — user-stopped, restored-but-dead, or host-killed (OOM/crash) —
  // offers Resume (a cold `--resume`). `live` is authoritative from the agent;
  // missing (older snapshots) is treated as live (Resumed).
  const isStopped = canResume(selectedSession);

  // Connectivity hint (NOT a lifecycle change): if this session's stream is
  // (re)connecting/unstable, or its host link is unreachable, surface it next to
  // the host label so the user knows we've temporarily lost contact — the session
  // itself stays Resumed because it's still alive on the host. Derived from the
  // SHARED sessionStatus helper (same source the dashboard uses), preferring the
  // per-session `conn_health` over the coarse host poll.
  const connHint = sessionConnHint(selectedSession, state.snapshot?.hosts ?? []);

  // Shared button style for all action buttons in the header — ensures uniform
  // height/padding/font across Back, Answer gate, Pause/Resume, Delete.
  const headerBtnBase: CSSProperties = {
    fontFamily: "inherit",
    fontWeight: 600,
    fontSize: "calc(14.5px * var(--fs,1))",
    padding: "7px 13px",
    borderRadius: 8,
    cursor: "pointer",
    minWidth: 84,
    whiteSpace: "nowrap",
    height: 34,
    boxSizing: "border-box",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
  };

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
              <Badge
                state={sessionState}
                endReason={selectedSession.end_reason}
                live={selectedSession.live}
              />
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
              {connHint && <ConnHintChip hint={connHint} size="xs" />}
              <span>⎇ {branch}</span>
              {model && (
                <span
                  data-testid="session-model"
                  style={{ color: "var(--txt3)" }}
                >
                  {model}
                </span>
              )}
              <span style={{ color: context_pct >= 75 ? "var(--needs)" : "var(--txt3)" }}>
                ctx {Math.round(context_pct)}%
              </span>
            </div>
          </div>

          {/* Action buttons — all use headerBtnBase for uniform height/padding */}
          <div style={{ display: "flex", gap: 7, flexShrink: 0, alignItems: "center" }}>
            {/* Back — label = parent workstream name when arrived from a
                multi-session graph, else "Workstreams" (design's Level-3
                header nav; see docs/design/DESIGN_HANDOFF.md §"LEVEL 3"). */}
            <button
              data-testid="session-back-btn"
              onClick={handleBack}
              style={{
                ...headerBtnBase,
                border: "1px solid var(--border2)",
                background: "var(--panel2)",
                color: "var(--txt2)",
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
                  ...headerBtnBase,
                  fontWeight: 700,
                  border: "1px solid var(--needs)",
                  background: "var(--needs-bg)",
                  color: "var(--needs)",
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
                  ...headerBtnBase,
                  border: "1px solid var(--done)",
                  background: "var(--done-bg)",
                  color: "var(--done)",
                }}
              >
                ▶ Resume
              </button>
            ) : (
              <button
                data-testid="pause-btn"
                onClick={handleStop}
                style={{
                  ...headerBtnBase,
                  border: "1px solid var(--border2)",
                  background: "var(--panel2)",
                  color: "var(--txt2)",
                }}
              >
                ⏸ Pause
              </button>
            )}

            <button
              data-testid="remove-btn"
              onClick={handleRemove}
              style={{
                ...headerBtnBase,
                border: "1px solid var(--error)",
                background: "var(--error-bg)",
                color: "var(--error)",
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
        {/* The terminal's xterm instance is cached module-side (keyed per
            session) and survives unmount, so a plain conditional render is fine:
            switching tabs/sessions detaches it and returning reattaches it with
            full scrollback. See TerminalTab's LIVENESS note. */}
        {activeTab === "Terminal" && <TerminalTab sessionId={sessionId} />}
      </div>
    </div>
  );
}
