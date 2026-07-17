/**
 * NodeModal — add / configure an agent (SLICE E).
 *
 * The Node modal from docs/design/DESIGN_HANDOFF.md §Modals & overlays: the
 * classic launch form (name, project dir + recent-dir chips, host, model
 * override, permission override, first prompt) PLUS an Agent/driver picker
 * (capabilities-driven — never a hardcoded model/driver list) and inline
 * "Controlled by" / "Feeds from" pickers so the new node wires to existing
 * ones without dragging. First agent added becomes the effective lead.
 *
 * Opened via `useVm().openNodeModal({ wsId, wsName, isFirstNode })` — today
 * from NewWorkstreamModal's "Single session" fast lane. `isFirstNode: true`
 * routes Launch straight to Level-3 (the graph/design-mode "＋ Add agent"
 * entry point is the Graph slice's job to wire; this component is ready for
 * it — pass `isFirstNode: false` and it renders Controlled-by/Feeds-from).
 */
import React, { useEffect, useMemo, useState } from "react";
import { useVm } from "../bridge/useVm";
import { launchSession, fetchGraph, capsKey, getSettings, refreshCapabilities } from "../bridge/api";
import type { CapabilitiesView, DiscoveredSession, ModelCatalog } from "../bridge/types";
import type { Driver, GraphNode } from "../bridge/workstreamTypes";
import { Modal } from "../components/Modal";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const EMPTY_MODEL_CATALOG: ModelCatalog = { models: [], source: "Observed", free_text: true };

/** Fixed enum of drivers Marsad knows about (mirrors `Driver` in
 *  workstreamTypes.ts) — NOT model data, so this is not a capabilities-gate
 *  violation. Availability/models for each ARE capabilities-driven below. */
const DRIVERS: { id: Driver; label: string }[] = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
];

/** Per-driver accent (design tokens `--d-claude/--d-codex/--d-gemini`, with
 *  literal fallbacks matching Board/GraphScreen's driverColor map). */
const DRIVER_COLOR: Record<Driver, string> = {
  claude: "var(--d-claude, #d97757)",
  codex: "var(--d-codex, #3fb6a8)",
  gemini: "var(--d-gemini, #9b8cf0)",
  opencode: "var(--d-opencode, #f0a050)",
};

const POSTURES: { value: "ask" | "auto" | "full"; label: string }[] = [
  { value: "ask", label: "Ask each time" },
  { value: "auto", label: "Auto-approve safe" },
  { value: "full", label: "Full autonomy" },
];

