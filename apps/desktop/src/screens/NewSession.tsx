/**
 * NewSession modal — project dir, host, model, posture, first prompt, launch.
 */
import React, { useState, useEffect } from "react";
import { useVm } from "../bridge/useVm";
import { launchSession, listSessions, getSettings } from "../bridge/api";
import type { SessionSummary } from "../bridge/types";
import { MODELS, DEFAULT_MODEL } from "../bridge/models";
import { Modal } from "../components/Modal";
import { Toggle } from "../components/Toggle";
import { Dropdown } from "../components/Dropdown";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const POSTURES: { value: "ask" | "auto" | "full"; label: string; desc: string }[] =
  [
    { value: "ask", label: "Ask each time", desc: "Gate every action" },
    { value: "auto", label: "Auto-approve safe", desc: "Gate risky actions only" },
    { value: "full", label: "Full autonomy", desc: "No gates" },
  ];

// ---------------------------------------------------------------------------
// Relative-time helper for session dropdown labels
// ---------------------------------------------------------------------------

function relativeTime(epochMs: number): string {
  const diffMs = Date.now() - epochMs;
  const diffSec = Math.floor(diffMs / 1000);
  if (diffSec < 60) return "just now";
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h ago`;
  const diffDay = Math.floor(diffHr / 24);
  return `${diffDay}d ago`;
}

function sessionLabel(s: SessionSummary): string {
  const time = relativeTime(s.modified);
  const snippet = s.snippet.length > 50 ? s.snippet.slice(0, 50) + "…" : s.snippet;
  return `${time} · ${snippet}`;
}

// ---------------------------------------------------------------------------
// Section label style (shared across all field headings)
// ---------------------------------------------------------------------------

const sectionLabel: React.CSSProperties = {
  fontSize: "calc(13.5px * var(--fs,1))",
  fontWeight: 700,
  textTransform: "uppercase",
  letterSpacing: 0.4,
  color: "var(--txt3)",
  marginBottom: 8,
};

// ---------------------------------------------------------------------------
// NewSession
// ---------------------------------------------------------------------------

export function NewSession() {
  const { state, dispatch, hosts } = useVm();

  const open = state.screen === "new-session";

  const [project, setProject] = useState("");
  const [hostId, setHostId] = useState(
    hosts.find((h) => h.is_local)?.id ?? hosts[0]?.id ?? "local"
  );
  const [model, setModel] = useState<string>(DEFAULT_MODEL);
  const [posture, setPosture] = useState<"ask" | "auto" | "full">("auto");

  // Initialize model + posture from persisted session defaults (once on mount).
  useEffect(() => {
    getSettings()
      .then((s) => {
        setModel(s.default_model);
        const p = s.default_posture as "ask" | "auto" | "full";
        if (p === "ask" || p === "auto" || p === "full") setPosture(p);
      })
      .catch(() => {/* keep defaults on error */});
  }, []);
  const [firstPrompt, setFirstPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [launchError, setLaunchError] = useState<string | null>(null);

  // Recent project dirs for the SELECTED host, derived from real session history
  // (distinct projects, most-recent first). Empty when this host has no sessions.
  const recentDirs = React.useMemo(() => {
    const sessions = state.snapshot?.sessions ?? [];
    const seen = new Set<string>();
    const dirs: string[] = [];
    for (const s of sessions) {
      if (s.host_id === hostId && s.project && !seen.has(s.project)) {
        seen.add(s.project);
        dirs.push(s.project);
      }
    }
    return dirs.slice(0, 6);
  }, [state.snapshot, hostId]);

  // Resume state
  const [resume, setResume] = useState(false);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [selectedResume, setSelectedResume] = useState<string>("continue");
  const [loadingSessions, setLoadingSessions] = useState(false);

  // Fetch sessions when project, host, or resume toggle changes. The host
  // matters because a remote host's resumable sessions live on the remote box
  // (listed over its connection), not on this machine. The fetch is async (the
  // Tauri command no longer blocks the UI) and we show a spinner while it runs;
  // a stale-guard drops results from a superseded request.
  useEffect(() => {
    if (!resume || !project.trim()) {
      setLoadingSessions(false);
      return;
    }
    let cancelled = false;
    setLoadingSessions(true);
    setSessions([]);
    listSessions(project.trim(), hostId)
      .then((s) => {
        if (!cancelled) setSessions(s);
      })
      .catch(() => {
        if (!cancelled) setSessions([]);
      })
      .finally(() => {
        if (!cancelled) setLoadingSessions(false);
      });
    return () => {
      cancelled = true;
    };
  }, [resume, project, hostId]);

  function close() {
    dispatch({ type: "SET_SCREEN", screen: "overview" });
  }

  function handleRecentDir(dir: string) {
    setProject(dir);
  }

  async function handleLaunch() {
    if (!project.trim()) return;
    setBusy(true);
    const resumeArgs: { resume_session_id?: string; continue_recent?: boolean } = {};
    if (resume) {
      if (selectedResume === "continue") {
        resumeArgs.continue_recent = true;
      } else {
        resumeArgs.resume_session_id = selectedResume;
      }
    }
    try {
      const newId = await launchSession({
        project: project.trim(),
        host_id: hostId,
        model,
        posture,
        first_prompt: firstPrompt.trim() || undefined,
        ...resumeArgs,
      });
      // Navigate to the new session (closes this modal).
      dispatch({ type: "SELECT_SESSION", id: newId });
    } catch (err) {
      // Don't leave the dialog stuck if the launch fails — surface it.
      console.error("[NewSession] launch failed:", err);
      setLaunchError(String(err));
    } finally {
      setBusy(false);
    }
  }

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
          background: "var(--panel)",
        }}
      >
        {/* Gradient "+" icon box */}
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
            fontSize: "calc(19px * var(--fs,1))",
            flexShrink: 0,
          }}
        >
          +
        </div>

        <div style={{ flex: 1 }}>
          <h2
            style={{
              fontSize: "calc(19px * var(--fs,1))",
              fontWeight: 700,
              margin: 0,
              letterSpacing: -0.2,
              color: "var(--txt)",
            }}
          >
            Launch a new session
          </h2>
          <div style={{ fontSize: "calc(14px * var(--fs,1))", color: "var(--txt3)", marginTop: 2 }}>
            Pick a directory and host — no terminal needed
          </div>
        </div>

        {/* Close button — styled box */}
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
            fontSize: "calc(17px * var(--fs,1))",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexShrink: 0,
            lineHeight: 1,
          }}
        >
          ✕
        </button>
      </div>

      {/* ===== BODY ===== */}
      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "20px 22px",
          display: "flex",
          flexDirection: "column",
          gap: 18,
          background: "var(--panel)",
        }}
      >
        {/* ---- Project directory ---- */}
        <div>
          <div style={sectionLabel}>Project directory</div>
          <input
            id="ns-project"
            data-testid="ns-project-input"
            value={project}
            onChange={(e) => setProject(e.target.value)}
            placeholder="~/code/my-project"
            className="mono"
            style={{
              width: "100%",
              boxSizing: "border-box",
              background: "var(--panel2)",
              border: "1px solid var(--border2)",
              borderRadius: 9,
              padding: "11px 13px",
              color: "var(--txt)",
              fontFamily: "var(--font-mono, monospace)",
              fontSize: "calc(15px * var(--fs,1))",
              outline: "none",
            }}
          />
          {/* Recent dir chips — real per-host history, hidden when empty */}
          {recentDirs.length > 0 && (
          <div
            style={{
              display: "flex",
              flexWrap: "wrap",
              gap: 6,
              marginTop: 8,
            }}
          >
            {recentDirs.map((dir) => (
              <span
                key={dir}
                className="mono"
                onClick={() => handleRecentDir(dir)}
                style={{
                  fontSize: "calc(13px * var(--fs,1))",
                  padding: "5px 9px",
                  borderRadius: 7,
                  border: project === dir
                    ? "1px solid var(--accent)"
                    : "1px solid var(--border)",
                  background: project === dir ? "var(--accent-bg)" : "var(--panel2)",
                  color: project === dir ? "var(--accent2)" : "var(--txt2)",
                  cursor: "pointer",
                  fontFamily: "var(--font-mono, monospace)",
                }}
              >
                {dir}
              </span>
            ))}
          </div>
          )}
        </div>

        {/* ---- Host picker ---- */}
        <div>
          <div style={sectionLabel}>Host</div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {hosts.map((h) => {
              const selected = hostId === h.id;
              return (
                <button
                  key={h.id}
                  data-testid={`ns-host-${h.id}`}
                  onClick={() => setHostId(h.id)}
                  style={{
                    fontFamily: "inherit",
                    flex: "1 1 110px",
                    fontSize: "calc(14.5px * var(--fs,1))",
                    fontWeight: 600,
                    padding: 10,
                    borderRadius: 9,
                    cursor: "pointer",
                    border: selected
                      ? "1px solid var(--accent)"
                      : "1px solid var(--border)",
                    background: selected ? "var(--accent-bg)" : "var(--panel2)",
                    color: selected ? "var(--accent2)" : "var(--txt)",
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    gap: 7,
                  }}
                >
                  <span
                    style={{
                      width: 7,
                      height: 7,
                      borderRadius: "50%",
                      background:
                        h.status === "Connected"
                          ? "var(--done)"
                          : h.status === "Reconnecting"
                          ? "var(--needs)"
                          : "var(--error)",
                      flexShrink: 0,
                    }}
                  />
                  {h.label}
                </button>
              );
            })}
          </div>
          {/* Manage hosts link — inside body, under hosts */}
          <div
            data-testid="ns-manage-hosts"
            onClick={() => dispatch({ type: "SET_SCREEN", screen: "settings" })}
            style={{
              marginTop: 8,
              fontSize: "calc(13.5px * var(--fs,1))",
              color: "var(--accent2)",
              cursor: "pointer",
              fontWeight: 600,
            }}
          >
            Manage hosts in Settings →
          </div>
        </div>

        {/* ---- Resume ---- */}
        <div>
          <div style={sectionLabel}>Resume</div>
          {/* Toggle + session picker on one row; the picker is greyed until the
              toggle is on. */}
          <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
            <div
              data-testid="ns-resume-toggle"
              style={{ display: "flex", alignItems: "center", gap: 10 }}
            >
              <Toggle checked={resume} onChange={setResume} />
              <span style={{ fontSize: "calc(15px * var(--fs,1))", color: "var(--txt2)", fontWeight: 500 }}>
                Resume a previous session
              </span>
            </div>
            <div style={{ marginLeft: "auto" }}>
              <Dropdown
                testId="ns-resume-select"
                value={selectedResume}
                onChange={setSelectedResume}
                disabled={!resume}
                width={260}
                align="right"
                options={[
                  { value: "continue", label: "Last (continue)" },
                  ...sessions.map((s) => ({ value: s.id, label: sessionLabel(s) })),
                ]}
              />
            </div>
          </div>
          {resume && loadingSessions && (
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 8,
                fontSize: "calc(14px * var(--fs,1))",
                color: "var(--txt3)",
                marginTop: 8,
              }}
            >
              <span
                style={{
                  width: 13,
                  height: 13,
                  border: "2px solid var(--border)",
                  borderTopColor: "var(--accent)",
                  borderRadius: "50%",
                  display: "inline-block",
                  animation: "spin 0.7s linear infinite",
                }}
              />
              Loading sessions…
            </div>
          )}
          {resume && !loadingSessions && sessions.length === 0 && (
            <div
              style={{
                fontSize: "calc(14px * var(--fs,1))",
                color: "var(--txt3)",
                marginTop: 8,
                fontStyle: "italic",
              }}
            >
              No previous sessions in this directory
            </div>
          )}
        </div>

        {/* ---- Model + Posture side-by-side ---- */}
        <div style={{ display: "flex", gap: 16 }}>
          {/* Model picker */}
          <div style={{ flex: 1 }}>
            <div style={sectionLabel}>Model</div>
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {MODELS.map((m) => {
                const selected = model === m.id;
                return (
                  <button
                    key={m.id}
                    data-testid={`ns-model-${m.id}`}
                    onClick={() => setModel(m.id)}
                    style={{
                      fontFamily: "inherit",
                      fontSize: "calc(14.5px * var(--fs,1))",
                      fontWeight: 600,
                      padding: "9px 11px",
                      borderRadius: 8,
                      cursor: "pointer",
                      textAlign: "left",
                      border: selected
                        ? "1px solid var(--accent)"
                        : "1px solid var(--border2)",
                      background: selected ? "var(--accent-bg)" : "var(--panel2)",
                      color: selected ? "var(--accent2)" : "var(--txt2)",
                    }}
                  >
                    {m.label}
                  </button>
                );
              })}
            </div>
          </div>

          {/* Permission posture picker */}
          <div style={{ flex: 1 }}>
            <div style={sectionLabel}>Permission posture</div>
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {POSTURES.map((p) => {
                const selected = posture === p.value;
                return (
                  <button
                    key={p.value}
                    data-testid={`ns-posture-${p.value}`}
                    onClick={() => setPosture(p.value)}
                    style={{
                      fontFamily: "inherit",
                      fontSize: "calc(14.5px * var(--fs,1))",
                      fontWeight: 600,
                      padding: "9px 11px",
                      borderRadius: 8,
                      cursor: "pointer",
                      textAlign: "left",
                      border: selected
                        ? "1px solid var(--accent)"
                        : "1px solid var(--border2)",
                      background: selected ? "var(--accent-bg)" : "var(--panel2)",
                      color: selected ? "var(--accent2)" : "var(--txt2)",
                    }}
                  >
                    {p.label}
                  </button>
                );
              })}
            </div>
          </div>
        </div>

        {/* ---- First prompt (optional) ---- */}
        <div>
          <label
            htmlFor="ns-first-prompt"
            style={{ ...sectionLabel, display: "block" }}
          >
            First prompt{" "}
            <span style={{ opacity: 0.6, fontWeight: 500, textTransform: "none" }}>
              (optional)
            </span>
          </label>
          <textarea
            id="ns-first-prompt"
            data-testid="ns-first-prompt"
            value={firstPrompt}
            onChange={(e) => setFirstPrompt(e.target.value)}
            placeholder="What should this session start working on?"
            rows={2}
            style={{
              width: "100%",
              boxSizing: "border-box",
              background: "var(--panel2)",
              border: "1px solid var(--border2)",
              borderRadius: 9,
              padding: "11px 13px",
              color: "var(--txt)",
              fontFamily: "inherit",
              fontSize: "calc(15px * var(--fs,1))",
              resize: "vertical",
              outline: "none",
            }}
          />
        </div>
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
        {/* Informational mono note */}
        <div
          className="mono"
          style={{
            fontSize: "calc(13px * var(--fs,1))",
            color: launchError ? "var(--error)" : "var(--txt3)",
            flex: 1,
            fontFamily: "var(--font-mono, monospace)",
          }}
        >
          {launchError
            ? `Launch failed: ${launchError}`
            : "Runs over your existing SSH config · nothing installed inbound"}
        </div>

        {/* Cancel */}
        <button
          onClick={close}
          disabled={busy}
          style={{
            fontFamily: "inherit",
            fontWeight: 600,
            fontSize: "calc(15px * var(--fs,1))",
            padding: "9px 16px",
            borderRadius: 9,
            border: "1px solid var(--border2)",
            background: "var(--panel2)",
            color: "var(--txt2)",
            cursor: busy ? "not-allowed" : "pointer",
            opacity: busy ? 0.5 : 1,
          }}
        >
          Cancel
        </button>

        {/* Launch */}
        <button
          data-testid="ns-launch-btn"
          onClick={handleLaunch}
          disabled={busy || !project.trim()}
          style={{
            fontFamily: "inherit",
            fontWeight: 700,
            fontSize: "calc(15px * var(--fs,1))",
            padding: "9px 20px",
            borderRadius: 9,
            border: "none",
            background: "var(--accent)",
            color: "#fff",
            cursor: busy || !project.trim() ? "not-allowed" : "pointer",
            opacity: busy || !project.trim() ? 0.5 : 1,
          }}
        >
          Launch session →
        </button>
      </div>
    </Modal>
  );
}
