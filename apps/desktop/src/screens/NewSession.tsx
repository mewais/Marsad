/**
 * NewSession modal — project dir, host, model, posture, first prompt, launch.
 */
import { useState } from "react";
import { useVm } from "../bridge/useVm";
import { launchSession } from "../bridge/api";
import { Modal } from "../components/Modal";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MODELS = [
  { id: "claude-haiku-4-5", label: "Haiku", sublabel: "Fast · cheap" },
  { id: "claude-sonnet-4-5", label: "Sonnet", sublabel: "Balanced" },
  { id: "claude-opus-4-5", label: "Opus", sublabel: "Most capable" },
] as const;

const POSTURES: { value: "ask" | "auto" | "full"; label: string; desc: string }[] =
  [
    { value: "ask", label: "Ask", desc: "Gate every action" },
    { value: "auto", label: "Auto", desc: "Gate risky actions only" },
    { value: "full", label: "Full auto", desc: "No gates" },
  ];

const RECENT_DIRS = [
  "~/projects/api",
  "~/projects/dashboard",
  "~/projects/worker-svc",
  "~/projects/ml-pipeline",
];

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
  const [model, setModel] = useState<string>("claude-sonnet-4-5");
  const [posture, setPosture] = useState<"ask" | "auto" | "full">("auto");
  const [firstPrompt, setFirstPrompt] = useState("");
  const [busy, setBusy] = useState(false);

  function close() {
    dispatch({ type: "SET_SCREEN", screen: "overview" });
  }

  function handleRecentDir(dir: string) {
    setProject(dir);
  }

  async function handleLaunch() {
    if (!project.trim()) return;
    setBusy(true);
    const newId = await launchSession({
      project: project.trim(),
      host_id: hostId,
      model,
      posture,
      first_prompt: firstPrompt.trim() || undefined,
    });
    setBusy(false);
    // Navigate to the new session
    dispatch({ type: "SELECT_SESSION", id: newId });
  }

  return (
    <Modal open={open} onClose={close} width={520}>
      {/* ===== HEADER ===== */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "18px 22px 14px",
          borderBottom: "1px solid var(--border)",
        }}
      >
        <div style={{ flex: 1 }}>
          <div style={{ fontWeight: 700, fontSize: 16 }}>New session</div>
          <div style={{ fontSize: 11, color: "var(--txt3)", marginTop: 2 }}>
            Launch a new Claude Code session
          </div>
        </div>
        <button
          onClick={close}
          aria-label="Close"
          style={{
            background: "none",
            border: "none",
            cursor: "pointer",
            fontSize: 18,
            color: "var(--txt3)",
            lineHeight: 1,
            padding: "2px 4px",
          }}
        >
          ✕
        </button>
      </div>

      {/* ===== BODY ===== */}
      <div
        style={{
          padding: "18px 22px",
          display: "flex",
          flexDirection: "column",
          gap: 18,
        }}
      >
        {/* ---- Project directory ---- */}
        <div>
          <label
            htmlFor="ns-project"
            style={{
              display: "block",
              fontSize: 10,
              fontWeight: 700,
              letterSpacing: 0.6,
              textTransform: "uppercase",
              color: "var(--txt3)",
              marginBottom: 6,
            }}
          >
            Project directory
          </label>
          <input
            id="ns-project"
            data-testid="ns-project-input"
            value={project}
            onChange={(e) => setProject(e.target.value)}
            placeholder="~/projects/my-app"
            style={{
              width: "100%",
              boxSizing: "border-box",
              background: "var(--panel2)",
              border: "1px solid var(--border2)",
              borderRadius: 8,
              padding: "9px 12px",
              color: "var(--txt)",
              fontFamily: "var(--font-mono, monospace)",
              fontSize: 12.5,
              outline: "none",
            }}
          />
          {/* Recent dir chips */}
          <div
            style={{
              display: "flex",
              flexWrap: "wrap",
              gap: 6,
              marginTop: 8,
            }}
          >
            {RECENT_DIRS.map((dir) => (
              <Chip
                key={dir}
                label={dir}
                active={project === dir}
                onClick={() => handleRecentDir(dir)}
              />
            ))}
          </div>
        </div>

        {/* ---- Host picker ---- */}
        <div>
          <div
            style={{
              fontSize: 10,
              fontWeight: 700,
              letterSpacing: 0.6,
              textTransform: "uppercase",
              color: "var(--txt3)",
              marginBottom: 8,
            }}
          >
            Host
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {hosts.map((h) => {
              const selected = hostId === h.id;
              return (
                <button
                  key={h.id}
                  data-testid={`ns-host-${h.id}`}
                  onClick={() => setHostId(h.id)}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 10,
                    padding: "9px 13px",
                    borderRadius: 9,
                    border: selected
                      ? "1px solid var(--accent)"
                      : "1px solid var(--border)",
                    background: selected ? "var(--accent-bg)" : "var(--panel2)",
                    cursor: "pointer",
                    fontFamily: "inherit",
                    textAlign: "left",
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
                  <span
                    style={{
                      fontWeight: 600,
                      fontSize: 12.5,
                      color: selected ? "var(--accent2)" : "var(--txt)",
                      flex: 1,
                    }}
                  >
                    {h.label}
                  </span>
                  <span
                    className="mono"
                    style={{ fontSize: 10, color: "var(--txt3)" }}
                  >
                    {h.target}
                  </span>
                </button>
              );
            })}
          </div>
        </div>

        {/* ---- Model picker ---- */}
        <div>
          <div
            style={{
              fontSize: 10,
              fontWeight: 700,
              letterSpacing: 0.6,
              textTransform: "uppercase",
              color: "var(--txt3)",
              marginBottom: 8,
            }}
          >
            Model
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            {MODELS.map((m) => {
              const selected = model === m.id;
              return (
                <button
                  key={m.id}
                  data-testid={`ns-model-${m.id}`}
                  onClick={() => setModel(m.id)}
                  style={{
                    flex: 1,
                    padding: "9px 10px",
                    borderRadius: 9,
                    border: selected
                      ? "1px solid var(--accent)"
                      : "1px solid var(--border)",
                    background: selected ? "var(--accent-bg)" : "var(--panel2)",
                    cursor: "pointer",
                    fontFamily: "inherit",
                    textAlign: "center",
                  }}
                >
                  <div
                    style={{
                      fontWeight: 700,
                      fontSize: 12.5,
                      color: selected ? "var(--accent2)" : "var(--txt)",
                    }}
                  >
                    {m.label}
                  </div>
                  <div style={{ fontSize: 10, color: "var(--txt3)", marginTop: 2 }}>
                    {m.sublabel}
                  </div>
                </button>
              );
            })}
          </div>
        </div>

        {/* ---- Permission posture ---- */}
        <div>
          <div
            style={{
              fontSize: 10,
              fontWeight: 700,
              letterSpacing: 0.6,
              textTransform: "uppercase",
              color: "var(--txt3)",
              marginBottom: 8,
            }}
          >
            Permission posture
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            {POSTURES.map((p) => {
              const selected = posture === p.value;
              return (
                <button
                  key={p.value}
                  data-testid={`ns-posture-${p.value}`}
                  onClick={() => setPosture(p.value)}
                  style={{
                    flex: 1,
                    padding: "9px 10px",
                    borderRadius: 9,
                    border: selected
                      ? "1px solid var(--accent)"
                      : "1px solid var(--border)",
                    background: selected ? "var(--accent-bg)" : "var(--panel2)",
                    cursor: "pointer",
                    fontFamily: "inherit",
                    textAlign: "center",
                  }}
                >
                  <div
                    style={{
                      fontWeight: 700,
                      fontSize: 12.5,
                      color: selected ? "var(--accent2)" : "var(--txt)",
                    }}
                  >
                    {p.label}
                  </div>
                  <div style={{ fontSize: 10, color: "var(--txt3)", marginTop: 2 }}>
                    {p.desc}
                  </div>
                </button>
              );
            })}
          </div>
        </div>

        {/* ---- First prompt (optional) ---- */}
        <div>
          <label
            htmlFor="ns-first-prompt"
            style={{
              display: "block",
              fontSize: 10,
              fontWeight: 700,
              letterSpacing: 0.6,
              textTransform: "uppercase",
              color: "var(--txt3)",
              marginBottom: 6,
            }}
          >
            First prompt{" "}
            <span style={{ fontWeight: 400, textTransform: "none" }}>
              (optional)
            </span>
          </label>
          <textarea
            id="ns-first-prompt"
            data-testid="ns-first-prompt"
            value={firstPrompt}
            onChange={(e) => setFirstPrompt(e.target.value)}
            placeholder="e.g. Add a login endpoint to the API"
            rows={3}
            style={{
              width: "100%",
              boxSizing: "border-box",
              background: "var(--panel2)",
              border: "1px solid var(--border2)",
              borderRadius: 8,
              padding: "9px 12px",
              color: "var(--txt)",
              fontFamily: "inherit",
              fontSize: 12.5,
              resize: "vertical",
              outline: "none",
            }}
          />
        </div>
      </div>

      {/* ===== FOOTER ===== */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "12px 22px 18px",
          borderTop: "1px solid var(--border)",
          justifyContent: "space-between",
        }}
      >
        {/* Manage hosts link */}
        <button
          data-testid="ns-manage-hosts"
          onClick={() => dispatch({ type: "SET_SCREEN", screen: "settings" })}
          style={{
            background: "none",
            border: "none",
            cursor: "pointer",
            fontSize: 12,
            color: "var(--txt3)",
            fontFamily: "inherit",
            padding: 0,
          }}
        >
          Manage hosts →
        </button>

        <div style={{ display: "flex", gap: 10 }}>
          <Button variant="ghost" size="md" onClick={close} disabled={busy}>
            Cancel
          </Button>
          <Button
            variant="primary"
            size="md"
            data-testid="ns-launch-btn"
            onClick={handleLaunch}
            disabled={busy || !project.trim()}
          >
            Launch
          </Button>
        </div>
      </div>
    </Modal>
  );
}
