/**
 * Activity tab — full transcript render + Composer at bottom.
 *
 * The conversation comes from `sv.transcript` (the real per-session feed the
 * backend derives from claude's event stream). Live status rows (a pending
 * gate, the working spinner, an error note, sub-agents) are appended after the
 * transcript from the current session state.
 */
import { useRef, useEffect } from "react";
import { useVm } from "../../bridge/useVm";
import { Composer } from "../../components/Composer";
import { Markdown } from "../../components/Markdown";
import { ToolUse } from "../../components/ToolUse";
import { Watchtower } from "../../components/Watchtower";
import {
  sendPrompt,
  interrupt,
  runSlash,
  runShellCommand,
  filterSlash,
} from "../../bridge/api";
import type { SlashEntry, SessionView, GateVM } from "../../bridge/types";

// ---------------------------------------------------------------------------
// Transcript message types (internal to Activity tab)
// ---------------------------------------------------------------------------

type MsgKind =
  | "prompt"
  | "response"
  | "tool"
  | "shell_cmd"
  | "shell_out"
  | "result"
  | "error"
  | "working"
  | "gate"
  | "system"
  | "subagent";

interface TranscriptMsg {
  id: string;
  kind: MsgKind;
  text?: string;
  // tool call fields
  tool?: string;
  args?: string;
  toolStatus?: string;
  // subagent fields
  name?: string;
  model?: string;
  subStatus?: string;
  // gate fields
  gateLead?: string;
  gateSummary?: string;
}

// ---------------------------------------------------------------------------
// Build the transcript: the real per-session conversation feed
// (`sv.transcript`) followed by live status rows from the current state.
// ---------------------------------------------------------------------------

function deriveTranscript(
  sv: SessionView,
  gate: GateVM | null
): TranscriptMsg[] {
  const msgs: TranscriptMsg[] = [];

  // "session started" system note
  msgs.push({
    id: "sys-0",
    kind: "system",
    text: `Session "${sv.name}" · ${sv.model}${sv.branch ? ` · ${sv.branch}` : ""}`,
  });

  // The real conversation, in arrival order.
  sv.transcript.forEach((entry, i) => {
    if (entry.role === "assistant") {
      msgs.push({ id: `t-${i}`, kind: "response", text: entry.text });
    } else if (entry.role === "user") {
      // Shell commands injected by the "!" prefix are User entries starting with
      // "! " — render them as a distinct shell command row instead of a prompt bubble.
      if (entry.text.startsWith("! ")) {
        msgs.push({ id: `t-${i}`, kind: "shell_cmd", text: entry.text });
      } else {
        msgs.push({ id: `t-${i}`, kind: "prompt", text: entry.text });
      }
    } else if (entry.name === "shell") {
      // Shell output from "!" commands: render as a pre-formatted output block.
      msgs.push({ id: `t-${i}`, kind: "shell_out", text: entry.input });
    } else {
      msgs.push({
        id: `t-${i}`,
        kind: "tool",
        tool: entry.name,
        args: entry.input,
        toolStatus: "done",
      });
    }
  });

  // If there's an active gate, show it as a pending-gate card
  if (gate) {
    msgs.push({
      id: "gate-0",
      kind: "gate",
      gateLead: gate.lead,
      gateSummary: gate.summary,
    });
  }

  // Show a working spinner only while Claude is actually working. A freshly
  // launched session sits idle (claude emits nothing until prompted), so we
  // don't show a perpetual "Starting up…" spinner — the composer below is ready
  // for the first prompt.
  if (sv.state === "Working") {
    msgs.push({
      id: "working-0",
      kind: "working",
      text: "Working…",
    });
  }

  // If session has an error, show an error entry
  if (sv.state === "Error") {
    msgs.push({
      id: "error-0",
      kind: "error",
      text: "Session encountered an error. Check the terminal tab for details.",
    });
  }

  // If there are running subagents, show a running indicator
  if (sv.running_subagents > 0) {
    msgs.push({
      id: "subagent-running-0",
      kind: "subagent",
      name: `${sv.running_subagents} sub-agent${sv.running_subagents > 1 ? "s" : ""} running`,
      model: sv.model,
      subStatus: "Working",
    });
  }

  // If there are completed subagents, show a summary entry
  const completedSubagents = sv.subagents - sv.running_subagents;
  if (completedSubagents > 0) {
    msgs.push({
      id: "subagent-0",
      kind: "subagent",
      name: `${completedSubagents} sub-agent${completedSubagents > 1 ? "s" : ""}`,
      model: sv.model,
      subStatus: "Done",
    });
  }

  return msgs;
}

