/**
 * Activity tab — full transcript render + Composer at bottom.
 *
 * The conversation comes from `sv.transcript` (the real per-session feed the
 * backend derives from claude's event stream). Live status rows (a pending
 * gate, the working spinner, an error note, sub-agents) are appended after the
 * transcript from the current session state.
 */
import { useRef, useEffect, useState, type ReactNode } from "react";
import { useVm } from "../../bridge/useVm";
import { Composer } from "../../components/Composer";
import { Markdown } from "../../components/Markdown";
import { ToolUse } from "../../components/ToolUse";
import { Watchtower } from "../../components/Watchtower";
import { CopyButton } from "../../components/CopyButton";
import {
  sendPrompt,
  interrupt,
  runSlash,
  runShellCommand,
  filterSlash,
  getResolvedGates,
  type ResolvedGateEntry,
} from "../../bridge/api";
import { endReasonText } from "../../bridge/types";
import type {
  SlashEntry,
  SessionView,
  Gate,
  PlanStep,
  QuestionPrompt,
  SubagentView,
} from "../../bridge/types";

// ---------------------------------------------------------------------------
// HITL collapse pattern (Workstreams redesign SLICE D) — "needs-you →
// resolved" is uniform across questions / permission / plan (and, since this
// codebase's Gate union also has a 4th kind not in the redesign doc, edit
// gates get the same treatment for consistency).
//
// A pending gate lives on `session.gate` — but once answered, the backend
// simply clears it; there is no history of past decisions to read back. So
// this module keeps a tiny, session-scoped log of every gate the UI has ever
// seen pending (`hitlEpisodes`, keyed by session id) plus every decision made
// through the GateOverlay modal (`getResolvedGates`, in bridge/api.ts — the
// ONLY other place that calls answerGate/answerQuestion). Combining "was this
// the current pending gate" (no) with "do we have a decision on record" (best
// effort) is enough to render the collapsed summary row. Module-level (not
// component state) so it survives the ActivityTab unmounting when the user
// switches tabs and back — same rationale as Composer's per-session draft
// store.
// ---------------------------------------------------------------------------

type HitlKind = "permission" | "plan" | "question" | "edit";

interface HitlEpisode {
  /** Kind-qualified identity, unique per gate OCCURRENCE — used for React
   *  keys and to dedup repeated sightings of the same pending gate. */
  id: string;
  /** The raw decision id this gate resolves under — exactly what GateOverlay
   *  passes to answerGate/answerQuestion (mirrors computeGate/GateVM in
   *  bridge/useVm.ts). Used to look up its resolution and to test whether
   *  it's the session's CURRENT pending gate. */
  resolutionKey: string;
  kind: HitlKind;
  /** Transcript length at the moment this episode was first recorded — used as
   *  the insertion index so the HITL card appears chronologically in the stream
   *  rather than always at the end (Bug #2). */
  transcriptIndexHint: number;
  permission?: { command: string; cwd: string; risk: string };
  plan?: { summary: string; steps: PlanStep[] };
  question?: { questions: QuestionPrompt[] };
  edit?: { file: string };
}

const hitlEpisodes = new Map<string, HitlEpisode[]>();

/** The decision id a gate resolves under (mirrors `computeGate()` in
 *  bridge/useVm.ts, which is what GateOverlay passes to answerGate). Permission
 *  and Question gates carry a real request id from the backend; Plan and Edit
 *  gates don't (yet) and both key by the plain session id — an existing
 *  backend/GateVM limitation, not new here. Two different gate KINDS in the
 *  same session could therefore share a resolutionKey; `hitlEpisodeId` below
 *  still tells them apart for episode identity, but their resolution-log
 *  entries would collide (see resolvedByDecision in buildHitlMessages). */
function hitlResolutionKey(sessionId: string, gate: Gate): string {
  if ("Permission" in gate) return gate.Permission.request_id;
  if ("Question" in gate) return gate.Question.request_id;
  return sessionId;
}

function hitlKind(gate: Gate): HitlKind {
  if ("Permission" in gate) return "permission";
  if ("Plan" in gate) return "plan";
  if ("Question" in gate) return "question";
  return "edit";
}

