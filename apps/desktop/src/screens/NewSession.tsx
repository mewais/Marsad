/**
 * NewSession modal — project dir, host, model, posture, first prompt, launch.
 */
import React, { useState, useEffect, useMemo } from "react";
import { useVm } from "../bridge/useVm";
import { launchSession, getSettings, capsForHostId, capsKey, refreshCapabilities } from "../bridge/api";
import type { CapabilitiesView, DiscoveredSession, ModelCatalog } from "../bridge/types";
import type { Driver } from "../bridge/workstreamTypes";
import { Modal } from "../components/Modal";
import { Toggle } from "../components/Toggle";

// Fixed enum of drivers Marsad knows about (mirrors `Driver` in
// workstreamTypes.ts) — NOT model data, so not a capabilities-gate violation.
// Availability + models per driver ARE capabilities-driven below.
const DRIVERS: { id: Driver; label: string }[] = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

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
  // Driver picker (#4) — capabilities-driven availability, defaulted to the
  // Settings default_driver on mount.
  const [driver, setDriver] = useState<Driver>("claude");
  const [userModifiedDriver, setUserModifiedDriver] = useState(false);
  // Per-driver default-model preferences (#2), keyed by driver name.
  const [defaultModels, setDefaultModels] = useState<Record<string, string>>({});

  // Initialize posture + driver + the persisted default-model preferences from
  // settings (once on mount). The model itself is resolved by the effect below,
  // which also needs the capabilities catalog for the selected driver/host.
  useEffect(() => {
    getSettings()
      .then((s) => {
        const dm: Record<string, string> = {};
        for (const [d, m] of s.default_models ?? []) dm[d] = m;
        if (dm.claude === undefined && s.default_model) dm.claude = s.default_model;
        setDefaultModels(dm);
        if (!userModifiedDriver && s.default_driver) setDriver(s.default_driver as Driver);
        const p = s.default_posture as "ask" | "auto" | "full";
        if (p === "ask" || p === "auto" || p === "full") setPosture(p);
      })
      .catch(() => {/* keep defaults on error */});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Capabilities for the selected driver+host — options = capabilities[(driver,
  // host)].models ONLY, never an import (dynamic-capabilities.md §5). Falls back
  // to any driver on the host (host-only lookup) when the exact (driver,host)
  // pair hasn't been probed yet — keeps resume/staleness working.
  const capsForDriverHost: CapabilitiesView | undefined = useMemo(
    () => state.capabilities[capsKey(driver, hostId)] ?? capsForHostId(state.capabilities, hostId),
    [state.capabilities, driver, hostId]
  );
  const capsForHost = capsForDriverHost;
  const modelCatalog: ModelCatalog = capsForDriverHost?.models ?? EMPTY_MODEL_CATALOG;

  // #1 model free-text — the custom input is ONLY for a model NOT in the
  // catalog. When `model` matches a catalog entry (a picked family/radio), the
  // box is empty; it shows the typed id only when the user is entering a custom
  // one. Prevents picking `opus` from filling the box with "opus".
  const customModelValue = modelCatalog.models.some((m) => m.id === model) ? "" : model;

  // Driver availability from REAL per-host discovery (dynamic-capabilities.md
  // §5). While caps are absent (discovery in flight) stay optimistic — never
  // grey a loading driver (a loading state shows instead). Once caps are present
  // the truth is authoritative: available iff Marsad has a launch driver
  // (`launchable`) AND the CLI was actually detected on the host
  // (`cli_available`). gemini → launchable:false → greyed; a driver not
  // installed on the host → cli_available:false → greyed.
  function driverDiscovering(d: Driver): boolean {
    return state.capabilities[capsKey(d, hostId)] === undefined;
  }
  function driverAvailable(d: Driver): boolean {
    const c = state.capabilities[capsKey(d, hostId)];
    if (!c) return true; // discovery in flight — optimistic
    if (!c.launchable) return false; // no Marsad driver (e.g. gemini) — hard no
    // A probe that ERRORED/TIMED OUT (common on slow-setup remote hosts, e.g. an
    // HPC login node running `source ~/module.sh`) says NOTHING about whether
    // claude is installed. Never block a launchable driver on a flaky probe: stay
    // optimistic and let the launch be the source of truth. Only grey out when the
    // probe SUCCEEDED (no errors) and the CLI was genuinely absent.
    if (c.errors.length > 0) return true;
    return c.cli_available === true;
  }
  /** Known-unavailable: caps are PRESENT and report the driver as not
   *  launchable (gemini) or its CLI not detected. Absent caps (loading) are
   *  never known-unavailable. */
  function driverKnownUnavailable(d: Driver): boolean {
    const c = state.capabilities[capsKey(d, hostId)];
    return !!c && !driverAvailable(d);
  }
  function selectDriver(d: Driver) {
    if (!driverAvailable(d)) return;
    setUserModifiedDriver(true);
    setDriver(d);
    setUserModifiedModel(false);
  }

  // Default selection = persisted last-used model from settings if present in
  // the catalog, else the first catalog entry, else empty (free text). Once
  // the user has explicitly modified the selection, this stops overriding it
  // (e.g. switching host must not clobber an explicit pick).
  useEffect(() => {
    if (userModifiedModel) return;
    const pref = defaultModels[driver];
    if (pref && (modelCatalog.free_text || modelCatalog.models.some((m) => m.id === pref))) {
      setModel(pref);
    } else if (modelCatalog.models.length > 0) {
      setModel(modelCatalog.models[0].id);
    } else {
      setModel("");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [defaultModels, driver, modelCatalog.models, modelCatalog.free_text, userModifiedModel]);

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

  // Capability discovery (agents + versioned models) is loaded at connect time —
  // a host only reaches "Connected" once every driver's caps have landed. The
  // modal must therefore NOT re-run capability discovery when it's already there:
  // on a slow remote (etx ~47s) that redoes the whole probe AND races the
  // path-scoped session probe. Fire a capability probe ONLY when the exact
  // (driver, host) pair is still absent (never probed, or host still Connecting).
  // NO project here — this is purely "what agents/models exist", not a session
  // listing (that's the effect below, on path-pick).
  useEffect(() => {
    if (!open) return;
    if (state.capabilities[capsKey(driver, hostId)] !== undefined) return;
    refreshCapabilities({ host_id: hostId, driver });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, hostId, driver]);

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
  // #5 loading — caps absent for the selected (driver, host) ⇒ real discovery
  // is in flight. Drives the model-list + resume-list spinners (never blocks UI).
  const capsLoading = state.capabilities[capsKey(driver, hostId)] === undefined;

  // Path-pick session loading: when the resume picker is open and the
  // (host, driver, project) changes, a scoped capability refresh is fired below
  // to list THAT path's resumable sessions. Show a spinner until the fresh
  // snapshot lands — even when caps already exist from connect-time (so
  // `capsLoading` is false and wouldn't show one). Cleared when the caps
  // snapshot's `fetched_at_ms` advances past the value captured at request time.
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const sessionsBaselineRef = React.useRef<number>(fetchedAtMs);
  useEffect(() => {
    // Only when a concrete path is chosen — with no project there is nothing to
    // list, so no probe fires and no spinner should show.
    if (!resume || !projectTrim) return;
    sessionsBaselineRef.current = fetchedAtMs;
    setSessionsLoading(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resume, hostId, driver, projectTrim]);
  useEffect(() => {
    if (sessionsLoading && fetchedAtMs !== sessionsBaselineRef.current) {
      setSessionsLoading(false);
    }
  }, [fetchedAtMs, sessionsLoading]);
  const loadingSessions = capsLoading || sessionsLoading;

  // The ONE session-listing probe: whenever the picker is open and a concrete
  // path is chosen (or host/driver/path changes), list THAT path's resumable
  // sessions. This is the only place the modal sends `project` — the backend
  // dedups per (driver, host, project) and merges results by project, so it can
  // never be swallowed by, or clobber, the connect-time capability probe. Fires
  // unconditionally on path change (no "only if empty" guard) so re-picking a
  // path always refreshes and clears the spinner.
  useEffect(() => {
    if (!resume || !projectTrim) return;
    refreshCapabilities({ host_id: hostId, driver, project: projectTrim });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resume, hostId, driver, projectTrim]);

  function close() {
    // Board is the home screen (Workstreams redesign SLICE A).
    dispatch({ type: "SET_SCREEN", screen: "board" });
  }

  function handleRecentDir(dir: string) {
    setProject(dir);
  }

  async function handleLaunch() {
    if (!project.trim()) return;
    if (!driverAvailable(driver)) return;
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
        driver,
        ...resumeArgs,
      });
      // Navigate to the new session (closes this modal). Seed an optimistic
      // placeholder so the header shows the picked name and the transcript shows
      // "Loading transcript…" during the launch gap.
      const proj = project.trim();
      const basename = proj.replace(/\/+$/, "").split("/").pop() || proj;
      dispatch({
        type: "SELECT_SESSION",
        id: newId,
        optimistic: { name: name.trim() || basename, project: proj, host_id: hostId, driver },
      });
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

        {/* ---- Agent / driver picker (capabilities-driven, #4) ---- */}
        <div>
          <div style={sectionLabel}>Agent</div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {DRIVERS.map((d) => {
              const available = driverAvailable(d.id);
              const selected = driver === d.id;
              const color =
                d.id === "claude"
                  ? "var(--d-claude, #d97757)"
                  : d.id === "codex"
                  ? "var(--d-codex, #3fb6a8)"
                  : "var(--d-gemini, #9b8cf0)";
              return (
                <button
                  key={d.id}
                  data-testid={`ns-driver-${d.id}`}
                  onClick={() => selectDriver(d.id)}
                  disabled={!available}
                  title={!available ? "Not available on this host" : undefined}
                  style={{
                    fontFamily: "inherit",
                    flex: "1 1 110px",
                    fontSize: "calc(14.5px * var(--fs,1))",
                    fontWeight: 600,
                    padding: 10,
                    borderRadius: 9,
                    cursor: available ? "pointer" : "not-allowed",
                    border: selected ? `1px solid ${color}` : "1px solid var(--border2)",
                    background: selected ? "var(--accent-bg)" : "var(--panel2)",
                    color: selected ? color : "var(--txt2)",
                    opacity: available ? 1 : 0.45,
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    gap: 7,
                  }}
                >
                  <span
                    style={{
                      width: 8,
                      height: 8,
                      borderRadius: "50%",
                      background: color,
                      flexShrink: 0,
                    }}
                  />
                  {d.label}
                </button>
              );
            })}
          </div>
          {driverKnownUnavailable(driver) && (
            <div
              data-testid="ns-driver-unavailable-note"
              style={{
                fontSize: "calc(13px * var(--fs,1))",
                color: "var(--txt3)",
                marginTop: 6,
                fontStyle: "italic",
              }}
            >
              {DRIVERS.find((d) => d.id === driver)?.label} isn't available on this host.
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
                          : h.status === "Reconnecting" || h.status === "Connecting"
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

              {loadingSessions && (
                <div
                  data-testid="ns-resume-loading"
                  style={{
                    fontSize: "calc(14px * var(--fs,1))",
                    color: "var(--txt3)",
                    fontStyle: "italic",
                  }}
                >
                  {visibleSessions.length === 0
                    ? "Discovering sessions…"
                    : "Refreshing…"}
                </div>
              )}
              {visibleSessions.length === 0 && !loadingSessions && (
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
            {modelCatalog.models.length === 0 && capsLoading && (
              <div
                data-testid="ns-model-loading"
                style={{
                  fontSize: "calc(14px * var(--fs,1))",
                  color: "var(--txt3)",
                  fontStyle: "italic",
                }}
              >
                Discovering models…
              </div>
            )}
            {modelCatalog.models.length === 0 && !capsLoading && (
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
                value={customModelValue}
                onChange={(e) => selectModel(e.target.value)}
                placeholder="Type your own"
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
          {launchError ? (
            `Launch failed: ${launchError}`
          ) : !project.trim() ? (
            <span data-testid="ns-launch-disabled-reason">Enter a project directory</span>
          ) : !driverAvailable(driver) ? (
            <span data-testid="ns-launch-disabled-reason">
              {driverDiscovering(driver)
                ? "Discovering… "
                : `${DRIVERS.find((d) => d.id === driver)?.label} isn't available on this host`}
            </span>
          ) : (
            "Runs over your existing SSH config · nothing installed inbound"
          )}
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
          disabled={busy || !project.trim() || !driverAvailable(driver)}
          style={{
            fontFamily: "inherit",
            fontWeight: 700,
            fontSize: "calc(15px * var(--fs,1))",
            padding: "9px 20px",
            borderRadius: 9,
            border: "none",
            background: "var(--accent)",
            color: "#fff",
            cursor: busy || !project.trim() || !driverAvailable(driver) ? "not-allowed" : "pointer",
            opacity: busy || !project.trim() || !driverAvailable(driver) ? 0.5 : 1,
          }}
        >
          Launch session →
        </button>
      </div>
    </Modal>
  );
}
