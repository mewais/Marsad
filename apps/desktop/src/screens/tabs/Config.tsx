/**
 * Config tab — model selector buttons, permission posture radios,
 * tools/connectors chips, Apply / Save-as-template actions.
 */
import { useState } from "react";
import { useVm } from "../../bridge/useVm";

// ---------------------------------------------------------------------------
// Static options (mirroring design tokens; wired to local UI state only —
// a real Apply would call a bridge command once the backend exposes one)
// ---------------------------------------------------------------------------

const MODEL_OPTIONS = [
  { id: "claude-opus-4-5", label: "Claude Opus 4.6" },
  { id: "claude-sonnet-4-5", label: "Claude Sonnet 4.6" },
  { id: "claude-haiku-4-5", label: "Claude Haiku 4" },
] as const;

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

const TOOL_CHIPS = [
  { id: "bash", label: "Bash" },
  { id: "edit", label: "Edit" },
  { id: "read", label: "Read" },
  { id: "web_search", label: "WebSearch" },
  { id: "git", label: "Git" },
  { id: "mcp", label: "MCP·postgres" },
] as const;

type ToolId = (typeof TOOL_CHIPS)[number]["id"];

// ---------------------------------------------------------------------------
// ConfigTab
// ---------------------------------------------------------------------------

export function ConfigTab() {
  const { selectedSession } = useVm();

  // Local state mirrors current session values (read from session); Apply
  // would push them to the backend once the bridge exposes an apply command.
  const initialModel = selectedSession?.model ?? "claude-sonnet-4-5";
  const [selectedModel, setSelectedModel] = useState(initialModel);
  const [posture, setPosture] = useState<PostureId>("ask");
  const [activeTools, setActiveTools] = useState<Set<ToolId>>(
    new Set(["bash", "edit", "read", "git"])
  );

  function toggleTool(tool: ToolId) {
    setActiveTools((prev) => {
      const next = new Set(prev);
      if (next.has(tool)) next.delete(tool);
      else next.add(tool);
      return next;
    });
  }

  function handleApply() {
    // NOTE: Wire to a bridge command (e.g. applySessionConfig) once the
    // backend exposes per-session config mutation.
  }

  function handleSaveTemplate() {
    // NOTE: Wire to saveTemplate bridge command.
  }

  return (
    <div style={{ flex: 1, overflowY: "auto", padding: 24 }}>
      <div
        style={{
          maxWidth: 660,
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
              fontSize: 12,
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
                    fontSize: 12,
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
              fontSize: 12,
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
                    <div style={{ fontWeight: 600, fontSize: 12.5 }}>
                      {p.label}
                    </div>
                    <div style={{ fontSize: 11, color: "var(--txt3)" }}>
                      {p.desc}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        {/* Tools & connectors */}
        <div>
          <div
            style={{
              fontSize: 12,
              fontWeight: 700,
              marginBottom: 9,
              color: "var(--txt2)",
            }}
          >
            Tools &amp; connectors
          </div>
          <div style={{ display: "flex", gap: 7, flexWrap: "wrap" }}>
            {TOOL_CHIPS.map((tc) => {
              const active = activeTools.has(tc.id);
              return (
                <span
                  key={tc.id}
                  data-testid={`tool-chip-${tc.id}`}
                  onClick={() => toggleTool(tc.id)}
                  style={{
                    fontFamily: "inherit",
                    fontSize: 11.5,
                    fontWeight: 600,
                    padding: "6px 11px",
                    borderRadius: 7,
                    cursor: "pointer",
                    border: active
                      ? "1px solid var(--accent)"
                      : "1px solid var(--border)",
                    background: active ? "var(--accent-bg)" : "var(--panel2)",
                    color: active ? "var(--accent2)" : "var(--txt3)",
                    userSelect: "none" as const,
                  }}
                >
                  {active ? "● " : "○ "}
                  {tc.label}
                </span>
              );
            })}
          </div>
        </div>

        {/* Action buttons */}
        <div style={{ display: "flex", gap: 9, paddingTop: 4 }}>
          <button
            data-testid="config-apply"
            onClick={handleApply}
            style={{
              fontFamily: "inherit",
              fontWeight: 700,
              fontSize: 12.5,
              padding: "9px 16px",
              borderRadius: 9,
              border: "none",
              background: "var(--accent)",
              color: "#fff",
              cursor: "pointer",
            }}
          >
            Apply to running session
          </button>
          <button
            data-testid="config-save-template"
            onClick={handleSaveTemplate}
            style={{
              fontFamily: "inherit",
              fontWeight: 600,
              fontSize: 12.5,
              padding: "9px 16px",
              borderRadius: 9,
              border: "1px solid var(--border2)",
              background: "var(--panel2)",
              color: "var(--txt2)",
              cursor: "pointer",
            }}
          >
            Save as template…
          </button>
        </div>
      </div>
    </div>
  );
}