function hitlEpisodeId(sessionId: string, gate: Gate): string {
  return `${hitlKind(gate)}:${hitlResolutionKey(sessionId, gate)}`;
}

function recordHitlEpisode(sessionId: string, gate: Gate, transcriptLen: number): void {
  const id = hitlEpisodeId(sessionId, gate);
  const list = hitlEpisodes.get(sessionId) ?? [];
  if (list.some((e) => e.id === id)) return;
  const resolutionKey = hitlResolutionKey(sessionId, gate);
  let episode: HitlEpisode;
  if ("Permission" in gate) {
    episode = {
      id,
      resolutionKey,
      kind: "permission",
      transcriptIndexHint: transcriptLen,
      permission: {
        command: gate.Permission.command,
        cwd: gate.Permission.cwd,
        risk: gate.Permission.risk,
      },
    };
  } else if ("Plan" in gate) {
    episode = {
      id,
      resolutionKey,
      kind: "plan",
      transcriptIndexHint: transcriptLen,
      plan: { summary: gate.Plan.summary, steps: gate.Plan.steps },
    };
  } else if ("Question" in gate) {
    episode = {
      id,
      resolutionKey,
      kind: "question",
      transcriptIndexHint: transcriptLen,
      question: { questions: gate.Question.questions },
    };
  } else {
    episode = { id, resolutionKey, kind: "edit", transcriptIndexHint: transcriptLen, edit: { file: gate.Edit.file } };
  }
  hitlEpisodes.set(sessionId, [...list, episode]);
}

function getHitlEpisodes(sessionId: string): HitlEpisode[] {
  return hitlEpisodes.get(sessionId) ?? [];
}

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
  | "hitl"
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
  /** Per-subagent detail list for the collapsible expanded view. */
  subagentList?: SubagentView[];
  // hitl fields (questions/permission/plan/edit — see HitlEpisode above)
  episode?: HitlEpisode;
  pending?: boolean;
  resolution?: ResolvedGateEntry;
}

// ---------------------------------------------------------------------------
// Build the ordered list of HITL transcript entries for a session: every
// episode this UI has ever seen pending (oldest → newest), each flagged
// "pending" iff it's the session's CURRENT gate (this data model only ever
// has 0 or 1 pending gate at a time, so "not current" reliably means
// "resolved") and paired with its decision, when one was recorded.
// ---------------------------------------------------------------------------

function buildHitlMessages(sv: SessionView): TranscriptMsg[] {
  const episodes = getHitlEpisodes(sv.id);
  const resolved = getResolvedGates(sv.id);
  const resolutionByDecision = new Map(resolved.map((r) => [r.decision_id, r]));
  // Kind-qualified so a resolved Plan (say) never reads as "pending" just
  // because a later Edit gate on the same session shares its resolutionKey.
  const currentPendingEpisodeId = sv.gate ? hitlEpisodeId(sv.id, sv.gate) : null;
  return episodes.map((episode) => ({
    id: `hitl-${episode.id}`,
    kind: "hitl" as const,
    episode,
    pending: episode.id === currentPendingEpisodeId,
    resolution: resolutionByDecision.get(episode.resolutionKey),
  }));
}

// ---------------------------------------------------------------------------
// Build the transcript: the real per-session conversation feed
// (`sv.transcript`) followed by live status rows from the current state.
// ---------------------------------------------------------------------------

