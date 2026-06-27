/**
 * Config tab — model selector buttons, permission posture radios,
 * tools/connectors chips, Apply / Save-as-template actions.
 */
import { useEffect, useState } from "react";
import { useVm } from "../../bridge/useVm";
import { MODELS as MODEL_OPTIONS, DEFAULT_MODEL } from "../../bridge/models";
import { restartSession, saveTemplate, listTemplates } from "../../bridge/api";
import type { PersistedTemplate } from "../../bridge/types";

// ---------------------------------------------------------------------------
// Static options
// ---------------------------------------------------------------------------

type PostureId = "ask" | "auto" | "full";

const POSTURE_OPTIONS: {
  id: PostureId;
  label: string;
  desc: string;
}[] = [
  {
    id: "ask",
    label: "Ask every time",
    desc: "Pause for approval before any action with side effects.",
  },
  {
    id: "auto",
    label: "Auto-approve safe actions",
    desc: "Reads and edits run freely; commands & writes still ask.",
  },
  {
    id: "full",
    label: "Full autonomy",
    desc: "Act without asking. Use only in throwaway sandboxes.",
  },
];

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
  const { selectedSession } = useVm();

  // Local state mirrors current session values; Apply restarts the session.
  const initialModel = selectedSession?.model ?? DEFAULT_MODEL;
  const [selectedModel, setSelectedModel] = useState(initialModel);
  const [posture, setPosture] = useState<PostureId>("ask");

  // Tool toggle state: keyed by tool name. Initially all tools are enabled
  // (empty disallowed set). When the session provides a real tools list it is
  // used; otherwise the set is empty (no tools to show).
  const sessionTools = selectedSession?.tools ?? [];
  const [disabledTools, setDisabledTools] = useState<Set<string>>(new Set());

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
      await restartSession({
        id: selectedSession.id,
        model: selectedModel,
        posture,
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
      await saveTemplate({ name: name.trim(), model: selectedModel, posture });
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
    setPosture(t.posture as PostureId);
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
          <div style={{ display: "flex", gap: 8 }}>
            {MODEL_OPTIONS.map((mo) => {
              const active = selectedModel === mo.id;
              return (
                <button
                  key={mo.id}
                  data-testid={`model-btn-${mo.id}`}
                  onClick={() => setSelectedModel(mo.id)}
                  style={{
                    fontFamily: "inherit",
                    fontSize: "calc(14.5px * var(--fs,1))",
                    fontWeight: 600,
                    padding: "9px 14px",
                    borderRadius: 9,
                    cursor: "pointer",
                    border: active
                      ? "1px solid var(--accent)"
                      : "1px solid var(--border2)",
                    background: active ? "var(--accent-bg)" : "var(--panel2)",
                    color: active ? "var(--accent2)" : "var(--txt2)",
                  }}
                >
                  {mo.label}
                </button>
              );
            })}
          </div>
        </div>

        {/* Permission posture */}
        <div>
          <div
            style={{
              fontSize: "calc(14.5px * var(--fs,1))",
              fontWeight: 700,
              marginBottom: 9,
              color: "var(--txt2)",
            }}
          >
            Permission posture
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 7 }}>
            {POSTURE_OPTIONS.map((p) => {
              const active = posture === p.id;
              return (
                <div
                  key={p.id}
                  data-testid={`posture-${p.id}`}
                  onClick={() => setPosture(p.id)}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 11,
                    padding: "12px 14px",
                    borderRadius: 10,
                    cursor: "pointer",
                    border: active
                      ? "1px solid var(--accent)"
                      : "1px solid var(--border)",
                    background: active ? "var(--accent-bg)" : "var(--panel2)",
                  }}
                >
                  {/* Radio dot */}
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
                      {p.label}
                    </div>
                    <div style={{ fontSize: "calc(13.5px * var(--fs,1))", color: "var(--txt3)" }}>
                      {p.desc}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </div>

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
                  title={`${t.model} · ${t.posture}`}
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
