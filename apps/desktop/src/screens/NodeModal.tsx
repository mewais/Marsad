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
import { launchSession, launchWaslSession, fetchGraph, capsKey, getSettings, refreshCapabilities } from "../bridge/api";
import type { CapabilitiesView, DiscoveredSession, ModelCatalog, SessionMode } from "../bridge/types";
import type { Driver, GraphNode } from "../bridge/workstreamTypes";
import { DRIVERS, DRIVER_COLOR, isWaslDriver, waslProtocol } from "../bridge/drivers";
import { buildWaslTargetKey } from "../bridge/waslTargetKey";
import { Modal } from "../components/Modal";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const EMPTY_MODEL_CATALOG: ModelCatalog = { models: [], source: "Observed", free_text: true };

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

/** "as of …" / "not fetched yet" staleness label. `fetched_at_ms: 0` is the
 *  "never successfully probed" sentinel — never rendered as a 1970 date. */
function formatStaleness(fetchedAtMs: number): string {
  if (!fetchedAtMs) return "Not fetched yet";
  return `as of ${relativeTime(fetchedAtMs)}`;
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
    addDraftNode,
    addDraftEdge,
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
  const [sessionMode, setSessionMode] = useState("");
  const [userModifiedSessionMode, setUserModifiedSessionMode] = useState(false);
  // #2 pre-pick — Settings-persisted defaults (default_driver / default_models
  // per driver / default_modes per driver) resolved on mount. Model defaults are keyed
  // by driver so switching driver picks that driver's default.
  const [defaultModels, setDefaultModels] = useState<Record<string, string>>({});
  const [defaultModes, setDefaultModes] = useState<Record<string, string>>({});
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
  // per-driver default model, per-driver default mode). Absent settings degrade
  // to the capabilities-first-model / backend-default fallbacks.
  useEffect(() => {
    getSettings()
      .then((s) => {
        const dm: Record<string, string> = {};
        for (const [d, m] of s.default_models ?? []) dm[d] = m;
        // default_model is the legacy claude default — honor it if no explicit
        // per-driver claude entry exists.
        if (dm.claude === undefined && s.default_model) dm.claude = s.default_model;
        setDefaultModels(dm);
        const dmodes: Record<string, string> = {};
        for (const [d, mode] of s.default_modes ?? []) dmodes[d] = mode;
        setDefaultModes(dmodes);
        if (!userModifiedDriver && s.default_driver) {
          setDriver(s.default_driver as Driver);
        }
      })
      .catch(() => {/* keep fallbacks on error */});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Capability discovery (agents + versioned models). Re-probe when we have NO
  // data yet, OR when the cached result says this LAUNCHABLE driver's CLI isn't
  // available — a negative verdict is exactly what goes stale after the user
  // installs the CLI, and nothing else re-probes a remote host, so an old
  // "not available" would otherwise stick forever (the WSL-opencode bug). A
  // POSITIVE result (CLI found) stays cached so we don't redo the whole slow
  // remote probe (etx ~47s) or race the path-scoped session probe on every open.
  // Non-launchable drivers (e.g. gemini) are never re-probed — they can't launch
  // regardless. NO project here (this is "what agents/models exist").
  // Wasl drivers alias to their underlying protocol for the probe (capabilities
  // are host+CLI facts, not Wasl-transport facts).
  useEffect(() => {
    if (!open) return;
    const caps = state.capabilities[capsKey(capDriver, hostId)];
    const staleNegative =
      caps !== undefined && caps.launchable && caps.cli_available !== true;
    if (caps !== undefined && !staleNegative) return;
    refreshCapabilities({ host_id: hostId, driver: capDriver });
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

  // Capability aliasing: wasl drivers are launched via Wasl transport, but their
  // underlying CLI facts (models, available_modes, sessions) are the SAME as
  // the non-wasl protocol on that host. "wasl-claude" → reads from "claude"
  // slot, "wasl-opencode" → "opencode", "wasl-codex" → "codex".
  // For non-wasl drivers `capDriver === driver` (identity). Only capability
  // READS + the discovery TRIGGER alias; the launch path keeps the real driver.
  const capDriver: Driver = isWaslDriver(driver)
    ? ((waslProtocol(driver) as Driver) ?? driver)
    : driver;

  // Capabilities-driven driver availability + model catalog — NEVER a
  // hardcoded list (dynamic-capabilities.md §5). A driver with no reported
  // capabilities (or cli_available: false) is disabled with a note; today
  // that's codex/gemini until the launch backend grows a driver parameter.
  function capsForDriver(d: Driver): CapabilitiesView | undefined {
    return state.capabilities[capsKey(d, hostId)];
  }
  /** True while REAL discovery for this (driver, host) hasn't landed yet — caps
   *  are absent. We stay optimistic during this window (never grey), and show a
   *  loading indicator instead of the empty state. Wasl drivers alias to their
   *  underlying protocol for the lookup. */
  function driverDiscovering(d: Driver): boolean {
    const aliased: Driver = isWaslDriver(d) ? ((waslProtocol(d) as Driver) ?? d) : d;
    return capsForDriver(aliased) === undefined;
  }
  // Driver availability from REAL per-host discovery (dynamic-capabilities.md
  // §5). While caps are absent (discovery in flight) stay optimistic — never
  // grey a loading driver. Once caps are present the truth is authoritative:
  // available iff Marsad has a launch driver (`launchable`) AND the CLI was
  // actually detected on the host (`cli_available`). gemini → launchable:false
  // → greyed; a driver not installed on the host → cli_available:false → greyed.
  //
  // Wasl drivers alias to their underlying protocol for the caps lookup, but a
  // managed `wasl-*` driver requires TWO things on the host, ANDed together:
  // (1) its native CLI is present (the aliased `cli_available`), AND (2) wasl
  // itself (`wasl-mcpd`) is installed (`wasl_available`, host-global on every
  // driver's caps). Missing either → greyed.
  function driverAvailable(d: Driver): boolean {
    const aliased: Driver = isWaslDriver(d) ? ((waslProtocol(d) as Driver) ?? d) : d;
    const c = capsForDriver(aliased);
    if (!c) return true; // discovery in flight — optimistic
    if (!c.launchable) return false; // no Marsad driver (e.g. gemini) — hard no
    // A probe that ERRORED/TIMED OUT (slow-setup remote hosts) says nothing about
    // whether the CLI is installed. Don't block a launchable driver on a flaky
    // probe; only grey out when the probe SUCCEEDED (no errors) and cli was absent.
    if (c.errors.length > 0) return true;
    const nativeAvail = c.cli_available === true;
    // wasl-* needs BOTH the native mirror AND wasl-mcpd on the host.
    if (isWaslDriver(d)) return nativeAvail && c.wasl_available === true;
    return nativeAvail;
  }
  /** Known-unavailable: caps are PRESENT and report the driver as not
   *  launchable (gemini) or its CLI not detected. Absent caps (loading) are
   *  never known-unavailable. */
  function driverKnownUnavailable(d: Driver): boolean {
    const c = capsForDriver(isWaslDriver(d) ? ((waslProtocol(d) as Driver) ?? d) : d);
    return !!c && !driverAvailable(d);
  }

  const modelCatalog: ModelCatalog = capsForDriver(capDriver)?.models ?? EMPTY_MODEL_CATALOG;

  // #1 model free-text — the custom input is ONLY for a model NOT in the
  // catalog. When `model` matches a catalog entry (a picked family/radio), the
  // box is empty; it shows the typed id only when the user is entering a custom
  // one. Prevents picking `opus` from filling the box with "opus".
  const customModelValue = modelCatalog.models.some((m) => m.id === model) ? "" : model;

  // Session-mode list from capabilities for the selected (driver, host).
  // Empty list = backend default (no mode picker rendered).
  // Uses capDriver alias: wasl-* reads from the underlying protocol slot.
  const modeList: SessionMode[] = capsForDriver(capDriver)?.available_modes ?? [];

  // #3 resume — sessions come ONLY from capabilities[(capDriver, host)].sessions
  // (populated by scan_claude_sessions once a project is in scope). String-
  // project entries are filtered to the chosen project; agy-style null-project
  // entries always show (they aren't scoped to a requested project).
  // capDriver alias: wasl-* reads sessions from the underlying protocol slot.
  const projectTrim = project.trim();
  const driverCaps = capsForDriver(capDriver);
  const allSessions: DiscoveredSession[] = driverCaps?.sessions ?? [];
  const visibleSessions = allSessions.filter(
    (s) => s.project == null || s.project === projectTrim
  );
  // agy-style `project: null` entries aren't scoped to a requested project —
  // they always render under "unknown project". String-project entries are
  // filtered to the chosen project.
  const unknownProjectSessions = visibleSessions.filter((s) => s.project == null);
  const knownProjectSessions = visibleSessions.filter((s) => s.project != null);
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
  // capDriver alias: wasl-* probes the underlying protocol's session store.
  useEffect(() => {
    if (!resume || !projectTrim) return;
    // DEBOUNCE: `projectTrim` changes on every keystroke of the path. Without a
    // debounce this fired a full scoped probe PER CHARACTER — on a slow remote
    // host (e.g. an HPC login shell ~7s/probe) that floods with overlapping
    // probes that race and time out, and a timed-out probe used to wipe the model
    // + session lists. Fire once, ~500ms after typing settles, so the settled
    // path gets a single clean probe.
    const t = setTimeout(() => {
      refreshCapabilities({ host_id: hostId, driver: capDriver, project: projectTrim });
    }, 500);
    return () => clearTimeout(t);
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

  // Default session mode: Settings default_modes[driver] if present in modeList,
  // else modeList[0]?.id, else "" (backend default). Mirror of model default logic.
  useEffect(() => {
    if (userModifiedSessionMode) return;
    const pref = defaultModes[driver];
    if (pref && modeList.some((m) => m.id === pref)) {
      setSessionMode(pref);
    } else if (modeList.length > 0) {
      setSessionMode(modeList[0].id);
    } else {
      setSessionMode("");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modeList, userModifiedSessionMode, defaultModes, driver]);

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

  function handleAddToGraph() {
    // Design-mode ("Build it out") add — a PURE CLIENT-SIDE draft add. NOTHING
    // may hit the real `marsad-core` here: no launchSession / assignSession /
    // addConnection / createWorkstream (DESIGN_HANDOFF.md:107,160,183). A draft
    // node has NO session id — a real id only exists post-launch (core.md:29),
    // and assign_session/add_connection require already-real ids
    // (core-ext.md:69-72,82-85). We only append the authored config to the
    // store's draft list; any explicitly-picked Controlled-by / Feeds-from wire
    // to an EXISTING DRAFT node is recorded as a draft edge (temp ids). Actual
    // materialization is the separate "Confirm & launch" action on the graph.
    if (!ctx || busy) return;
    if (!project.trim()) return;
    if (!driverAvailable(driver)) return;

    const typedName = name.trim();
    if (!ctx.isFirstNode && !typedName) {
      setLaunchError("Agent name is required");
      return;
    }

    setLaunchError(null);
    // This add authored a node into the workstream — it is no longer an empty
    // draft, so cancelling afterwards must NOT discard the workstream (BUG #2).
    launchedRef.current = true;
    markDraftWorkstream(null);

    const tempId = addDraftNode({
      name: typedName || ctx.wsName || "Agent",
      project: project.trim(),
      hostId,
      driver,
      model,
      session_mode: sessionMode,
      firstPrompt: firstPrompt.trim() || undefined,
      // W2b: carry resume state for wasl drivers so onConfirmDraft can thread it.
      resume: isWaslDriver(driver) ? resume : undefined,
      selectedResume: isWaslDriver(driver) ? selectedResume : undefined,
      resumeSessions: isWaslDriver(driver) ? visibleSessions.map((s) => ({ id: s.id })) : undefined,
    });
    // Honor explicit Controlled-by / Feeds-from picks against existing DRAFT
    // nodes — recorded as draft edges (temp ids), never a real addConnection.
    if (controller) {
      addDraftEdge({ from: controller, to: tempId, bi: false });
    }
    if (feeder) {
      addDraftEdge({ from: feeder, to: tempId, bi: false });
    }
    // Back to the design canvas (the graph screen) so more nodes/connections
    // can be authored — never a live single session, never the board.
    dispatch({ type: "SET_SCREEN", screen: "graph" });
  }

  async function handleLaunch() {
    if (!ctx || busy) return;
    // Design-mode ("Build it out") add routes to the graph, never a launch.
    if (ctx.designMode) {
      handleAddToGraph();
      return;
    }
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
    try {
      let newId: string;
      if (isWaslDriver(driver)) {
        // Generate a stable id for this worker. The NodeModal single-launch
        // path does NOT have workstream connections in scope, so peers is
        // empty — the agent tolerates this. If connections are ever wired
        // here, compute peers via peersForNode(nodeId, connections).
        // TODO(phase E): single-launch peers when connections are in scope.
        const nodeId = typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
          ? `node-${crypto.randomUUID()}`
          : `node-${Date.now()}`;
        const protocol = waslProtocol(driver) ?? driver;
        const wasl_target_key = buildWaslTargetKey(
          nodeId,
          protocol,
          project.trim(),
          model,
          sessionMode,
          [],
          resume,
          selectedResume,
          visibleSessions
        );
        newId = await launchWaslSession({
          host_id: hostId,
          wasl_target_key,
          name: sessionName,
        });
      } else {
        const resumeArgs: { resume_session_id?: string; continue_recent?: boolean } = {};
        if (resume) {
          if (selectedResume === "continue") {
            resumeArgs.continue_recent = true;
          } else {
            resumeArgs.resume_session_id = selectedResume;
          }
        }
        newId = await launchSession({
          project: project.trim(),
          host_id: hostId,
          model,
          session_mode: sessionMode,
          first_prompt: firstPrompt.trim() || undefined,
          name: sessionName,
          driver,
          ...resumeArgs,
        });
      }
      // A session now exists for this workstream — it is no longer an empty
      // draft, so cancelling afterwards must NOT discard it (BUG #2).
      launchedRef.current = true;
      markDraftWorkstream(null);
      // First agent added = effective lead (isFirstNode is exactly that).
      await assignSession(newId, ctx.wsId, ctx.isFirstNode);
      if (controller) {
        await addConnection(`conn-${newId}-ctrl`, ctx.wsId, controller, newId, false);
      }
      if (feeder) {
        await addConnection(`conn-${newId}-feed`, ctx.wsId, feeder, newId, false);
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
            project). Hidden for wasl drivers (wasl sessions don't resume). */}
        {!isWaslDriver(driver) && <div>
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
            {resume && (
              <span
                data-testid="nm-capabilities-staleness"
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
              {knownProjectSessions.map((s) => {
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
              {unknownProjectSessions.length > 0 && (
                <>
                  <div
                    data-testid="nm-resume-unknown-project"
                    style={{
                      marginTop: 4,
                      fontSize: "calc(11.5px * var(--fs,1))",
                      fontWeight: 700,
                      textTransform: "uppercase",
                      letterSpacing: 0.5,
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
                        data-testid={`nm-resume-session-${s.id}`}
                        onClick={() => setSelectedResume(s.id)}
                        style={pickerBtnStyle(selected)}
                      >
                        {sessionLabel(s)}
                      </div>
                    );
                  })}
                </>
              )}
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
        </div>}

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

        {/* Model + Session mode */}
        <div style={{ display: "flex", gap: 14 }}>
          <div style={{ flex: 1 }}>
            <div style={sectionLabel}>Model</div>
            {modelCatalog.source === "Observed" && modelCatalog.models.length > 0 && (
              <div
                data-testid="nm-model-unverified-badge"
                style={{
                  fontSize: "calc(11px * var(--fs,1))",
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

          {modeList.length > 0 && (
            <div style={{ flex: 1 }}>
              <div style={sectionLabel}>Mode</div>
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {modeList.map((m) => {
                  const selected = sessionMode === m.id;
                  return (
                    <button
                      key={m.id}
                      data-testid={`nm-mode-${m.id}`}
                      onClick={() => { setUserModifiedSessionMode(true); setSessionMode(m.id); }}
                      style={pickerBtnStyle(selected)}
                      title={m.description}
                    >
                      <div style={{ fontWeight: 600 }}>{m.label || m.id}</div>
                      {m.description && (
                        <div style={{ fontSize: "calc(12px * var(--fs,1))", color: selected ? "var(--accent2)" : "var(--txt3)", fontWeight: 400 }}>
                          {m.description}
                        </div>
                      )}
                    </button>
                  );
                })}
              </div>
            </div>
          )}
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