// ---------------------------------------------------------------------------
// Individual message renderers
// ---------------------------------------------------------------------------

function MAvatar() {
  return (
    <div
      style={{
        width: 26,
        height: 26,
        borderRadius: 7,
        background: "linear-gradient(135deg,var(--accent),var(--accent2))",
        flexShrink: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "#fff",
        fontWeight: 800,
        fontSize: "calc(13.5px * var(--fs,1))",
      }}
    >
      <Watchtower size={15} />
    </div>
  );
}

function PromptBubble({ text }: { text: string }) {
  return (
    <div
      className="md-bubble"
      style={{
        alignSelf: "flex-end",
        maxWidth: "78%",
        background: "var(--accent)",
        color: "#fff",
        padding: "11px 14px",
        borderRadius: "13px 13px 4px 13px",
        fontSize: "calc(15.5px * var(--fs,1))",
        lineHeight: 1.5,
        boxShadow: "var(--shadow-sm)",
        wordBreak: "break-word",
      }}
    >
      <Markdown text={text} />
    </div>
  );
}

function SystemNote({ text }: { text: string }) {
  return (
    <div
      className="mono"
      style={{
        alignSelf: "center",
        fontSize: "calc(13px * var(--fs,1))",
        color: "var(--txt3)",
        background: "var(--panel2)",
        border: "1px solid var(--border)",
        padding: "5px 13px",
        borderRadius: 20,
      }}
    >
      {text}
    </div>
  );
}

function ResponseMsg({ text }: { text: string }) {
  return (
    <div style={{ display: "flex", gap: 11 }}>
      <MAvatar />
      <div
        style={{
          flex: 1,
          fontSize: "calc(15.5px * var(--fs,1))",
          lineHeight: 1.6,
          color: "var(--txt)",
          paddingTop: 3,
          minWidth: 0,
        }}
      >
        <Markdown text={text} />
      </div>
    </div>
  );
}

// ToolCall is now handled by the imported ToolUse component (see ../../components/ToolUse.tsx).

function ToolResult({ text }: { text: string }) {
  return (
    <pre
      className="mono"
      style={{
        margin: "0 0 0 37px",
        padding: "11px 13px",
        borderRadius: 9,
        background: "var(--panel2)",
        border: "1px solid var(--border)",
        fontSize: "calc(13.5px * var(--fs,1))",
        lineHeight: 1.55,
        color: "var(--txt2)",
        whiteSpace: "pre-wrap",
        overflowX: "auto",
      }}
    >
      {text}
    </pre>
  );
}

/** Shell command row — shows the "! <cmd>" user entry as a distinct monospaced
 *  pill so it's visually different from a normal chat prompt. */
function ShellCmd({ text }: { text: string }) {
  return (
    <div
      className="mono"
      style={{
        alignSelf: "flex-end",
        maxWidth: "78%",
        background: "var(--panel2)",
        border: "1px solid var(--border2)",
        color: "var(--txt2)",
        padding: "8px 14px",
        borderRadius: "11px 11px 4px 11px",
        fontSize: "calc(14px * var(--fs,1))",
        lineHeight: 1.5,
        whiteSpace: "pre-wrap",
        wordBreak: "break-all",
      }}
    >
      {text}
    </div>
  );
}

/** Shell output block — preformatted, indented like a tool result. */
function ShellOutput({ text }: { text: string }) {
  return (
    <pre
      className="mono"
      style={{
        margin: "0 0 0 0",
        padding: "11px 13px",
        borderRadius: 9,
        background: "var(--panel)",
        border: "1px solid var(--border)",
        fontSize: "calc(13px * var(--fs,1))",
        lineHeight: 1.55,
        color: "var(--txt)",
        whiteSpace: "pre-wrap",
        overflowX: "auto",
      }}
    >
      {text}
    </pre>
  );
}

function ErrorMsg({ text }: { text: string }) {
  return (
    <pre
      className="mono"
      style={{
        margin: "0 0 0 37px",
        padding: "11px 13px",
        borderRadius: 9,
        background: "var(--error-bg)",
        border: "1px solid var(--error)",
        fontSize: "calc(14px * var(--fs,1))",
        color: "var(--error)",
        whiteSpace: "pre-wrap",
      }}
    >
      {text}
    </pre>
  );
}

