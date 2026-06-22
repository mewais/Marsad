/**
 * Activity tab — full transcript render + Composer at bottom.
 *
 * NOTE: SessionView has no transcript array field in the current store —
 * it only carries summary counts (tokens, cost, subagents, changed_files).
 * We render a placeholder transcript from the gate/state shape and leave a
 * clearly-marked injection point where a richer per-session transcript feed
 * (e.g. via a new "transcript" field on SessionView or a separate Tauri command)
 * will plug in once the backend exposes it.
 */
import { useRef, useEffect } from "react";
import { useVm } from "../../bridge/useVm";
import { Composer } from "../../components/Composer";
import {
  sendPrompt,
  interrupt,
  runSlash,
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
// Derive a synthetic transcript from whatever the snapshot exposes.
// NOTE: Replace this function with a real transcript feed once the backend
// exposes per-session message history (e.g. SessionView.transcript[]).
// ---------------------------------------------------------------------------

function deriveTranscript(
  sv: SessionView,
  gate: GateVM | null
): TranscriptMsg[] {
  const msgs: TranscriptMsg[] = [];

  // Synthetic "session started" system note
  msgs.push({
    id: "sys-0",
    kind: "system",
    text: `Session "${sv.name}" · ${sv.model} · ${sv.branch}`,
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

  // If there are subagents, show a stub subagent entry
  if (sv.subagents > 0) {
    msgs.push({
      id: "subagent-0",
      kind: "subagent",
      name: `${sv.subagents} sub-agent${sv.subagents > 1 ? "s" : ""}`,
      model: sv.model,
      subStatus: sv.state === "Working" ? "Working" : "Done",
    });
  }

  // NOTE: Once backend exposes sv.transcript[], replace everything above
  // with a direct map over sv.transcript to build TranscriptMsg[].

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
        fontSize: 11,
      }}
    >
      M
    </div>
  );
}

function PromptBubble({ text }: { text: string }) {
  return (
    <div
      style={{
        alignSelf: "flex-end",
        maxWidth: "78%",
        background: "var(--accent)",
        color: "#fff",
        padding: "11px 14px",
        borderRadius: "13px 13px 4px 13px",
        fontSize: 13,
        lineHeight: 1.5,
        boxShadow: "var(--shadow-sm)",
      }}
    >
      {text}
    </div>
  );
}

function SystemNote({ text }: { text: string }) {
  return (
    <div
      className="mono"
      style={{
        alignSelf: "center",
        fontSize: 10.5,
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
          fontSize: 13,
          lineHeight: 1.6,
          color: "var(--txt)",
          paddingTop: 3,
        }}
      >
        {text}
      </div>
    </div>
  );
}

function ToolCall({
  tool,
  args,
  toolStatus,
}: {
  tool: string;
  args?: string;
  toolStatus?: string;
}) {
  return (
    <div
      style={{
        marginLeft: 37,
        display: "flex",
        alignItems: "center",
        gap: 9,
        padding: "9px 12px",
        borderRadius: 9,
        background: "var(--panel)",
        border: "1px solid var(--border)",
        fontSize: 11.5,
      }}
    >
      <span style={{ fontSize: 11, color: "var(--accent2)" }}>⚙</span>
      <span style={{ fontWeight: 700, color: "var(--txt)" }}>{tool}</span>
      <span
        className="mono"
        style={{
          color: "var(--txt2)",
          whiteSpace: "nowrap",
          overflow: "hidden",
          textOverflow: "ellipsis",
          flex: 1,
        }}
      >
        {args ?? ""}
      </span>
      {toolStatus && (
        <span style={{ fontSize: 10, color: "var(--done)" }}>{toolStatus}</span>
      )}
    </div>
  );
}

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
        fontSize: 11,
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
        fontSize: 11.5,
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
          fontSize: 12.5,
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
        <span style={{ fontSize: 12 }}>▲</span>
        <span
          style={{
            fontWeight: 700,
            fontSize: 12.5,
            color: "var(--needs)",
          }}
        >
          {lead}
        </span>
      </div>
      <div
        className="mono"
        style={{ fontSize: 12, color: "var(--txt)", marginBottom: 11 }}
      >
        {summary}
      </div>
      <button
        onClick={onReview}
        style={{
          fontFamily: "inherit",
          fontWeight: 700,
          fontSize: 12,
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
        <span style={{ fontSize: 12, color: "var(--accent2)" }}>⊟</span>
        <span style={{ fontWeight: 700, fontSize: 12 }}>Subagent · {name}</span>
        {model && (
          <span
            className="mono"
            style={{ fontSize: 10, color: "var(--txt3)" }}
          >
            {model}
          </span>
        )}
        {subStatus && (
          <span
            style={{
              fontSize: 9,
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

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [selectedSession?.state]);

  const sv = selectedSession;
  if (!sv) {
    return (
      <div style={{ padding: 20, color: "var(--txt3)" }}>
        No session selected.
      </div>
    );
  }

  const messages = deriveTranscript(sv, selectedGate);

  function handleSend(text: string) {
    // If text starts with "/" and matches a slash command, run it
    if (text.startsWith("/")) {
      const matches = filterSlash(text);
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
            maxWidth: 780,
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
                  <ToolCall
                    key={msg.id}
                    tool={msg.tool ?? ""}
                    args={msg.args}
                    toolStatus={msg.toolStatus}
                  />
                );
              case "result":
                return <ToolResult key={msg.id} text={msg.text ?? ""} />;
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
        <div style={{ maxWidth: 780, margin: "0 auto" }}>
          <Composer
            onSend={handleSend}
            onInterrupt={handleInterrupt}
            onSlashSelect={handleSlashSelect}
          />
        </div>
      </div>
    </div>
  );
}