function deriveTranscript(
  sv: SessionView,
  hitlMsgs: TranscriptMsg[]
): TranscriptMsg[] {
  const msgs: TranscriptMsg[] = [];

  // "session started" system note
  msgs.push({
    id: "sys-0",
    kind: "system",
    text: `Session "${sv.name}" · ${sv.model}${sv.branch ? ` · ${sv.branch}` : ""}`,
  });

  // Build a lookup: transcript index → HITL messages to insert AFTER that entry.
  // transcriptIndexHint is sv.transcript.length at first-sighting of the gate.
  // hint=N means N entries existed; insert after entry[N-1] (0-based index N-1).
  // hint=0 means the gate appeared before any transcript → use sentinel key -1
  // to flush before the transcript loop output.
  const hitlByIndex = new Map<number, TranscriptMsg[]>();
  for (const hm of hitlMsgs) {
    const hint = hm.episode?.transcriptIndexHint ?? 0;
    // hint=0 → no transcript yet → sentinel -1; hint=N → after entry[N-1]
    const key = hint === 0 ? -1 : hint - 1;
    const bucket = hitlByIndex.get(key) ?? [];
    bucket.push(hm);
    hitlByIndex.set(key, bucket);
  }

  // HITL cards whose gate appeared before any transcript entries.
  const preTranscript = hitlByIndex.get(-1);
  if (preTranscript) msgs.push(...preTranscript);

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
    } else if (entry.role === "compaction") {
      // Compaction entry: render inline at its chronological position.
      msgs.push({
        id: `t-${i}`,
        kind: "system",
        text: `Context compacted (${entry.trigger}, ${entry.dropped_tokens} tokens dropped)`,
      });
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
    // Insert HITL cards immediately after the transcript entry that was the last
    // one present when the gate first appeared (Bug #2: chronological placement).
    const after = hitlByIndex.get(i);
    if (after) msgs.push(...after);
  });

  // Flush any HITL cards whose hint exceeds the current transcript length
  // (gate appeared at a point beyond what's now in sv.transcript — safe fallback).
  for (const [idx, bucket] of hitlByIndex) {
    if (idx >= sv.transcript.length) msgs.push(...bucket);
  }

  // Show a working spinner only while Claude is actually working. A freshly
  // launched session sits idle (claude emits nothing until prompted), so we
  // don't show a perpetual "Starting up…" spinner — the composer below is ready
  // for the first prompt. NOT while a `!` shell command runs: that surfaces the
  // session as Working (core::make_session_view) but has its own dedicated
  // "Running shell command…" spinner below — showing both is redundant. And NOT
  // when the session is Paused (live === false): a not-live session does no
  // work, so a "Working…" spinner would be a lie (two-axis model).
  if (sv.state === "Working" && !sv.shell_running && sv.live !== false) {
    msgs.push({
      id: "working-0",
      kind: "working",
      text: "Working…",
    });
  }

  // An async `!` shell command is running: show a working spinner after the
  // just-injected `! <cmd>` line, until its output arrives (run_shell runs the
  // command off-thread so the UI never freezes).
  if (sv.shell_running) {
    msgs.push({
      id: "shell-working-0",
      kind: "working",
      text: "Running shell command…",
    });
  }

  // Surface why the last turn ended, when noteworthy. A user cancel is calm
  // ("Cancelled by the user"); a real error (max turns / execution) is red with
  // its specific reason — never the old blank/generic error line.
  if (sv.end_reason) {
    msgs.push({
      id: "end-reason-0",
      kind: sv.end_reason === "Cancelled" ? "system" : "error",
      text: endReasonText(sv.end_reason),
    });
  } else if (sv.state === "Error") {
    // Fallback for an Error with no specific reason (older/edge streams).
    msgs.push({
      id: "error-0",
      kind: "error",
      text: "Session encountered an error. Check the terminal tab for details.",
    });
  }

  // Rate-limit indicator: surface the most recent rate_limit_event so the
  // user is never surprised by a silent throttle (owner "nothing dropped").
  if (sv.rate_limit) {
    msgs.push({
      id: "ratelimit-0",
      kind: "system",
      text: `Rate limit: ${sv.rate_limit.status}${
        sv.rate_limit.is_using_overage ? " (using overage)" : ""
      }`,
    });
  }

  // Running subagents: one collapsible entry with the full list.
  if (sv.running_subagents > 0) {
    msgs.push({
      id: "subagent-running-0",
      kind: "subagent",
      name: `${sv.running_subagents} sub-agent${sv.running_subagents > 1 ? "s" : ""} running`,
      subStatus: "Working",
      // Pass the full list for the expanded view; filter to running only for the header count.
      subagentList: sv.subagent_list?.filter(a => a.status === "Running") ?? [],
    });
  }

  // Completed subagents: summary entry (not collapsible — done is inert).
  const completedSubagents = sv.subagents - sv.running_subagents;
  if (completedSubagents > 0) {
    msgs.push({
      id: "subagent-0",
      kind: "subagent",
      name: `${completedSubagents} sub-agent${completedSubagents > 1 ? "s" : ""} completed`,
      subStatus: "Done",
      subagentList: sv.subagent_list?.filter(a => a.status === "Done") ?? [],
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

function SystemNote({ text, testId }: { text: string; testId?: string }) {
  return (
    <div
      className="mono"
      data-testid={testId}
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
      className="mono copy-wrap"
      style={{
        position: "relative",
        alignSelf: "flex-end",
        maxWidth: "78%",
        background: "var(--panel2)",
        border: "1px solid var(--border2)",
        color: "var(--txt2)",
        padding: "8px 34px 8px 14px",
        borderRadius: "11px 11px 4px 11px",
        fontSize: "calc(14px * var(--fs,1))",
        lineHeight: 1.5,
        whiteSpace: "pre-wrap",
        wordBreak: "break-all",
      }}
    >
      {text}
      {/* Copy the command without the leading "! " prefix. */}
      <CopyButton text={text.replace(/^!\s+/, "")} style={{ top: 5, right: 5 }} />
    </div>
  );
}

/** Shell output block — preformatted, indented like a tool result. */
function ShellOutput({ text }: { text: string }) {
  return (
    <div className="copy-wrap" style={{ position: "relative" }}>
      <pre
        className="mono"
        style={{
          margin: "0 0 0 0",
          padding: "11px 34px 11px 13px",
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
      <CopyButton text={text} style={{ top: 5, right: 5 }} />
    </div>
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

// ---------------------------------------------------------------------------
// HITL cards — the uniform "needs-you → resolved" collapse pattern.
// Pending: amber glowing card + Review/Answer CTA (opens the overlay — the
// Modals slice owns GateOverlay; this just dispatches to open it).
// Resolved: a muted, optionally `▸`-expandable summary row.
// ---------------------------------------------------------------------------

/** Shared amber "needs you" card shell — pending questions/permission/plan/edit. */
function PendingCard({
  lead,
  ctaLabel,
  onCta,
  meta,
  children,
}: {
  lead: string;
  ctaLabel: string;
  onCta: () => void;
  meta?: string;
  children?: ReactNode;
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
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 7 }}>
        <span style={{ color: "var(--needs)", fontWeight: 800 }}>▲</span>
        <span
          style={{
            fontWeight: 700,
            fontSize: "calc(14px * var(--fs,1))",
            color: "var(--needs)",
          }}
        >
          {lead}
        </span>
      </div>
      {children}
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <button
          onClick={onCta}
          style={{
            fontFamily: "inherit",
            fontWeight: 700,
            fontSize: "calc(14px * var(--fs,1))",
            padding: "8px 15px",
            borderRadius: 8,
            border: "none",
            background: "var(--needs)",
            color: "#0e1116",
            cursor: "pointer",
          }}
        >
          {ctaLabel}
        </button>
        {meta && (
          <span style={{ fontSize: "calc(12px * var(--fs,1))", color: "var(--txt3)" }}>
            {meta}
          </span>
        )}
      </div>
    </div>
  );
}

interface Decision {
  mark: string;
  text: string;
  color: string;
}

/** Shared resolved-row shell — a muted, optionally expandable summary. */
function ResolvedRow({
  icon,
  iconColor = "var(--txt2)",
  label,
  subtitle,
  decision,
  expandable,
  expanded,
  onToggle,
  children,
}: {
  icon: string;
  iconColor?: string;
  label: string;
  subtitle: string;
  decision?: Decision;
  expandable?: boolean;
  expanded?: boolean;
  onToggle?: () => void;
  children?: ReactNode;
}) {
  return (
    <div
      style={{
        marginLeft: 37,
        borderRadius: 9,
        border: "1px solid var(--border)",
        background: "var(--panel)",
        overflow: "hidden",
      }}
    >
      <div
        onClick={expandable ? onToggle : undefined}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 9,
          padding: "9px 12px",
          fontSize: "calc(12px * var(--fs,1))",
          cursor: expandable ? "pointer" : "default",
        }}
      >
        {expandable && (
          <span style={{ color: "var(--txt3)", fontSize: "calc(9px * var(--fs,1))", width: 9 }}>
            {expanded ? "▾" : "▸"}
          </span>
        )}
        <span style={{ color: iconColor }}>{icon}</span>
        <span style={{ fontWeight: 700 }}>{label}</span>
        <span
          className="mono"
          style={{
            color: "var(--txt3)",
            flex: 1,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {subtitle}
        </span>
        {decision && (
          <span
            className="mono"
            style={{ fontSize: "calc(10.5px * var(--fs,1))", color: decision.color, flexShrink: 0 }}
          >
            {decision.mark} {decision.text}
          </span>
        )}
      </div>
      {expandable && expanded && (
        <div style={{ padding: "0 13px 12px 30px" }}>{children}</div>
      )}
    </div>
  );
}

/** A single option chip — used by the Questions resolved-row expansion. */
function OptionChip({ label, picked }: { label: string; picked: boolean }) {
  return (
    <span
      style={{
        fontSize: "calc(11px * var(--fs,1))",
        fontWeight: 600,
        padding: "4px 10px",
        borderRadius: 7,
        border: `1px solid ${picked ? "var(--done)" : "var(--border2)"}`,
        background: picked ? "var(--done-bg)" : "var(--panel2)",
        color: picked ? "var(--done)" : "var(--txt3)",
      }}
    >
      {picked ? "✓ " : ""}
      {label}
    </span>
  );
}

function QuestionsHitl({
  episode,
  pending,
  resolution,
  onOpenGate,
}: {
  episode: HitlEpisode;
  pending: boolean;
  resolution?: ResolvedGateEntry;
  onOpenGate: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const questions = episode.question?.questions ?? [];

  if (pending) {
    const count = questions.length;
    return (
      <PendingCard
        lead={`${count} question${count === 1 ? "" : "s"} need you`}
        ctaLabel="Answer →"
        onCta={onOpenGate}
      >
        <div style={{ fontSize: "calc(13px * var(--fs,1))", marginBottom: 11 }}>
          {questions[0]?.question ?? ""}
        </div>
      </PendingCard>
    );
  }

  const answers = resolution?.answers;
  const answeredCount = answers ? Object.keys(answers).length : 0;

  return (
    <ResolvedRow
      icon="?"
      iconColor="var(--accent2)"
      label="Questions"
      subtitle={
        resolution
          ? `${answeredCount} of ${questions.length} answered`
          : "resolved"
      }
      expandable
      expanded={expanded}
      onToggle={() => setExpanded((x) => !x)}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 11 }}>
        {questions.map((q, qi) => {
          const picked = answers?.[q.question];
          const extra = picked && !q.options.some((o) => o.label === picked);
          return (
            <div key={qi}>
              <div style={{ fontSize: "calc(12.5px * var(--fs,1))", marginBottom: 6 }}>
                {q.question}
              </div>
              <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                {q.options.map((o, oi) => (
                  <OptionChip key={oi} label={o.label} picked={picked === o.label} />
                ))}
                {extra && <OptionChip label={picked!} picked />}
              </div>
            </div>
          );
        })}
      </div>
    </ResolvedRow>
  );
}

function PermissionHitl({
  episode,
  pending,
  resolution,
  onOpenGate,
}: {
  episode: HitlEpisode;
  pending: boolean;
  resolution?: ResolvedGateEntry;
  onOpenGate: () => void;
}) {
  const p = episode.permission;
  if (!p) return null;

  if (pending) {
    const preview = p.command.split("\n")[0];
    const meta = `${p.command.includes("\n") ? "multiline · " : ""}cwd ${p.cwd}`;
    return (
      <PendingCard lead="Permission needed" ctaLabel="Review →" onCta={onOpenGate} meta={meta}>
        <div
          className="mono"
          style={{
            fontSize: "calc(12px * var(--fs,1))",
            color: "var(--accent2)",
            background: "var(--bg)",
            border: "1px solid var(--border2)",
            borderRadius: 8,
            padding: "8px 12px",
            marginBottom: 11,
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          $ {preview}
        </div>
      </PendingCard>
    );
  }

  const decision: Decision | undefined = resolution
    ? {
        mark: resolution.allow ? "✓" : "•",
        text: resolution.allow ? "approved" : "rejected",
        color: resolution.allow ? "var(--done)" : "var(--error)",
      }
    : undefined;
  return (
    <ResolvedRow
      icon="▲"
      iconColor="var(--needs)"
      label="Permission"
      subtitle={`$ ${p.command}`}
      decision={decision}
    />
  );
}

function PlanHitl({
  episode,
  pending,
  resolution,
  onOpenGate,
}: {
  episode: HitlEpisode;
  pending: boolean;
  resolution?: ResolvedGateEntry;
  onOpenGate: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const plan = episode.plan;
  if (!plan) return null;

  if (pending) {
    return (
      <PendingCard
        lead="Plan mode — needs you"
        ctaLabel="Review plan →"
        onCta={onOpenGate}
        meta={`${plan.steps.length} steps`}
      >
        <div style={{ fontSize: "calc(12px * var(--fs,1))", color: "var(--txt2)", marginBottom: 12 }}>
          {plan.summary}
        </div>
      </PendingCard>
    );
  }

  const decision: Decision | undefined = resolution
    ? {
        mark: resolution.allow ? "✓" : "•",
        text: resolution.allow ? "Approved — exiting plan mode" : "Kept in plan mode",
        color: resolution.allow ? "var(--done)" : "var(--txt3)",
      }
    : undefined;
  return (
    <ResolvedRow
      icon="❑"
      label="Plan"
      subtitle={plan.summary.slice(0, 54)}
      decision={decision}
      expandable
      expanded={expanded}
      onToggle={() => setExpanded((x) => !x)}
    >
      <div style={{ fontSize: "calc(12px * var(--fs,1))", color: "var(--txt2)", marginBottom: 10 }}>
        {plan.summary}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {plan.steps.map((s, i) => (
          <div key={i} style={{ display: "flex", gap: 11, fontSize: "calc(12.5px * var(--fs,1))" }}>
            <span
              style={{
                width: 19,
                height: 19,
                flexShrink: 0,
                borderRadius: "50%",
                background: "var(--accent-bg)",
                color: "var(--accent2)",
                fontWeight: 800,
                fontSize: "calc(10px * var(--fs,1))",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
              }}
            >
              {i + 1}
            </span>
            <div>
              <div style={{ fontWeight: 600 }}>{s.title}</div>
              <div style={{ color: "var(--txt3)" }}>{s.detail}</div>
            </div>
          </div>
        ))}
      </div>
    </ResolvedRow>
  );
}

function EditHitl({
  episode,
  pending,
  resolution,
  onOpenGate,
}: {
  episode: HitlEpisode;
  pending: boolean;
  resolution?: ResolvedGateEntry;
  onOpenGate: () => void;
}) {
  const edit = episode.edit;
  if (!edit) return null;

  if (pending) {
    return (
      <PendingCard lead="Edit needs review" ctaLabel="Review →" onCta={onOpenGate}>
        <div
          className="mono"
          style={{ fontSize: "calc(12px * var(--fs,1))", color: "var(--accent2)", marginBottom: 11 }}
        >
          {edit.file}
        </div>
      </PendingCard>
    );
  }

  const decision: Decision | undefined = resolution
    ? {
        mark: resolution.allow ? "✓" : "•",
        text: resolution.allow ? "approved" : "rejected",
        color: resolution.allow ? "var(--done)" : "var(--error)",
      }
    : undefined;
  return <ResolvedRow icon="±" label="Edit" subtitle={edit.file} decision={decision} />;
}

function HitlCard({
  episode,
  pending,
  resolution,
  onOpenGate,
}: {
  episode: HitlEpisode;
  pending: boolean;
  resolution?: ResolvedGateEntry;
  onOpenGate: () => void;
}) {
  switch (episode.kind) {
    case "question":
      return (
        <QuestionsHitl episode={episode} pending={pending} resolution={resolution} onOpenGate={onOpenGate} />
      );
    case "permission":
      return (
        <PermissionHitl episode={episode} pending={pending} resolution={resolution} onOpenGate={onOpenGate} />
      );
    case "plan":
      return (
        <PlanHitl episode={episode} pending={pending} resolution={resolution} onOpenGate={onOpenGate} />
      );
    case "edit":
      return (
        <EditHitl episode={episode} pending={pending} resolution={resolution} onOpenGate={onOpenGate} />
      );
    default:
      return null;
  }
}

/** One row in the expanded per-subagent detail list. */
function SubagentDetailRow({ agent }: { agent: SubagentView }) {
  const isRunning = agent.status === "Running";
  const statusColor = isRunning ? "var(--accent2)" : "var(--done)";
  const statusBg = isRunning ? "var(--working-bg)" : "var(--done-bg)";
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 9,
        padding: "5px 0",
        borderBottom: "1px solid var(--border)",
      }}
    >
      <span style={{ fontSize: "calc(12px * var(--fs,1))", color: "var(--accent2)" }}>⊟</span>
      <span
        style={{
          flex: 1,
          fontSize: "calc(13px * var(--fs,1))",
          fontWeight: 600,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {agent.name || "(unnamed)"}
      </span>
      {agent.model && (
        <span
          className="mono"
          style={{
            fontSize: "calc(11px * var(--fs,1))",
            color: "var(--txt3)",
            flexShrink: 0,
          }}
        >
          {agent.model}
        </span>
      )}
      <span
        style={{
          fontSize: "calc(10.5px * var(--fs,1))",
          fontWeight: 700,
          padding: "1px 6px",
          borderRadius: 4,
          textTransform: "uppercase" as const,
          letterSpacing: 0.3,
          color: statusColor,
          background: statusBg,
          flexShrink: 0,
        }}
      >
        {agent.status}
      </span>
    </div>
  );
}

function SubagentEntry({
  name,
  model,
  subStatus,
  subagentList,
}: {
  name: string;
  model?: string;
  subStatus?: string;
  subagentList?: SubagentView[];
}) {
  const [expanded, setExpanded] = useState(false);
  const isWorking = subStatus === "Working";
  const statusColor = isWorking ? "var(--accent2)" : "var(--done)";
  const statusBg = isWorking ? "var(--working-bg)" : "var(--done-bg)";
  const hasDetail = subagentList && subagentList.length > 0;

  return (
    <div
      style={{
        marginLeft: 37,
        borderRadius: 9,
        border: "1px solid var(--border)",
        background: "var(--panel)",
        overflow: "hidden",
      }}
    >
      {/* Header row — always visible, clickable to toggle */}
      <div
        onClick={hasDetail ? () => setExpanded((x) => !x) : undefined}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 9,
          padding: "8px 12px",
          cursor: hasDetail ? "pointer" : "default",
        }}
      >
        {hasDetail && (
          <span style={{ color: "var(--txt3)", fontSize: "calc(9px * var(--fs,1))", width: 9 }}>
            {expanded ? "▾" : "▸"}
          </span>
        )}
        <span style={{ fontSize: "calc(14px * var(--fs,1))", color: "var(--accent2)" }}>⊟</span>
        <span style={{ fontWeight: 700, fontSize: "calc(14px * var(--fs,1))", flex: 1 }}>{name}</span>
        {model && (
          <span
            className="mono"
            style={{ fontSize: "calc(12px * var(--fs,1))", color: "var(--txt3)", flexShrink: 0 }}
          >
            {model}
          </span>
        )}
        {subStatus && (
          <span
            style={{
              fontSize: "calc(11px * var(--fs,1))",
              fontWeight: 700,
              padding: "1px 7px",
              borderRadius: 5,
              textTransform: "uppercase" as const,
              letterSpacing: 0.3,
              color: statusColor,
              background: statusBg,
              flexShrink: 0,
            }}
          >
            {subStatus}
          </span>
        )}
      </div>

      {/* Expanded per-agent detail list */}
      {hasDetail && expanded && (
        <div style={{ padding: "0 12px 10px 30px", borderTop: "1px solid var(--border)" }}>
          {subagentList!.map((agent, i) => (
            <SubagentDetailRow key={`${agent.name}-${i}`} agent={agent} />
          ))}
        </div>
      )}
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
  const { selectedSession, selectedGate, selectedSessionLoading } = useVm();
  const bottomRef = useRef<HTMLDivElement>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  // True while the user is at (or very near) the bottom; new content auto-scrolls
  // only when this is true so reading earlier content is never interrupted (Bug #6).
  const stickToBottomRef = useRef(true);

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
    if (stickToBottomRef.current) {
      bottomRef.current?.scrollIntoView({ behavior: "auto" });
    }
  }, [
    transcriptLen,
    lastEntrySig,
    selectedSession?.state,
    selectedGate?.decision_id,
  ]);

  // Session switch: always jump to bottom and re-enable stick-to-bottom.
  useEffect(() => {
    stickToBottomRef.current = true;
    bottomRef.current?.scrollIntoView({ behavior: "auto" });
  }, [selectedSession?.id]);

  const sv = selectedSession;

  // Record every gate this session shows as pending, so it survives past
  // resolution as a HITL episode (see the module doc-comment above). Done
  // directly during render (not a useEffect) — recordHitlEpisode is a
  // dedup-checked, idempotent write to a module-level cache, and the newly
  // recorded episode must be visible to THIS render's buildHitlMessages call
  // below, not just a later one (a useEffect here mutates the cache too late
  // to affect the transcript on the render that first sees the gate).
  if (sv?.gate) {
    recordHitlEpisode(sv.id, sv.gate, sv.transcript.length);
  }

  if (!sv) {
    return (
      <div style={{ padding: 20, color: "var(--txt3)" }}>
        No session selected.
      </div>
    );
  }

  // Launch gap: the session is selected and its name/header already show, but the
  // real SessionView (with transcript) hasn't arrived yet — show an honest
  // loading state instead of an empty body.
  if (selectedSessionLoading && sv.transcript.length === 0) {
    return (
      <div
        data-testid="transcript-loading"
        style={{
          padding: 24,
          color: "var(--txt3)",
          display: "flex",
          alignItems: "center",
          gap: 10,
        }}
      >
        <span
          aria-hidden="true"
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
        Loading transcript…
      </div>
    );
  }

  const messages = deriveTranscript(sv, buildHitlMessages(sv));

  // Derive prompt history for the Composer (Up/Down arrow recall).
  // Include user-typed prompts AND `! ` shell commands, in arrival order — the
  // shell entries recall verbatim as `! <cmd>` (the composer re-runs a leading
  // `!` as a shell command), so past shell commands are recallable too.
  const promptHistory: string[] = sv.transcript.flatMap((entry) =>
    entry.role === "user" ? [entry.text] : [],
  );

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
        ref={scrollContainerRef}
        id="transcript"
        style={{ flex: 1, overflowY: "auto", padding: "20px 24px" }}
        onScroll={() => {
          const el = scrollContainerRef.current;
          if (el) stickToBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
        }}
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
                return (
                  <SystemNote
                    key={msg.id}
                    text={msg.text ?? ""}
                    testId={
                      msg.id === "ratelimit-0"
                        ? "ratelimit-indicator"
                        : undefined
                    }
                  />
                );
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
              case "hitl":
                return msg.episode ? (
                  <HitlCard
                    key={msg.id}
                    episode={msg.episode}
                    pending={!!msg.pending}
                    resolution={msg.resolution}
                    onOpenGate={onOpenGate}
                  />
                ) : null;
              case "subagent":
                return (
                  <SubagentEntry
                    key={msg.id}
                    name={msg.name ?? ""}
                    model={msg.model}
                    subStatus={msg.subStatus}
                    subagentList={msg.subagentList}
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