function WorkingSpinner({ text }: { text: string }) {
  return (
    <div style={{ display: "flex", gap: 11, alignItems: "center" }}>
      <MAvatar />
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 9,
          color: "var(--txt2)",
          fontSize: "calc(15px * var(--fs,1))",
        }}
      >
        <span
          style={{
            width: 13,
            height: 13,
            border: "2px solid var(--border2)",
            borderTopColor: "var(--accent)",
            borderRadius: "50%",
            animation: "spin .8s linear infinite",
            display: "inline-block",
          }}
        />
        {text}
      </div>
    </div>
  );
}

function GateCard({
  lead,
  summary,
  onReview,
}: {
  lead: string;
  summary: string;
  onReview: () => void;
}) {
  return (
    <div
      style={{
        marginLeft: 37,
        borderRadius: 11,
        border: "1px solid var(--needs)",
        background: "var(--needs-bg)",
        padding: 14,
        animation: "glow 2.6s ease-in-out infinite",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          marginBottom: 7,
        }}
      >
        <span style={{ fontSize: "calc(14.5px * var(--fs,1))" }}>▲</span>
        <span
          style={{
            fontWeight: 700,
            fontSize: "calc(15px * var(--fs,1))",
            color: "var(--needs)",
          }}
        >
          {lead}
        </span>
      </div>
      <div
        className="mono"
        style={{ fontSize: "calc(14.5px * var(--fs,1))", color: "var(--txt)", marginBottom: 11 }}
      >
        {summary}
      </div>
      <button
        onClick={onReview}
        style={{
          fontFamily: "inherit",
          fontWeight: 700,
          fontSize: "calc(14.5px * var(--fs,1))",
          padding: "8px 15px",
          borderRadius: 8,
          border: "none",
          background: "var(--needs)",
          color: "#0e1116",
          cursor: "pointer",
        }}
      >
        Review &amp; decide →
      </button>
    </div>
  );
}

