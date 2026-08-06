/**
 * Config tab — model selector buttons, live session modes,
 * tools/connectors chips, Apply / Save-as-template actions.
 */
import { useEffect, useMemo, useState } from "react";
import { useVm } from "../../bridge/useVm";
import {
  restartSession,
  saveTemplate,
  listTemplates,
  capsForDriverHost,
  setSessionConfigOption,
} from "../../bridge/api";
import type { PersistedTemplate } from "../../bridge/types";

// ---------------------------------------------------------------------------
// Static options
// ---------------------------------------------------------------------------

// Status → color token for MCP server chips.
function mcpStatusColor(status: string): string {
  switch (status) {
    case "connected": return "var(--success, #22c55e)";
    case "failed":    return "var(--error, #ef4444)";
    case "pending":   return "var(--warning, #f59e0b)";
    default:          return "var(--txt3)";
  }
}

// ---------------------------------------------------------------------------
// ConfigTab
// ---------------------------------------------------------------------------

export function ConfigTab() {
  const { selectedSession, state } = useVm();

  // Model options come ONLY from capabilities[(driver, host)].models for THIS
  // session's driver+host (dynamic-capabilities.md §5) — never a static import.
  // Must key on the session's driver: a host can expose several drivers, so a
  // host-only lookup would list e.g. codex/gpt models for a claude session.
  const capsForHost = useMemo(
    () =>
      selectedSession
        ? capsForDriverHost(
            state.capabilities,
            selectedSession.driver ?? "claude",
            selectedSession.host_id,
          )
        : undefined,
    [state.capabilities, selectedSession]
  );
  const modelOptions = capsForHost?.models.models ?? [];
  const modelFreeText = capsForHost?.models.free_text ?? true;

  // Local state mirrors current session values; Apply restarts the session.
  // Initial values come from the selected session so the tab reflects the
  // session's REAL model rather than a hardcoded default.
  const initialModel = selectedSession?.model ?? "";
  const [selectedModel, setSelectedModel] = useState(initialModel);
  // Pending session mode for drivers that change mode via RESTART (claude/codex —
  // they have no reliable live mode switch; opencode does, handled separately).
  // Applied by "Apply" alongside the model. Defaults to the running mode.
  const [pendingMode, setPendingMode] = useState<string>(selectedSession?.current_mode ?? "");
  // The currently RUNNING model for this session (from system/init, never
  // overridden by the UI's "next restart" picker state).
  const runningModel = selectedSession?.model ?? "";
  // #1 model free-text — the custom input is ONLY for a model NOT in the
  // catalog. Empty when the selected model matches a catalog entry (a picked
  // radio); shows the typed id only when it's a custom one. Prevents picking a
  // catalog model from filling the custom box.
  const customModelValue = modelOptions.some((m) => m.id === selectedModel) ? "" : selectedModel;

  // Tool toggle state: keyed by tool name. Initially all tools are enabled
  // (empty disallowed set). When the session provides a real tools list it is
  // used; otherwise the set is empty (no tools to show).
  const sessionTools = selectedSession?.tools ?? [];
  const [disabledTools, setDisabledTools] = useState<Set<string>>(new Set());

  // Re-sync model whenever the selected session changes so each
  // session shows its own real values, not values left over from the previous one.
  useEffect(() => {
    setSelectedModel(selectedSession?.model ?? "");
    setPendingMode(selectedSession?.current_mode ?? "");
    setDisabledTools(new Set()); // reset tool overrides for the new session
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedSession?.id]);

  // Keep the pending mode in sync when the RUNNING mode changes underneath us
  // (e.g. after an Apply-restart lands the new mode via system/init), so the
  // highlighted radio tracks reality once applied.
  useEffect(() => {
    setPendingMode(selectedSession?.current_mode ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedSession?.current_mode]);

  // Apply / Save-template busy + error state.
  const [applyBusy, setApplyBusy] = useState(false);
  const [applyError, setApplyError] = useState<string | null>(null);
  const [saveBusy, setSaveBusy] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // Persisted templates loaded on mount.
  const [templates, setTemplates] = useState<PersistedTemplate[]>([]);

  useEffect(() => {
    listTemplates().then(setTemplates).catch(() => {});
  }, []);

  function toggleTool(tool: string) {
    setDisabledTools((prev) => {
      const next = new Set(prev);
      if (next.has(tool)) next.delete(tool);
      else next.add(tool);
      return next;
    });
  }

  async function handleApply() {
    if (!selectedSession) return;
    setApplyBusy(true);
    setApplyError(null);
    try {
      // opencode switches mode LIVE (no restart), so a restart preserves its
      // running mode. claude/codex have no reliable live switch → the picked
      // `pendingMode` is applied here, on restart (launch honors --permission-mode
      // / codex mode-settings).
      const driver = selectedSession.driver ?? "claude";
      const modeForRestart =
        driver === "opencode" ? (selectedSession.current_mode ?? "") : pendingMode;
      await restartSession({
        id: selectedSession.id,
        model: selectedModel,
        session_mode: modeForRestart,
        disallowed_tools: Array.from(disabledTools),
      });
    } catch (e) {
      setApplyError(String(e));
    } finally {
      setApplyBusy(false);
    }
  }

  async function handleSaveTemplate() {
    const name = window.prompt("Template name:");
    if (!name || !name.trim()) return;
    setSaveBusy(true);
    setSaveError(null);
    try {
      await saveTemplate({ name: name.trim(), model: selectedModel, mode: selectedSession?.current_mode ?? "" });
      const updated = await listTemplates();
      setTemplates(updated);
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaveBusy(false);
    }
  }

  function applyTemplate(t: PersistedTemplate) {
    setSelectedModel(t.model);
  }

  return (
    <div style={{ flex: 1, overflowY: "auto", padding: 24 }}>
      <div
        style={{
          maxWidth: 900,
          margin: "0 auto",
          display: "flex",
          flexDirection: "column",
          gap: 20,
        }}
      >
        {/* Model */}
        <div>
          <div
            style={{
              fontSize: "calc(14.5px * var(--fs,1))",
              fontWeight: 700,
              marginBottom: 9,
              color: "var(--txt2)",
            }}
          >
            Model
          </div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {modelOptions.map((mo) => {
              const active = selectedModel === mo.id;
              // "current" = the model actually running in this session, which
              // may differ from the picker's "next restart" selection.
              const current = runningModel === mo.id && !active;
              return (
                <button
                  key={mo.id}
                  data-testid={`model-btn-${mo.id}`}
                  onClick={() => setSelectedModel(mo.id)}
                  title={current ? `Currently running: ${mo.id}` : mo.id}
                  style={{
                    fontFamily: "inherit",
                    fontSize: "calc(14.5px * var(--fs,1))",
                    fontWeight: 600,
                    padding: "9px 14px",
                    borderRadius: 9,
                    cursor: "pointer",
                    border: active
                      ? "1px solid var(--accent)"
                      : current
                        ? "1px solid var(--done)"
                        : "1px solid var(--border2)",
                    background: active
                      ? "var(--accent-bg)"
                      : current
                        ? "var(--done-bg)"
                        : "var(--panel2)",
                    color: active
                      ? "var(--accent2)"
                      : current
                        ? "var(--done)"
                        : "var(--txt2)",
                  }}
                >
                  {mo.id}
                  {mo.annotation ? ` — ${mo.annotation}` : ""}
                  {current && (
                    <span
                      style={{
                        marginLeft: 6,
                        fontSize: "calc(10.5px * var(--fs,1))",
                        fontWeight: 700,
                        opacity: 0.8,
                      }}
                    >
                      ● live
                    </span>
                  )}
                </button>
              );
            })}
          </div>
          {modelOptions.length === 0 && (
            <div style={{ fontSize: "calc(13.5px * var(--fs,1))", color: "var(--txt3)", fontStyle: "italic" }}>
              No models observed yet for this host — type one below.
            </div>
          )}
          {modelFreeText && (
            <input
              data-testid="config-model-freetext"
              value={customModelValue}
              onChange={(e) => setSelectedModel(e.target.value)}
              placeholder="Type your own"
              className="mono"
              autoComplete="off"
              autoCorrect="off"
              spellCheck={false}
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
                marginTop: 8,
              }}
            />
          )}
        </div>

        {/* Session mode — AGENT-REPORTED (never hardcoded). opencode's modes come
            from configOptions; claude's from its version. Only opencode has a
            reliable LIVE switch (session/set_config_option, agent-confirmed on the
            stream). claude/codex do NOT reliably switch live — their mode is applied
            by RESTART (launch honors --permission-mode / codex mode-settings), so a
            picked mode is pending until "Apply". */}
        {selectedSession && (selectedSession.available_modes?.length ?? 0) > 0 && (() => {
          const modes = selectedSession.available_modes ?? [];
          const current = selectedSession.current_mode;
          const driver = selectedSession.driver ?? "claude";
          // Only opencode switches live in place. claude/codex apply on restart.
          const liveSwitch = driver === "opencode";
          // The mode shown as selected: the LIVE mode for opencode, the PENDING
          // (to-be-applied) pick for claude/codex.
          const selectedMode = liveSwitch ? current : pendingMode;
          const modeDirty = !liveSwitch && pendingMode !== (current ?? "");
          return (
            <div>
              <div
                style={{
                  fontSize: "calc(14.5px * var(--fs,1))",
                  fontWeight: 700,
                  marginBottom: 9,
                  color: "var(--txt2)",
                }}
              >
                Session mode
                {!liveSwitch && (
                  <span
                    style={{
                      marginLeft: 8,
                      fontSize: "calc(12px * var(--fs,1))",
                      fontWeight: 500,
                      color: "var(--txt3)",
                    }}
                  >
                    {modeDirty ? "(applies on Apply — restarts the session)" : "(applied on restart)"}
                  </span>
                )}
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 7 }}>
                {modes.map((m) => {
                  const active = selectedMode === m.id;
                  // claude/codex rows are always clickable (they set the pending
                  // pick); opencode rows are clickable only when not already active.
                  const clickable = liveSwitch ? !active : true;
                  return (
                    <div
                      key={m.id}
                      data-testid={`mode-${m.id}`}
                      data-active={active ? "true" : "false"}
                      onClick={
                        clickable
                          ? () => {
                              if (liveSwitch) {
                                setSessionConfigOption(selectedSession.id, "mode", m.id);
                              } else {
                                // claude/codex: stage the pick; Apply restarts with it.
                                setPendingMode(m.id);
                              }
                            }
                          : undefined
                      }
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 11,
                        padding: "12px 14px",
                        borderRadius: 10,
                        cursor: clickable ? "pointer" : "default",
                        border: active
                          ? "1px solid var(--accent)"
                          : "1px solid var(--border)",
                        background: active ? "var(--accent-bg)" : "var(--panel2)",
                        opacity: clickable || active ? 1 : 0.85,
                      }}
                    >
                      <span
                        style={{
                          width: 16,
                          height: 16,
                          borderRadius: "50%",
                          border: `2px solid ${active ? "var(--accent)" : "var(--border2)"}`,
                          flexShrink: 0,
                          display: "flex",
                          alignItems: "center",
                          justifyContent: "center",
                        }}
                      >
                        <span
                          style={{
                            width: 7,
                            height: 7,
                            borderRadius: "50%",
                            background: active ? "var(--accent)" : "transparent",
                          }}
                        />
                      </span>
                      <div>
                        <div style={{ fontWeight: 600, fontSize: "calc(15px * var(--fs,1))" }}>
                          {m.label || m.id}
                          {active && (
                            <span
                              style={{
                                marginLeft: 8,
                                fontSize: "calc(10.5px * var(--fs,1))",
                                fontWeight: 700,
                                color: "var(--accent2)",
                              }}
                            >
                              ● current
                            </span>
                          )}
                        </div>
                        {m.description && (
                          <div style={{ fontSize: "calc(13.5px * var(--fs,1))", color: "var(--txt3)" }}>
                            {m.description}
                          </div>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          );
        })()}

        {/* Tools (real list from system/init — toggleable; unchecked = disallowed) */}
        {sessionTools.length > 0 && (
          <div>
            <div
              style={{
                fontSize: "calc(14.5px * var(--fs,1))",
                fontWeight: 700,
                marginBottom: 9,
                color: "var(--txt2)",
              }}
            >
              Tools
            </div>
            <div style={{ display: "flex", gap: 7, flexWrap: "wrap" }}>
              {sessionTools.map((tool) => {
                const enabled = !disabledTools.has(tool);
                return (
                  <span
                    key={tool}
                    data-testid={`tool-chip-${tool}`}
                    onClick={() => toggleTool(tool)}
                    style={{
                      fontFamily: "inherit",
                      fontSize: "calc(14px * var(--fs,1))",
                      fontWeight: 600,
                      padding: "6px 11px",
                      borderRadius: 7,
                      cursor: "pointer",
                      border: enabled
                        ? "1px solid var(--accent)"
                        : "1px solid var(--border)",
                      background: enabled ? "var(--accent-bg)" : "var(--panel2)",
                      color: enabled ? "var(--accent2)" : "var(--txt3)",
                      userSelect: "none" as const,
                    }}
                  >
                    {enabled ? "● " : "○ "}
                    {tool}
                  </span>
                );
              })}
            </div>
          </div>
        )}

        {/* MCP connectors (display-only — status colored by connection state) */}
        {selectedSession && selectedSession.mcp_servers.length > 0 && (
          <div>
            <div
              style={{
                fontSize: "calc(14.5px * var(--fs,1))",
                fontWeight: 700,
                marginBottom: 9,
                color: "var(--txt2)",
              }}
            >
              MCP connectors
            </div>
            <div style={{ display: "flex", gap: 7, flexWrap: "wrap" }}>
              {selectedSession.mcp_servers.map((srv) => (
                <span
                  key={srv.name}
                  data-testid={`mcp-chip-${srv.name}`}
                  title={`Status: ${srv.status || "unknown"}`}
                  style={{
                    fontFamily: "inherit",
                    fontSize: "calc(14px * var(--fs,1))",
                    fontWeight: 600,
                    padding: "6px 11px",
                    borderRadius: 7,
                    border: "1px solid var(--border)",
                    background: "var(--panel2)",
                    color: "var(--txt2)",
                    userSelect: "none" as const,
                    cursor: "default",
                  }}
                >
                  <span
                    style={{
                      display: "inline-block",
                      width: 8,
                      height: 8,
                      borderRadius: "50%",
                      background: mcpStatusColor(srv.status),
                      marginRight: 6,
                      verticalAlign: "middle",
                    }}
                  />
                  {srv.name}
                </span>
              ))}
            </div>
          </div>
        )}

        {/* Saved templates — quick-apply chips */}
        {templates.length > 0 && (
          <div>
            <div
              style={{
                fontSize: "calc(14.5px * var(--fs,1))",
                fontWeight: 700,
                marginBottom: 9,
                color: "var(--txt2)",
              }}
            >
              Saved templates
            </div>
            <div style={{ display: "flex", gap: 7, flexWrap: "wrap" }}>
              {templates.map((t) => (
                <span
                  key={t.name}
                  data-testid={`template-chip-${t.name}`}
                  onClick={() => applyTemplate(t)}
                  style={{
                    fontFamily: "inherit",
                    fontSize: "calc(14px * var(--fs,1))",
                    fontWeight: 600,
                    padding: "6px 11px",
                    borderRadius: 7,
                    cursor: "pointer",
                    border: "1px solid var(--border)",
                    background: "var(--panel2)",
                    color: "var(--txt2)",
                    userSelect: "none" as const,
                  }}
                  title={`${t.model}${t.mode ? ` · ${t.mode}` : ""}`}
                >
                  {t.name}
                </span>
              ))}
            </div>
          </div>
        )}

        {/* Action buttons */}
        <div style={{ display: "flex", flexDirection: "column", gap: 8, paddingTop: 4 }}>
          <div style={{ display: "flex", gap: 9 }}>
            <button
              data-testid="config-apply"
              onClick={handleApply}
              disabled={applyBusy || !selectedSession}
              style={{
                fontFamily: "inherit",
                fontWeight: 700,
                fontSize: "calc(15px * var(--fs,1))",
                padding: "9px 16px",
                borderRadius: 9,
                border: "none",
                background: applyBusy || !selectedSession ? "var(--border2)" : "var(--accent)",
                color: applyBusy || !selectedSession ? "var(--txt3)" : "#fff",
                cursor: applyBusy || !selectedSession ? "not-allowed" : "pointer",
              }}
            >
              {applyBusy ? "Restarting…" : "Apply to running session"}
            </button>
            <button
              data-testid="config-save-template"
              onClick={handleSaveTemplate}
              disabled={saveBusy}
              style={{
                fontFamily: "inherit",
                fontWeight: 600,
                fontSize: "calc(15px * var(--fs,1))",
                padding: "9px 16px",
                borderRadius: 9,
                border: "1px solid var(--border2)",
                background: "var(--panel2)",
                color: saveBusy ? "var(--txt3)" : "var(--txt2)",
                cursor: saveBusy ? "not-allowed" : "pointer",
              }}
            >
              {saveBusy ? "Saving…" : "Save as template…"}
            </button>
          </div>
          {applyError && (
            <div
              data-testid="config-apply-error"
              style={{ fontSize: "calc(13px * var(--fs,1))", color: "var(--error, #e53)", paddingLeft: 2 }}
            >
              {applyError}
            </div>
          )}
          {saveError && (
            <div
              data-testid="config-save-error"
              style={{ fontSize: "calc(13px * var(--fs,1))", color: "var(--error, #e53)", paddingLeft: 2 }}
            >
              {saveError}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
