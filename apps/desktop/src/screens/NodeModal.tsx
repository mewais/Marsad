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
import { launchSession, fetchGraph, capsKey } from "../bridge/api";
import type { CapabilitiesView, ModelCatalog } from "../bridge/types";
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

const POSTURES: { value: "ask" | "auto" | "full"; label: string }[] = [
  { value: "ask", label: "Ask each time" },
  { value: "auto", label: "Auto-approve safe" },
  { value: "full", label: "Full autonomy" },
];

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
  const { state, dispatch, hosts, assignSession, addConnection, goToGraph } = useVm();
  const ctx = state.nodeModalCtx;
  const open = state.screen === "node-modal" && ctx !== null;

  const [name, setName] = useState("");
  const [project, setProject] = useState("");
  const [hostId, setHostId] = useState(
    hosts.find((h) => h.is_local)?.id ?? hosts[0]?.id ?? "local"
  );
  const [driver, setDriver] = useState<Driver>("claude");
  const [model, setModel] = useState("");
  const [userModifiedModel, setUserModifiedModel] = useState(false);
  const [posture, setPosture] = useState<"ask" | "auto" | "full">("auto");
  const [firstPrompt, setFirstPrompt] = useState("");
  const [controller, setController] = useState<string | null>(null);
  const [feeder, setFeeder] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [launchError, setLaunchError] = useState<string | null>(null);
  const [existingNodes, setExistingNodes] = useState<GraphNode[]>([]);

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
  function driverAvailable(d: Driver): boolean {
    return capsForDriver(d)?.cli_available === true;
  }

  const modelCatalog: ModelCatalog = capsForDriver(driver)?.models ?? EMPTY_MODEL_CATALOG;

  useEffect(() => {
    if (userModifiedModel) return;
    if (modelCatalog.models.length > 0) {
      setModel(modelCatalog.models[0].id);
    } else {
      setModel("");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modelCatalog.models, userModifiedModel]);

  function selectModel(id: string) {
    setUserModifiedModel(true);
    setModel(id);
  }

  function selectDriver(d: Driver) {
    if (!driverAvailable(d)) return;
    setDriver(d);
    setUserModifiedModel(false);
  }

  function close() {
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
    try {
      const newId = await launchSession({
        project: project.trim(),
        host_id: hostId,
        model,
        posture,
        first_prompt: firstPrompt.trim() || undefined,
        name: sessionName,
      });
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
        dispatch({ type: "SELECT_SESSION", id: newId, fromGraph: false, wsId: ctx.wsId });
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
                  title={!available ? "Not yet launchable" : undefined}
                  style={pickerBtnStyle(selected, !available)}
                >
                  {d.label}
                </button>
              );
            })}
          </div>
          {!driverAvailable(driver) && (
            <div
              data-testid="nm-driver-unavailable-note"
              style={{ fontSize: "calc(12px * var(--fs,1))", color: "var(--txt3)", marginTop: 6, fontStyle: "italic" }}
            >
              {DRIVERS.find((d) => d.id === driver)?.label} isn't launchable on this host yet — not
              yet launchable.
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
            {modelCatalog.models.length === 0 && (
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
                value={model}
                onChange={(e) => selectModel(e.target.value)}
                placeholder="Custom model id"
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
          {launchError ? `Launch failed: ${launchError}` : "Runs over your SSH config · nothing installed inbound"}
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
