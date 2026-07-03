/**
 * NewSession modal — project dir, host, model, posture, first prompt, launch.
 */
import React, { useState, useEffect, useMemo } from "react";
import { useVm } from "../bridge/useVm";
import { launchSession, getSettings, capsForHostId, refreshCapabilities } from "../bridge/api";
import type { CapabilitiesView, DiscoveredSession, ModelCatalog } from "../bridge/types";
import { Modal } from "../components/Modal";
import { Toggle } from "../components/Toggle";

// ---------------------------------------------------------------------------
// Capabilities helpers (U2 — dynamic-capabilities.md §5)
// ---------------------------------------------------------------------------

/** The honest empty state — nothing observed yet, free text allowed
 *  (mirrors `ModelCatalog::default()`, proto §1). */
const EMPTY_MODEL_CATALOG: ModelCatalog = { models: [], source: "Observed", free_text: true };

/** "as of …" / "not fetched yet" staleness label. `fetched_at_ms: 0` is the
 *  "never successfully probed" sentinel — never rendered as a 1970 date. */
function formatStaleness(fetchedAtMs: number): string {
  if (!fetchedAtMs) return "Not fetched yet";
  return `as of ${relativeTime(fetchedAtMs)}`;
}

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

function sessionLabel(s: DiscoveredSession): string {
  const time = relativeTime(s.modified_ms);
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

  const [name, setName] = useState("");
  const [project, setProject] = useState("");
  const [hostId, setHostId] = useState(
    hosts.find((h) => h.is_local)?.id ?? hosts[0]?.id ?? "local"
  );
  // No `DEFAULT_MODEL` constant anywhere (dynamic-capabilities.md §5) — the
  // effective default is resolved below once capabilities + settings load.
  const [model, setModel] = useState<string>("");
  // True once the user has explicitly picked/typed a model — after that,
  // switching host must not silently clobber their choice.
  const [userModifiedModel, setUserModifiedModel] = useState(false);
  function selectModel(id: string) {
    setUserModifiedModel(true);
    setModel(id);
  }
  const [posture, setPosture] = useState<"ask" | "auto" | "full">("auto");
  const [defaultModelSetting, setDefaultModelSetting] = useState<string | null>(null);

  // Initialize posture + the persisted default-model preference from settings
  // (once on mount). The model itself is resolved by the effect below, which
  // also needs the capabilities catalog for the selected host.
  useEffect(() => {
    getSettings()
      .then((s) => {
        setDefaultModelSetting(s.default_model);
        const p = s.default_posture as "ask" | "auto" | "full";
        if (p === "ask" || p === "auto" || p === "full") setPosture(p);
      })
      .catch(() => {/* keep defaults on error */});
  }, []);

  // Capabilities for the selected host — options = capabilities[(driver,
  // host)].models ONLY, never an import (dynamic-capabilities.md §5). There is
  // no driver picker in the UI yet (M7d, non-goal), so we key off host id
  // alone; today exactly one driver's capabilities exist per host.
  const capsForHost: CapabilitiesView | undefined = useMemo(
    () => capsForHostId(state.capabilities, hostId),
    [state.capabilities, hostId]
  );
  const modelCatalog: ModelCatalog = capsForHost?.models ?? EMPTY_MODEL_CATALOG;

  // Default selection = persisted last-used model from settings if present in
  // the catalog, else the first catalog entry, else empty (free text). Once
  // the user has explicitly modified the selection, this stops overriding it
  // (e.g. switching host must not clobber an explicit pick).
  useEffect(() => {
    if (userModifiedModel) return;
    if (defaultModelSetting !== null && modelCatalog.models.some((m) => m.id === defaultModelSetting)) {
      setModel(defaultModelSetting);
    } else if (modelCatalog.models.length > 0) {
      setModel(modelCatalog.models[0].id);
    } else {
      setModel("");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [defaultModelSetting, modelCatalog.models, userModifiedModel]);

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

  // Resume state — sessions come ONLY from capabilities[(driver, host)].sessions
  // (dynamic-capabilities.md §5), never the kill-listed `listSessions()`.
  const [resume, setResume] = useState(false);
  const [selectedResume, setSelectedResume] = useState<string>("continue");

  // Opening the picker (or changing project/host while it's open) fires a
  // fire-and-forget refresh; the cached list (below) renders instantly off
  // `state.capabilities` regardless — never spinner-blocked (§3.3/§5).
  useEffect(() => {
    if (!resume) return;
    refreshCapabilities({
      host_id: hostId,
      driver: capsForHost?.driver ?? "claude",
      project: project.trim() || undefined,
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resume, hostId, project]);

  const projectTrim = project.trim();
  const allSessions: DiscoveredSession[] = capsForHost?.sessions ?? [];
  // agy-style `project: null` entries aren't scoped to a requested project —
  // they always render under "unknown project" (§5). String-project entries
  // are filtered to the chosen project.
  const visibleSessions = allSessions.filter(
    (s) => s.project == null || s.project === projectTrim
  );
  const unknownProjectSessions = visibleSessions.filter((s) => s.project == null);
  const knownProjectSessions = visibleSessions.filter((s) => s.project != null);
  const fetchedAtMs = capsForHost?.fetched_at_ms ?? 0;

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
        name: name.trim() || undefined,
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
        {/* ---- Session name (optional) ---- */}
        <div>
          <label
            htmlFor="ns-name"
            style={{ ...sectionLabel, display: "block" }}
          >
            Name{" "}
            <span style={{ opacity: 0.6, fontWeight: 500, textTransform: "none" }}>
              (optional)
            </span>
          </label>
          <input
            id="ns-name"
            data-testid="ns-name-input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Defaults to the project directory name"
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            // Defeat WebView2/Chromium autofill history dropdown on this field.
            name="ns-name-nofill"
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
              outline: "none",
            }}
          />
        </div>

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
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            name="ns-project-nofill"
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
          {/* Whole row toggles resume; the inner Toggle stops propagation so a
              direct click on it doesn't double-fire. */}
          <div
            data-testid="ns-resume-toggle"
            onClick={() => setResume((r) => !r)}
            style={{ display: "flex", alignItems: "center", gap: 10, cursor: "pointer" }}
          >
            <div onClick={(e) => e.stopPropagation()}>
              <Toggle checked={resume} onChange={setResume} />
            </div>
            <span style={{ fontSize: "calc(15px * var(--fs,1))", color: "var(--txt2)", fontWeight: 500 }}>
              Resume a previous session
            </span>
            {resume && (
              <span
                data-testid="ns-capabilities-staleness"
                style={{ marginLeft: "auto", fontSize: "calc(12.5px * var(--fs,1))", color: "var(--txt3)" }}
              >
                {formatStaleness(fetchedAtMs)}
              </span>
            )}
          </div>

          {resume && (
            <div
              style={{
                marginTop: 8,
                display: "flex",
                flexDirection: "column",
                gap: 6,
                maxHeight: 220,
                overflowY: "auto",
              }}
            >
              <div
                data-testid="ns-resume-continue"
                onClick={() => setSelectedResume("continue")}
                style={{
                  padding: "8px 11px",
                  borderRadius: 8,
                  cursor: "pointer",
                  fontSize: "calc(14px * var(--fs,1))",
                  fontWeight: 600,
                  border: selectedResume === "continue" ? "1px solid var(--accent)" : "1px solid var(--border2)",
                  background: selectedResume === "continue" ? "var(--accent-bg)" : "var(--panel2)",
                  color: selectedResume === "continue" ? "var(--accent2)" : "var(--txt2)",
                }}
              >
                Last (continue)
              </div>

              {knownProjectSessions.length > 0 &&
                knownProjectSessions.map((s) => {
                  const selected = selectedResume === s.id;
                  return (
                    <div
                      key={s.id}
                      data-testid={`ns-resume-session-${s.id}`}
                      onClick={() => setSelectedResume(s.id)}
                      style={{
                        padding: "8px 11px",
                        borderRadius: 8,
                        cursor: "pointer",
                        fontSize: "calc(14px * var(--fs,1))",
                        border: selected ? "1px solid var(--accent)" : "1px solid var(--border2)",
                        background: selected ? "var(--accent-bg)" : "var(--panel2)",
                        color: selected ? "var(--accent2)" : "var(--txt2)",
                      }}
                    >
                      {sessionLabel(s)}
                    </div>
                  );
                })}

              {unknownProjectSessions.length > 0 && (
                <>
                  <div
                    data-testid="ns-resume-unknown-project"
                    style={{
                      marginTop: 4,
                      fontSize: "calc(12.5px * var(--fs,1))",
                      fontWeight: 700,
                      textTransform: "uppercase",
                      letterSpacing: 0.4,
                      color: "var(--txt3)",
                    }}
                  >
                    Unknown project
                  </div>
                  {unknownProjectSessions.map((s) => {
                    const selected = selectedResume === s.id;
                    return (
                      <div
                        key={s.id}
                        data-testid={`ns-resume-session-${s.id}`}
                        onClick={() => setSelectedResume(s.id)}
                        style={{
                          padding: "8px 11px",
                          borderRadius: 8,
                          cursor: "pointer",
                          fontSize: "calc(14px * var(--fs,1))",
                          border: selected ? "1px solid var(--accent)" : "1px solid var(--border2)",
                          background: selected ? "var(--accent-bg)" : "var(--panel2)",
                          color: selected ? "var(--accent2)" : "var(--txt2)",
                        }}
                      >
                        {sessionLabel(s)}
                      </div>
                    );
                  })}
                </>
              )}

              {visibleSessions.length === 0 && (
                <div
                  data-testid="ns-resume-hint"
                  style={{
                    fontSize: "calc(14px * var(--fs,1))",
                    color: "var(--txt3)",
                    fontStyle: "italic",
                  }}
                >
                  No previous sessions discovered yet for this project.
                </div>
              )}
            </div>
          )}
        </div>

        {/* ---- Model + Posture side-by-side ---- */}
        <div style={{ display: "flex", gap: 16 }}>
          {/* Model picker — options ONLY from capabilities[(driver, host)].models
              (dynamic-capabilities.md §5); never a static import. */}
          <div style={{ flex: 1 }}>
            <div style={sectionLabel}>Model</div>
            {modelCatalog.source === "Observed" && modelCatalog.models.length > 0 && (
              <div
                data-testid="ns-model-unverified-badge"
                style={{
                  fontSize: "calc(12px * var(--fs,1))",
                  fontWeight: 700,
                  color: "var(--needs, #d9a441)",
                  background: "var(--accent-bg)",
                  border: "1px solid var(--border2)",
                  borderRadius: 6,
                  padding: "3px 7px",
                  display: "inline-block",
                  marginBottom: 6,
                }}
              >
                Unverified — models seen in past sessions
              </div>
            )}
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {modelCatalog.models.map((m) => {
                const selected = model === m.id;
                return (
                  <button
                    key={m.id}
                    data-testid={`ns-model-${m.id}`}
                    onClick={() => selectModel(m.id)}
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
                    {m.id}
                    {m.annotation ? ` — ${m.annotation}` : ""}
                  </button>
                );
              })}
            </div>
            {modelCatalog.models.length === 0 && (
              <div
                data-testid="ns-model-empty-hint"
                style={{
                  fontSize: "calc(14px * var(--fs,1))",
                  color: "var(--txt3)",
                  fontStyle: "italic",
                }}
              >
                No models observed yet for this host — type one below.
              </div>
            )}
            {modelCatalog.free_text && (
              <input
                data-testid="ns-model-freetext"
                value={model}
                onChange={(e) => selectModel(e.target.value)}
                placeholder="Custom model id"
                className="mono"
                autoComplete="off"
                autoCorrect="off"
                spellCheck={false}
                name="ns-model-freetext-nofill"
                style={{
                  width: "100%",
                  boxSizing: "border-box",
                  background: "var(--panel2)",
                  border: "1px solid var(--border2)",
                  borderRadius: 8,
                  padding: "8px 11px",
                  color: "var(--txt)",
                  fontFamily: "var(--font-mono, monospace)",
                  fontSize: "calc(14px * var(--fs,1))",
                  outline: "none",
                  marginTop: 6,
                }}
              />
            )}
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