function SubagentEntry({
  name,
  model,
  subStatus,
}: {
  name: string;
  model?: string;
  subStatus?: string;
}) {
  const isWorking = subStatus === "Working";
  const statusColor = isWorking ? "var(--accent2)" : "var(--done)";
  const statusBg = isWorking ? "var(--working-bg)" : "var(--done-bg)";
  return (
    <div
      style={{
        marginLeft: 37,
        borderLeft: "2px solid var(--accent)",
        padding: "3px 0 3px 14px",
        display: "flex",
        flexDirection: "column",
        gap: 7,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 9 }}>
        <span style={{ fontSize: "calc(14.5px * var(--fs,1))", color: "var(--accent2)" }}>⊟</span>
        <span style={{ fontWeight: 700, fontSize: "calc(14.5px * var(--fs,1))" }}>Subagent · {name}</span>
        {model && (
          <span
            className="mono"
            style={{ fontSize: "calc(12.5px * var(--fs,1))", color: "var(--txt3)" }}
          >
            {model}
          </span>
        )}
        {subStatus && (
          <span
            style={{
              fontSize: "calc(11.5px * var(--fs,1))",
              fontWeight: 700,
              padding: "1px 7px",
              borderRadius: 5,
              textTransform: "uppercase" as const,
              letterSpacing: 0.3,
              color: statusColor,
              background: statusBg,
            }}
          >
            {subStatus}
          </span>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// ActivityTab
// ---------------------------------------------------------------------------

interface ActivityTabProps {
  sessionId: string;
  onOpenGate: () => void;
}

export function ActivityTab({ sessionId, onOpenGate }: ActivityTabProps) {
  const { selectedSession, selectedGate } = useVm();
  const bottomRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to the bottom whenever the transcript grows or the session/phase
  // changes, so the latest message is always in view by default. Track the
  // transcript length AND a signature of the last entry (so a streaming message
  // that grows in place still scrolls), plus the phase (drives live status rows),
  // the gate, and the session id (jump to bottom when switching sessions).
  const transcriptLen = selectedSession?.transcript?.length ?? 0;
  const lastEntrySig =
    transcriptLen > 0
      ? JSON.stringify(selectedSession!.transcript[transcriptLen - 1])
      : "";
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "auto" });
  }, [
    selectedSession?.id,
    transcriptLen,
    lastEntrySig,
    selectedSession?.state,
    selectedGate?.decision_id,
  ]);

  const sv = selectedSession;
  if (!sv) {
    return (
      <div style={{ padding: 20, color: "var(--txt3)" }}>
        No session selected.
      </div>
    );
  }

  const messages = deriveTranscript(sv, selectedGate);

  // Derive prompt history for the Composer (Up/Down arrow recall).
  // Collect user-typed prompts in arrival order, skipping shell "! " entries.
  const promptHistory: string[] = sv.transcript.flatMap((entry) => {
    if (entry.role !== "user") return [];
    return entry.text.startsWith("! ") ? [] : [entry.text];
  });

  function handleSend(text: string) {
    // If text starts with "!" run the shell command on the session's host.
    // The command is stripped of the leading "!" and trimmed.
    if (text.startsWith("!")) {
      const cmd = text.slice(1).trim();
      if (cmd.length > 0) {
        runShellCommand(sessionId, cmd);
        return;
      }
      // A bare "!" with no command is silently ignored.
      return;
    }
    // If text starts with "/" and matches a slash command, run it. Slash
    // commands are session-scoped only (sv.slash_commands, from system/init) —
    // no preset catalog fallback (dynamic-capabilities.md §1.3).
    if (text.startsWith("/")) {
      const matches = filterSlash(text, sv?.slash_commands);
      if (matches.length > 0) {
        runSlash(sessionId, matches[0].name);
        return;
      }
    }
    sendPrompt(sessionId, text);
  }

  function handleInterrupt() {
    interrupt(sessionId);
  }

  function handleSlashSelect(entry: SlashEntry) {
    runSlash(sessionId, entry.name);
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", overflow: "hidden" }}>
      {/* Transcript scroll area */}
      <div
        id="transcript"
        style={{ flex: 1, overflowY: "auto", padding: "20px 24px" }}
      >
        <div
          style={{
            maxWidth: 1040,
            margin: "0 auto",
            display: "flex",
            flexDirection: "column",
            gap: 14,
          }}
        >
          {messages.map((msg) => {
            switch (msg.kind) {
              case "prompt":
                return <PromptBubble key={msg.id} text={msg.text ?? ""} />;
              case "system":
                return <SystemNote key={msg.id} text={msg.text ?? ""} />;
              case "response":
                return <ResponseMsg key={msg.id} text={msg.text ?? ""} />;
              case "tool":
                return (
                  <ToolUse
                    key={msg.id}
                    tool={msg.tool ?? ""}
                    args={msg.args}
                    toolStatus={msg.toolStatus}
                  />
                );
              case "result":
                return <ToolResult key={msg.id} text={msg.text ?? ""} />;
              case "shell_cmd":
                return <ShellCmd key={msg.id} text={msg.text ?? ""} />;
              case "shell_out":
                return <ShellOutput key={msg.id} text={msg.text ?? ""} />;
              case "error":
                return <ErrorMsg key={msg.id} text={msg.text ?? ""} />;
              case "working":
                return (
                  <WorkingSpinner key={msg.id} text={msg.text ?? "Working…"} />
                );
              case "gate":
                return (
                  <GateCard
                    key={msg.id}
                    lead={msg.gateLead ?? ""}
                    summary={msg.gateSummary ?? ""}
                    onReview={onOpenGate}
                  />
                );
              case "subagent":
                return (
                  <SubagentEntry
                    key={msg.id}
                    name={msg.name ?? ""}
                    model={msg.model}
                    subStatus={msg.subStatus}
                  />
                );
              default:
                return null;
            }
          })}
          <div ref={bottomRef} />
        </div>
      </div>

      {/* Composer */}
      <div
        style={{
          flexShrink: 0,
          borderTop: "1px solid var(--border)",
          padding: "14px 24px",
          background: "var(--panel)",
        }}
      >
        <div style={{ maxWidth: 1040, margin: "0 auto" }}>
          <Composer
            sessionId={sessionId}
            onSend={handleSend}
            onInterrupt={handleInterrupt}
            onSlashSelect={handleSlashSelect}
            history={promptHistory}
            slashCommands={sv.slash_commands}
          />
        </div>
      </div>
    </div>
  );
}