// Resume-label helpers (mirror NewSession's).
function relativeTime(epochMs: number): string {
  const diffSec = Math.floor((Date.now() - epochMs) / 1000);
  if (diffSec < 60) return "just now";
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h ago`;
  return `${Math.floor(diffHr / 24)}d ago`;
}

function sessionLabel(s: DiscoveredSession): string {
  const snippet = s.snippet.length > 50 ? s.snippet.slice(0, 50) + "…" : s.snippet;
  return `${relativeTime(s.modified_ms)} · ${snippet}`;
}

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
  fontSize: "calc(14px * var(--fs,1))",
  outline: "none",
};

function pickerBtnStyle(selected: boolean, disabled?: boolean): React.CSSProperties {
  return {
    fontFamily: "inherit",
    fontSize: "calc(13px * var(--fs,1))",
    fontWeight: 600,
    padding: "9px 13px",
    borderRadius: 9,
    cursor: disabled ? "not-allowed" : "pointer",
    textAlign: "left",
    border: selected ? "1px solid var(--accent)" : "1px solid var(--border2)",
    background: selected ? "var(--accent-bg)" : "var(--panel2)",
    color: selected ? "var(--accent2)" : "var(--txt2)",
    opacity: disabled ? 0.45 : 1,
  };
}

// ---------------------------------------------------------------------------
// NodeModal
// ---------------------------------------------------------------------------

export function NodeModal() {
  const {
    state,
    dispatch,
    hosts,
    assignSession,
    addConnection,
    goToGraph,
    deleteWorkstream,
    markDraftWorkstream,
  } = useVm();
  const ctx = state.nodeModalCtx;
  const open = state.screen === "node-modal" && ctx !== null;

  // BUG #2 — a "Single session" fast-lane workstream is created (empty) BEFORE
  // this modal opens. If the user cancels without ever launching an agent, that
  // empty draft must be discarded (otherwise it lingers on the Board and has to
  // be deleted by hand). We only treat the first-node modal as a discardable
  // draft, and only when it never produced a session — a successful launch sets
  // this ref so the finally/close path leaves the (now non-empty) workstream be.
  const launchedRef = React.useRef(false);

  const [name, setName] = useState("");
  const [project, setProject] = useState("");
  const [hostId, setHostId] = useState(
    hosts.find((h) => h.is_local)?.id ?? hosts[0]?.id ?? "local"
  );
  const [driver, setDriver] = useState<Driver>("claude");
  const [userModifiedDriver, setUserModifiedDriver] = useState(false);
  const [model, setModel] = useState("");
  const [userModifiedModel, setUserModifiedModel] = useState(false);
  const [posture, setPosture] = useState<"ask" | "auto" | "full">("auto");
  // #2 pre-pick — Settings-persisted defaults (default_driver / default_models
  // per driver / default_posture) resolved on mount. Model defaults are keyed
  // by driver so switching driver picks that driver's default.
  const [defaultModels, setDefaultModels] = useState<Record<string, string>>({});
  const [firstPrompt, setFirstPrompt] = useState("");
  const [controller, setController] = useState<string | null>(null);
  const [feeder, setFeeder] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [launchError, setLaunchError] = useState<string | null>(null);
  const [existingNodes, setExistingNodes] = useState<GraphNode[]>([]);
  // #3 resume/continue — mirrors NewSession. `selectedResume === "continue"`
  // maps to continue_recent; any other value is a resume_session_id.
  const [resume, setResume] = useState(false);
  const [selectedResume, setSelectedResume] = useState<string>("continue");

  // #2 pre-pick — load Settings defaults once on mount (default driver,
  // per-driver default model, default posture). Absent settings degrade to the
  // capabilities-first-model / "auto" fallbacks used before.
  useEffect(() => {
    getSettings()
      .then((s) => {
        const dm: Record<string, string> = {};
        for (const [d, m] of s.default_models ?? []) dm[d] = m;
        // default_model is the legacy claude default — honor it if no explicit
        // per-driver claude entry exists.
        if (dm.claude === undefined && s.default_model) dm.claude = s.default_model;
        setDefaultModels(dm);
        if (!userModifiedDriver && s.default_driver) {
          setDriver(s.default_driver as Driver);
        }
        const p = s.default_posture as "ask" | "auto" | "full";
        if (p === "ask" || p === "auto" || p === "full") setPosture(p);
      })
      .catch(() => {/* keep fallbacks on error */});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Capability discovery (agents + versioned models) is loaded at connect time —
  // a host only reaches "Connected" once every driver's caps have landed. Do NOT
  // re-run it from the modal when it's already present: on a slow remote (etx
  // ~47s) that redoes the whole probe AND races the path-scoped session probe.
  // Fire ONLY when the exact (driver, host) pair is still absent, and with NO
  // project (this is "what agents/models exist", not a session listing).
  useEffect(() => {
    if (!open) return;
    if (state.capabilities[capsKey(driver, hostId)] !== undefined) return;
    refreshCapabilities({ host_id: hostId, driver });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, hostId, driver]);

  // Existing nodes in this workstream — for the Controlled-by/Feeds-from
  // pickers. A brand-new "single session" workstream has none (nmHasNodes).
  useEffect(() => {
    if (!ctx || ctx.isFirstNode) {
      setExistingNodes([]);
      return;
    }
    let cancelled = false;
    fetchGraph(ctx.wsId).then((g) => {
      if (!cancelled) setExistingNodes(g?.nodes ?? []);
    });
    return () => {
      cancelled = true;
    };
  }, [ctx?.wsId, ctx?.isFirstNode]);

  // Recent project dirs for the selected host, from real session history.
  const recentDirs = useMemo(() => {
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

  // Capabilities-driven driver availability + model catalog — NEVER a
  // hardcoded list (dynamic-capabilities.md §5). A driver with no reported
  // capabilities (or cli_available: false) is disabled with a note; today
  // that's codex/gemini until the launch backend grows a driver parameter.
  function capsForDriver(d: Driver): CapabilitiesView | undefined {
    return state.capabilities[capsKey(d, hostId)];
  }
  /** True while REAL discovery for this (driver, host) hasn't landed yet — caps
   *  are absent. We stay optimistic during this window (never grey), and show a
   *  loading indicator instead of the empty state. */
  function driverDiscovering(d: Driver): boolean {
    return capsForDriver(d) === undefined;
  }
  // Driver availability from REAL per-host discovery (dynamic-capabilities.md
  // §5). While caps are absent (discovery in flight) stay optimistic — never
  // grey a loading driver. Once caps are present the truth is authoritative:
  // available iff Marsad has a launch driver (`launchable`) AND the CLI was
  // actually detected on the host (`cli_available`). gemini → launchable:false
  // → greyed; a driver not installed on the host → cli_available:false → greyed.
  function driverAvailable(d: Driver): boolean {
    const c = capsForDriver(d);
    if (!c) return true; // discovery in flight — optimistic
    if (!c.launchable) return false; // no Marsad driver (e.g. gemini) — hard no
    // A probe that ERRORED/TIMED OUT (slow-setup remote hosts) says nothing about
    // whether claude is installed. Don't block a launchable driver on a flaky
    // probe; only grey out when the probe SUCCEEDED (no errors) and cli was absent.
    if (c.errors.length > 0) return true;
    return c.cli_available === true;
  }
  /** Known-unavailable: caps are PRESENT and report the driver as not
   *  launchable (gemini) or its CLI not detected. Absent caps (loading) are
   *  never known-unavailable. */
  function driverKnownUnavailable(d: Driver): boolean {
    const c = capsForDriver(d);
    return !!c && !driverAvailable(d);
  }

  const modelCatalog: ModelCatalog = capsForDriver(driver)?.models ?? EMPTY_MODEL_CATALOG;

  // #1 model free-text — the custom input is ONLY for a model NOT in the
  // catalog. When `model` matches a catalog entry (a picked family/radio), the
  // box is empty; it shows the typed id only when the user is entering a custom
  // one. Prevents picking `opus` from filling the box with "opus".
  const customModelValue = modelCatalog.models.some((m) => m.id === model) ? "" : model;

  // #3 resume — sessions come ONLY from capabilities[(driver, host)].sessions
  // (populated by scan_claude_sessions once a project is in scope). String-
  // project entries are filtered to the chosen project; agy-style null-project
  // entries always show (they aren't scoped to a requested project).
  const projectTrim = project.trim();
  const driverCaps = capsForDriver(driver);
  const allSessions: DiscoveredSession[] = driverCaps?.sessions ?? [];
  const visibleSessions = allSessions.filter(
    (s) => s.project == null || s.project === projectTrim
  );
  // #5 loading — caps absent for the selected (driver, host) ⇒ real discovery
  // is in flight. Drives the model-list + resume-list spinners.
  const capsLoading = driverCaps === undefined;
  const fetchedAtMs = driverCaps?.fetched_at_ms ?? 0;
  // Path-pick session loading (mirrors NewSession): show a spinner while the
  // scoped refresh for a newly-picked path lands, even when caps already exist.
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const sessionsBaselineRef = React.useRef<number>(fetchedAtMs);
  useEffect(() => {
    // Only when a concrete path is chosen — no project, nothing to list, no spinner.
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
  // sessions. This is the only place the modal sends `project`; the backend dedups
  // per (driver, host, project) and merges by project, so it can never be
  // swallowed by, or clobber, the connect-time capability probe. Fires
  // unconditionally on path change so re-picking always refreshes + clears the spinner.
  useEffect(() => {
    if (!resume || !projectTrim) return;
    refreshCapabilities({ host_id: hostId, driver, project: projectTrim });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resume, hostId, driver, projectTrim]);

  useEffect(() => {
    if (userModifiedModel) return;
    // Prefer the Settings per-driver default when it's a real option in this
    // (driver,host)'s catalog (or when free text is allowed); else the
    // newest/first catalog model; else empty.
    const pref = defaultModels[driver];
    if (pref && (modelCatalog.free_text || modelCatalog.models.some((m) => m.id === pref))) {
      setModel(pref);
    } else if (modelCatalog.models.length > 0) {
      setModel(modelCatalog.models[0].id);
    } else {
      setModel("");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modelCatalog.models, modelCatalog.free_text, userModifiedModel, defaultModels, driver]);

  function selectModel(id: string) {
    setUserModifiedModel(true);
    setModel(id);
  }

  function selectDriver(d: Driver) {
    if (!driverAvailable(d)) return;
    setUserModifiedDriver(true);
    setDriver(d);
    setUserModifiedModel(false);
  }

  function close() {
    // BUG #2 — discard a just-created empty draft on cancel. Only when this is
    // the first-node fast lane (`isFirstNode`), no agent was launched in this
    // modal session, AND the workstream genuinely has 0 sessions right now.
    // Never delete a workstream that already has sessions (the user might just
    // be adding another agent and cancelling that).
    if (ctx && ctx.isFirstNode && !launchedRef.current) {
      const card = state.board?.all.find((c) => c.id === ctx.wsId);
      const sessionCount = card?.session_count ?? 0;
      if (sessionCount === 0) {
        void deleteWorkstream(ctx.wsId);
      }
      // Draft consumed (discarded above, or never a draft) — clear the marker
      // so a later openBoard doesn't try to discard it again.
      markDraftWorkstream(null);
    }
    dispatch({ type: "SET_SCREEN", screen: "board" });
  }

  async function handleLaunch() {
    if (!ctx || busy) return;
    if (!project.trim()) return;
    if (!driverAvailable(driver)) return;

    const typedName = name.trim();
    if (!ctx.isFirstNode && !typedName) {
      setLaunchError("Agent name is required");
      return;
    }
    const sessionName = ctx.isFirstNode ? typedName || ctx.wsName || undefined : typedName;

    setBusy(true);
    setLaunchError(null);
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
        name: sessionName,
        driver,
        ...resumeArgs,
      });
      // A session now exists for this workstream — it is no longer an empty
      // draft, so cancelling afterwards must NOT discard it (BUG #2).
      launchedRef.current = true;
      markDraftWorkstream(null);
      // First agent added = effective lead (isFirstNode is exactly that).
      await assignSession(newId, ctx.wsId, ctx.isFirstNode);
      if (controller) {
        await addConnection(`conn-${newId}-ctrl`, ctx.wsId, controller, newId, "command", false);
      }
      if (feeder) {
        await addConnection(`conn-${newId}-feed`, ctx.wsId, feeder, newId, "data", false);
      }
      if (ctx.isFirstNode) {
        // Straight to Level-3 — never via the graph (single-session nav rule).
        // Seed an optimistic placeholder so the header shows the picked name and
        // the transcript shows "Loading transcript…" until the real view lands.
        const proj = project.trim();
        const basename = proj.replace(/\/+$/, "").split("/").pop() || proj;
        dispatch({
          type: "SELECT_SESSION",
          id: newId,
          fromGraph: false,
          wsId: ctx.wsId,
          optimistic: { name: sessionName || basename, project: proj, host_id: hostId, driver },
        });
      } else {
        await goToGraph(ctx.wsId);
      }
    } catch (err) {
      console.error("[NodeModal] launch failed:", err);
      setLaunchError(String(err));
    } finally {
      setBusy(false);
    }
  }

  if (!ctx) {
    // Keep mounted (for test/route compatibility) but render nothing visible.
    return <div style={{ display: "none" }} data-testid="node-modal-hidden" aria-hidden="true" />;
  }

  const launchLabel = ctx.isFirstNode ? "Launch session →" : "Add agent →";

  return (
    <Modal open={open} onClose={close} width={600}>
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
            Add an agent
          </h2>
          <div style={{ fontSize: "calc(13px * var(--fs,1))", color: "var(--txt3)" }}>
            to {ctx.wsName}
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
      <div style={{ padding: "20px 22px", display: "flex", flexDirection: "column", gap: 16 }}>
        {/* Agent / driver picker — capabilities-driven */}
        <div>
          <div style={sectionLabel}>Agent</div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {DRIVERS.map((d) => {
              const available = driverAvailable(d.id);
              const selected = driver === d.id;
              return (
                <button
                  key={d.id}
                  data-testid={`nm-driver-${d.id}`}
                  onClick={() => selectDriver(d.id)}
                  disabled={!available}
                  title={!available ? "Not available on this host" : undefined}
                  style={{
                    ...pickerBtnStyle(selected, !available),
                    display: "flex",
                    alignItems: "center",
                    gap: 7,
                    borderColor: selected ? DRIVER_COLOR[d.id] : "var(--border2)",
                    color: selected ? DRIVER_COLOR[d.id] : "var(--txt2)",
                  }}
                >
                  <span
                    style={{
                      width: 8,
                      height: 8,
                      borderRadius: "50%",
                      background: DRIVER_COLOR[d.id],
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
              data-testid="nm-driver-unavailable-note"
              style={{ fontSize: "calc(12px * var(--fs,1))", color: "var(--txt3)", marginTop: 6, fontStyle: "italic" }}
            >
              {DRIVERS.find((d) => d.id === driver)?.label} isn't available on this host.
            </div>
          )}
        </div>

        {/* Name — hidden on the "single session" fast lane (defaults to the
            workstream name / project dir basename). */}
        {!ctx.isFirstNode && (
          <div>
            <div style={sectionLabel}>Name</div>
            <input
              data-testid="nm-name-input"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Build frontend bundle"
              autoComplete="off"
              autoCorrect="off"
              spellCheck={false}
              name="nm-name-nofill"
              style={textInputStyle}
            />
          </div>
        )}

        {/* Project directory + recent-dir chips */}
        <div>
          <div style={sectionLabel}>Project directory</div>
          <input
            data-testid="nm-project-input"
            value={project}
            onChange={(e) => setProject(e.target.value)}
            placeholder="~/code/my-project"
            className="mono"
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            name="nm-project-nofill"
            style={{ ...textInputStyle, fontFamily: "var(--font-mono, monospace)" }}
          />
          {recentDirs.length > 0 && (
            <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginTop: 8 }}>
              {recentDirs.map((dir) => (
                <span
                  key={dir}
                  data-testid={`nm-recentdir-${dir}`}
                  className="mono"
                  onClick={() => setProject(dir)}
                  style={{
                    fontSize: "calc(12.5px * var(--fs,1))",
                    padding: "5px 9px",
                    borderRadius: 7,
                    border: project === dir ? "1px solid var(--accent)" : "1px solid var(--border)",
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

        {/* Host picker */}
        <div>
          <div style={sectionLabel}>Host</div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {hosts.map((h) => {
              const selected = hostId === h.id;
              return (
                <button
                  key={h.id}
                  data-testid={`nm-host-${h.id}`}
                  onClick={() => setHostId(h.id)}
                  style={{
                    ...pickerBtnStyle(selected),
                    display: "flex",
                    alignItems: "center",
                    gap: 7,
                    flex: "1 1 110px",
                    justifyContent: "center",
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
        </div>

        {/* Resume / continue — #3, mirrors NewSession. Sourced from
            capabilities[(driver,host)].sessions (claude: scoped to the chosen
            project). */}
        <div>
          <div style={sectionLabel}>Resume</div>
          <div
            data-testid="nm-resume-toggle"
            onClick={() => setResume((r) => !r)}
            style={{ display: "flex", alignItems: "center", gap: 9, cursor: "pointer" }}
          >
            <span
              style={{
                width: 34,
                height: 19,
                borderRadius: 10,
                background: resume ? "var(--accent)" : "var(--border2)",
                position: "relative",
                flexShrink: 0,
                transition: "background 0.15s",
              }}
            >
              <span
                style={{
                  position: "absolute",
                  top: 2,
                  left: resume ? 17 : 2,
                  width: 15,
                  height: 15,
                  borderRadius: "50%",
                  background: "#fff",
                  transition: "left 0.15s",
                }}
              />
            </span>
            <span style={{ fontSize: "calc(13.5px * var(--fs,1))", color: "var(--txt2)", fontWeight: 500 }}>
              Continue or resume a previous session
            </span>
          </div>

          {resume && (
            <div
              style={{
                marginTop: 8,
                display: "flex",
                flexDirection: "column",
                gap: 6,
                maxHeight: 200,
                overflowY: "auto",
              }}
            >
              <div
                data-testid="nm-resume-continue"
                onClick={() => setSelectedResume("continue")}
                style={{
                  ...pickerBtnStyle(selectedResume === "continue"),
                  fontWeight: 600,
                }}
              >
                Continue most recent
              </div>
              {visibleSessions.map((s) => {
                const selected = selectedResume === s.id;
                return (
                  <div
                    key={s.id}
                    data-testid={`nm-resume-session-${s.id}`}
                    onClick={() => setSelectedResume(s.id)}
                    style={pickerBtnStyle(selected)}
                  >
                    {sessionLabel(s)}
                  </div>
                );
              })}
              {loadingSessions && (
                <div
                  data-testid="nm-resume-loading"
                  style={{ fontSize: "calc(12.5px * var(--fs,1))", color: "var(--txt3)", fontStyle: "italic" }}
                >
                  {visibleSessions.length === 0 ? "Discovering sessions…" : "Refreshing…"}
                </div>
              )}
              {visibleSessions.length === 0 && !loadingSessions && (
                <div
                  data-testid="nm-resume-hint"
                  style={{ fontSize: "calc(12.5px * var(--fs,1))", color: "var(--txt3)", fontStyle: "italic" }}
                >
                  No previous sessions discovered yet for this project.
                </div>
              )}
            </div>
          )}
        </div>

        {/* Controlled by / Feeds from — only when the workstream already has
            nodes to wire to (nmHasNodes). Single-select toggle each. */}
        {existingNodes.length > 0 && (
          <div style={{ display: "flex", gap: 14 }}>
            <div style={{ flex: 1 }}>
              <div style={sectionLabel}>
                Controlled by{" "}
                <span style={{ opacity: 0.7, textTransform: "none", fontWeight: 500 }}>
                  · optional
                </span>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 5 }}>
                {existingNodes.map((n) => {
                  const selected = controller === n.id;
                  return (
                    <button
                      key={n.id}
                      data-testid={`nm-controller-${n.id}`}
                      onClick={() => setController(selected ? null : n.id)}
                      style={pickerBtnStyle(selected)}
                    >
                      {n.name}
                    </button>
                  );
                })}
              </div>
            </div>
            <div style={{ flex: 1 }}>
              <div style={sectionLabel}>
                Feeds from{" "}
                <span style={{ opacity: 0.7, textTransform: "none", fontWeight: 500 }}>
                  · optional
                </span>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 5 }}>
                {existingNodes.map((n) => {
                  const selected = feeder === n.id;
                  return (
                    <button
                      key={n.id}
                      data-testid={`nm-feeder-${n.id}`}
                      onClick={() => setFeeder(selected ? null : n.id)}
                      style={pickerBtnStyle(selected)}
                    >
                      {n.name}
                    </button>
                  );
                })}
              </div>
            </div>
          </div>
        )}

        {/* Model + Permission posture */}
        <div style={{ display: "flex", gap: 14 }}>
          <div style={{ flex: 1 }}>
            <div style={sectionLabel}>Model</div>
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {modelCatalog.models.map((m) => {
                const selected = model === m.id;
                return (
                  <button
                    key={m.id}
                    data-testid={`nm-model-${m.id}`}
                    onClick={() => selectModel(m.id)}
                    style={pickerBtnStyle(selected)}
                  >
                    {m.id}
                    {m.annotation ? ` — ${m.annotation}` : ""}
                  </button>
                );
              })}
            </div>
            {modelCatalog.models.length === 0 && capsLoading && (
              <div
                data-testid="nm-model-loading"
                style={{ fontSize: "calc(13px * var(--fs,1))", color: "var(--txt3)", fontStyle: "italic" }}
              >
                Discovering models…
              </div>
            )}
            {modelCatalog.models.length === 0 && !capsLoading && (
              <div
                data-testid="nm-model-empty-hint"
                style={{ fontSize: "calc(13px * var(--fs,1))", color: "var(--txt3)", fontStyle: "italic" }}
              >
                No models observed yet for this host/driver — type one below.
              </div>
            )}
            {modelCatalog.free_text && (
              <input
                data-testid="nm-model-freetext"
                value={customModelValue}
                onChange={(e) => selectModel(e.target.value)}
                placeholder="Type your own"
                className="mono"
                autoComplete="off"
                autoCorrect="off"
                spellCheck={false}
                name="nm-model-freetext-nofill"
                style={{ ...textInputStyle, fontFamily: "var(--font-mono, monospace)", marginTop: 6 }}
              />
            )}
          </div>

          <div style={{ flex: 1 }}>
            <div style={sectionLabel}>Permission</div>
            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              {POSTURES.map((p) => {
                const selected = posture === p.value;
                return (
                  <button
                    key={p.value}
                    data-testid={`nm-posture-${p.value}`}
                    onClick={() => setPosture(p.value)}
                    style={pickerBtnStyle(selected)}
                  >
                    {p.label}
                  </button>
                );
              })}
            </div>
          </div>
        </div>

        {/* First prompt */}
        <div>
          <div style={sectionLabel}>
            First prompt{" "}
            <span style={{ opacity: 0.7, textTransform: "none", fontWeight: 500 }}>
              · optional
            </span>
          </div>
          <textarea
            data-testid="nm-first-prompt"
            value={firstPrompt}
            onChange={(e) => setFirstPrompt(e.target.value)}
            placeholder="What should this agent start working on?"
            rows={2}
            style={{ ...textInputStyle, resize: "vertical" }}
          />
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
          className="mono"
          style={{ fontSize: "calc(12px * var(--fs,1))", color: launchError ? "var(--error)" : "var(--txt3)", flex: 1 }}
        >
          {launchError ? (
            `Launch failed: ${launchError}`
          ) : !project.trim() ? (
            <span data-testid="nm-launch-disabled-reason">Enter a project directory</span>
          ) : !driverAvailable(driver) ? (
            <span data-testid="nm-launch-disabled-reason">
              {driverDiscovering(driver)
                ? "Discovering… "
                : `${DRIVERS.find((d) => d.id === driver)?.label} isn't available on this host`}
            </span>
          ) : (
            "Runs over your SSH config · nothing installed inbound"
          )}
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
          data-testid="nm-launch-btn"
          onClick={handleLaunch}
          disabled={busy || !project.trim() || !driverAvailable(driver)}
          style={{
            fontFamily: "inherit",
            fontWeight: 700,
            fontSize: "calc(14px * var(--fs,1))",
            padding: "9px 18px",
            borderRadius: 9,
            border: "none",
            background: "var(--accent)",
            color: "#fff",
            cursor: busy || !project.trim() ? "not-allowed" : "pointer",
            opacity: busy || !project.trim() || !driverAvailable(driver) ? 0.5 : 1,
          }}
        >
          {launchLabel}
        </button>
      </div>
    </Modal>
  );
}
