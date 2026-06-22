/**
 * Terminal tab — escape-hatch raw session view + command input.
 *
 * NOTE: SessionView carries no raw terminal output; it only provides summary
 * counts. We render a stub output with a clear injection point. A real feed
 * would subscribe to per-session stdout/stderr lines via Tauri events once
 * the backend exposes a terminal-stream channel.
 */
import React, { useRef, useState } from "react";
import { useVm } from "../../bridge/useVm";
import { sendPrompt } from "../../bridge/api";

interface TermLine {
  text: string;
  color: string;
}

// ---------------------------------------------------------------------------
// Derive stub terminal lines from the session state.
// NOTE: Replace with real terminal stream lines once backend exposes them.
// ---------------------------------------------------------------------------

function deriveTermLines(
  sv: import("../../bridge/types").SessionView
): TermLine[] {
  const lines: TermLine[] = [
    { text: `# Session: ${sv.name}`, color: "var(--txt3)" },
    { text: `# Host: ${sv.host_id}  Branch: ${sv.branch}`, color: "var(--txt3)" },
    { text: `# Model: ${sv.model}  State: ${sv.state}`, color: "var(--txt3)" },
    { text: "", color: "var(--txt)" },
    { text: `$ claude --model ${sv.model} --project "${sv.project}"`, color: "var(--done)" },
  ];

  if (sv.state === "Working" || sv.state === "Starting") {
    lines.push({ text: "Claude is working…", color: "var(--txt2)" });
  }
  if (sv.state === "Error") {
    lines.push({
      text: "Error: session encountered an issue. See logs for details.",
      color: "var(--error)",
    });
  }
  if (sv.state === "Done") {
    lines.push({ text: "Session complete.", color: "var(--done)" });
  }

  // NOTE: Append sv.termLines[] here when backend streams terminal output.

  return lines;
}

// ---------------------------------------------------------------------------
// TerminalTab
// ---------------------------------------------------------------------------

interface TerminalTabProps {
  sessionId: string;
}

export function TerminalTab({ sessionId }: TerminalTabProps) {
  const { selectedSession } = useVm();
  const inputRef = useRef<HTMLInputElement>(null);
  const [inputVal, setInputVal] = useState("");

  if (!selectedSession) {
    return (
      <div style={{ padding: 20, color: "var(--txt3)" }}>
        No session selected.
      </div>
    );
  }

  const lines = deriveTermLines(selectedSession);

  function focusTerm() {
    inputRef.current?.focus();
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter" && inputVal.trim()) {
      // Send input as a raw prompt to the session
      sendPrompt(sessionId, inputVal.trim());
      setInputVal("");
    }
  }

  const termTitle = `${selectedSession.host_id}:${selectedSession.branch}`;

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        background: "#0a0d12",
      }}
    >
      {/* Header bar */}
      <div
        style={{
          flexShrink: 0,
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "10px 18px",
          borderBottom: "1px solid var(--border)",
          background: "var(--panel)",
        }}
      >
        <span
          style={{ fontSize: 11, fontWeight: 700, color: "var(--needs)" }}
        >
          ⎈ Escape hatch
        </span>
        <span style={{ fontSize: 11.5, color: "var(--txt2)" }}>
          Full raw session — anything the purpose-built UI doesn't cover, do
          it here.
        </span>
        <span style={{ flex: 1 }} />
        <span
          className="mono"
          style={{ fontSize: 10.5, color: "var(--txt3)" }}
        >
          {termTitle}
        </span>
      </div>

      {/* Terminal output area */}
      <div
        style={{ flex: 1, overflowY: "auto", padding: "14px 18px" }}
        onClick={focusTerm}
        data-testid="terminal-output"
      >
        {lines.map((line, i) => (
          <div
            key={i}
            className="mono"
            style={{
              fontSize: 12,
              lineHeight: 1.65,
              whiteSpace: "pre-wrap",
              color: line.color,
            }}
          >
            {line.text}
          </div>
        ))}

        {/* Input row */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            marginTop: 6,
          }}
        >
          <span
            className="mono"
            style={{ fontSize: 12.5, color: "var(--done)" }}
          >
            ❯
          </span>
          <input
            ref={inputRef}
            value={inputVal}
            onChange={(e) => setInputVal(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="type a command, e.g. git status"
            className="mono"
            data-testid="terminal-input"
            style={{
              flex: 1,
              background: "none",
              border: "none",
              outline: "none",
              color: "var(--txt)",
              fontSize: 12,
            }}
          />
        </div>
      </div>
    </div>
  );
}
