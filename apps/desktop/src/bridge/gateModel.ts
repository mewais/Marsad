/**
 * gateModel — the ONE normalized model every gate kind (permission / MCP
 * elicitation / plan / edit / question) collapses into, plus the pure mapping
 * from a chosen response back to the concrete answer-api call.
 *
 * GateOverlay used to carry five per-kind JSX branches, each with its own
 * option-building + answer-dispatch logic. This module extracts ALL of that
 * kind-specific knowledge into two pure functions:
 *
 *   normalizeGate(gv, raw) -> NormalizedGate   (what to render)
 *   responseToAnswer(...)  -> ApiCall          (what to send)
 *
 * so GateOverlay becomes a single render path over `NormalizedGate` with no
 * branching on kind.
 */
import type { Gate, GateVM, PermissionSuggestion, SideRow } from "./types";
import { answerGate, answerQuestion, answerWaslGate } from "./api";

// ---------------------------------------------------------------------------
// Response — the concrete thing the human picked. Kept abstract here; the
// mapping to an api call lives in responseToAnswer / responseToApiCall below.
// ---------------------------------------------------------------------------

export type GateResponse =
  | { type: "allow" }
  // free-text reason optional (carried separately as the confirm text).
  | { type: "deny" }
  | { type: "acpOption"; optionId: string }
  | { type: "elicitation"; key: string; value: string }
  // claude "always allow …" suggestion, echoed back VERBATIM.
  | { type: "permissionRule"; suggestion: unknown }
  | { type: "answer"; question: string; label: string }
  | { type: "skipQuestions" }
  // X1/X2: Wasl A2A send-gate responses — routed to answerWaslGate (host-scoped),
  // NOT the normal answerGate (session-scoped). `gate_id` is the Wasl connector
  // gate identity; `host_id` is supplied by the caller (GateOverlay has it from
  // selectedSession.host_id — the SessionView projection always carries host_id).
  | { type: "waslApprove"; gate_id: string }
  | { type: "waslDeny"; gate_id: string };

/** One selectable answer within a prompt. */
export interface GateOptionModel {
  id: string;
  testid?: string;
  label: string;
  description?: string;
  title?: string;
  response: GateResponse;
  /** When present, picking this option reveals a free-text field before firing. */
  freeText?: { label: string; placeholder?: string };
  tone?: "approve" | "reject" | "neutral";
}

/** A group of options the human answers together. Permission/plan/edit carry
 *  exactly one (no question text); a Question gate carries one per sub-question. */
export interface GatePromptModel {
  id: string;
  question?: string;
  header?: string;
  multiSelect: boolean;
  options: GateOptionModel[];
}

/** A rich body attachment rendered above the options. */
export type GateAttachment =
  | { kind: "command"; command: string; cwd: string }
  | { kind: "diff"; rows: SideRow[]; filename: string };

/** The unified gate model every kind normalizes into. */
export interface NormalizedGate {
  kind: string;
  title: string;
  body?: string;
  risk?: string;
  attachments: GateAttachment[];
  /** "pick-one": clicking an option fires it (or its free-text sub-mode).
   *  "collect": options accumulate per prompt and a Submit aggregates them. */
  mode: "pick-one" | "collect";
  prompts: GatePromptModel[];
  /** Present in collect mode — the aggregate Submit button. */
  submit?: { label: string };
  /** Present in collect mode — the always-available Skip option. */
  skip?: GateOptionModel;
}

// ---------------------------------------------------------------------------
// suggestionLabel — button text for one permission suggestion.
//
// ACP options carry a `name` field (verbatim from the agent) and no
// `behavior`/`rules`. claude options carry `behavior` + `rules`. Use `name`
// when present (ACP path); fall back to the claude behavior+rules path.
// (Moved verbatim from GateOverlay.)
// ---------------------------------------------------------------------------

export function suggestionLabel(s: PermissionSuggestion): string {
  if (typeof (s as Record<string, unknown>).name === "string") {
    return ((s as Record<string, unknown>).name as string).trim();
  }
  const behavior = (s.behavior ?? "allow").trim();
  const rules = (s.rules ?? [])
    .map((r) => r?.ruleContent?.trim())
    .filter((c): c is string => !!c);
  return rules.length ? `${behavior} ${rules.join(", ")}` : behavior;
}

/** True when any suggestion carries an ACP `optionId` (the ACP permission shape). */
function isAcpSuggestions(suggestions: PermissionSuggestion[]): boolean {
  return suggestions.some(
    (s) => typeof (s as Record<string, unknown>).optionId === "string"
  );
}

// ---------------------------------------------------------------------------
// Shared option factories
// ---------------------------------------------------------------------------

const DENY_FREE_TEXT = {
  label: "Reason for rejection (sent back to the session)",
  placeholder:
    "e.g. Don't run migrations against prod — target the staging DB instead.",
};

/** The Deny/Reject option shared by permission / elicitation / plan / edit. */
function denyOption(label: string): GateOptionModel {
  return {
    id: "deny",
    testid: "reject-btn",
    label,
    response: { type: "deny" },
    tone: "reject",
    freeText: DENY_FREE_TEXT,
  };
}

// ---------------------------------------------------------------------------
// normalizeGate — pure per-kind normalization.
// ---------------------------------------------------------------------------

export function normalizeGate(gv: GateVM, raw: Gate): NormalizedGate {
  // ---- PERMISSION ----
  if ("Permission" in raw) {
    const p = raw.Permission;
    const attachments: GateAttachment[] = [];
    if (p.command.trim()) {
      attachments.push({ kind: "command", command: p.command, cwd: p.cwd });
    }
    const suggestions = p.permission_suggestions ?? [];
    const choices = p.elicitation_choices ?? [];

    let options: GateOptionModel[];

    if (p.is_elicitation && choices.length > 0) {
      // Elicitation: one option per scope choice + a Deny. NO generic Allow.
      options = [
        ...choices.map((c, i) => ({
          id: `${c.key}=${c.value}-${i}`,
          testid: `gate-choice-${c.value}`,
          label: c.label,
          response: {
            type: "elicitation" as const,
            key: c.key,
            value: c.value,
          },
        })),
        denyOption("Reject…"),
      ];
    } else if (isAcpSuggestions(suggestions)) {
      // ACP: options ship verbatim (Allow/Deny are among them). NO synth options.
      options = suggestions.map((s, i) => ({
        id: `suggestion-${i}`,
        testid: `approve-suggestion-${i}`,
        label: suggestionLabel(s),
        title: s.destination ? `destination: ${s.destination}` : undefined,
        response: {
          type: "acpOption" as const,
          optionId: (s as Record<string, unknown>).optionId as string,
        },
      }));
    } else {
      // claude (non-elicitation, non-ACP): verbatim suggestions + Allow + Deny.
      options = [
        ...suggestions.map((s, i) => ({
          id: `suggestion-${i}`,
          testid: `approve-suggestion-${i}`,
          label: suggestionLabel(s),
          title: s.destination ? `destination: ${s.destination}` : undefined,
          response: { type: "permissionRule" as const, suggestion: s },
        })),
        {
          id: "allow",
          testid: "approve-btn",
          label: "Approve & run",
          response: { type: "allow" as const },
          tone: "approve" as const,
        },
        denyOption("Reject…"),
      ];
    }

    return {
      kind: "permission",
      title: gv.lead,
      body: p.explanation || undefined,
      risk: p.risk.trim() || undefined,
      attachments,
      mode: "pick-one",
      prompts: [{ id: "permission", multiSelect: false, options }],
    };
  }

  // ---- PLAN ----
  if ("Plan" in raw) {
    const pl = raw.Plan;
    return {
      kind: "plan",
      title: gv.lead,
      body: pl.summary || undefined,
      attachments: [],
      mode: "pick-one",
      prompts: [
        {
          id: "plan",
          multiSelect: false,
          options: [
            {
              id: "allow",
              testid: "approve-btn",
              label: "Approve plan",
              response: { type: "allow" },
              tone: "approve",
            },
            // Rejecting a plan asks the agent to keep planning, not abandon.
            {
              id: "deny",
              testid: "reject-btn",
              label: "Keep planning",
              response: { type: "deny" },
              tone: "reject",
              freeText: DENY_FREE_TEXT,
            },
          ],
        },
      ],
    };
  }

  // ---- EDIT ----
  if ("Edit" in raw) {
    const ed = raw.Edit;
    return {
      kind: "edit",
      title: gv.lead,
      body: gv.lead || undefined,
      attachments: [{ kind: "diff", rows: gv.diff_rows, filename: ed.file }],
      mode: "pick-one",
      prompts: [
        {
          id: "edit",
          multiSelect: false,
          options: [
            {
              id: "allow",
              testid: "approve-btn",
              label: "Approve change",
              response: { type: "allow" },
              tone: "approve",
            },
            denyOption("Reject…"),
          ],
        },
      ],
    };
  }

  // ---- QUESTION ----
  if ("Question" in raw) {
    const q = raw.Question;
    const prompts: GatePromptModel[] = q.questions.map((qp, qi) => ({
      id: String(qi),
      question: qp.question,
      header: qp.header || undefined,
      multiSelect: qp.multi_select,
      options: [
        ...qp.options.map((o, oi) => ({
          id: String(oi),
          label: o.label,
          description: o.description || undefined,
          response: {
            type: "answer" as const,
            question: qp.question,
            label: o.label,
          },
        })),
        {
          id: "other",
          label: "Other…",
          response: {
            type: "answer" as const,
            question: qp.question,
            label: "",
          },
          freeText: { label: "Your answer", placeholder: "Type a custom answer" },
        },
      ],
    }));
    return {
      kind: "question",
      title: gv.lead,
      attachments: [],
      mode: "collect",
      prompts,
      submit: { label: "Submit" },
      skip: {
        id: "skip",
        testid: "question-skip-btn",
        label: "Skip",
        response: { type: "skipQuestions" },
      },
    };
  }

  // ---- HANDOFF (Wasl A2A send-gate) ----
  //
  // A Handoff gate is surfaced on the owning node's SessionView.gate by core
  // when a WaslGateRequested event arrives. It must be answered via
  // answerWaslGate (host-scoped), NOT the normal answerGate (session-scoped).
  // The gate_id is in the response so responseToAnswer can route correctly.
  // The host_id is supplied by the caller (GateOverlay, which has selectedSession.host_id).
  if ("Handoff" in raw) {
    const h = raw.Handoff;
    // Capitalise "consult" → "Consult", "message" → "Message", etc.
    const kindLabel = h.kind.charAt(0).toUpperCase() + h.kind.slice(1);
    return {
      kind: "handoff",
      title: gv.lead,
      body: h.preview || h.summary || undefined,
      risk: `A2A ${kindLabel} gate — approving sends this message to another agent`,
      attachments: [],
      mode: "pick-one",
      prompts: [
        {
          id: "handoff",
          multiSelect: false,
          options: [
            {
              id: "wasl-approve",
              testid: "wasl-approve-btn",
              label: "Approve send",
              response: {
                type: "waslApprove" as const,
                gate_id: h.gate_id,
              },
              tone: "approve" as const,
            },
            {
              id: "wasl-deny",
              testid: "wasl-deny-btn",
              label: "Deny…",
              response: {
                type: "waslDeny" as const,
                gate_id: h.gate_id,
              },
              tone: "reject" as const,
              freeText: {
                label: "Reason for denial (sent to the Wasl operator surface)",
                placeholder: "e.g. Message content not appropriate for this recipient.",
              },
            },
          ],
        },
      ],
    };
  }

  // Exhaustive fallback (should never hit — Gate union is closed).
  return {
    kind: gv.kind,
    title: gv.lead,
    attachments: [],
    mode: "pick-one",
    prompts: [],
  };
}

// ---------------------------------------------------------------------------
// responseToAnswer — the pure mapping from a chosen response to the concrete
// answer-api call. Centralizes what GateOverlay used to inline per-kind.
//
// For "pick-one" responses, `text` carries the optional free-text (a deny
// reason). For the "collect" (questions) submit/skip the caller aggregates the
// per-prompt answers itself and calls `submitAnswers` / this with skipQuestions.
// ---------------------------------------------------------------------------

/** Fire the concrete api call for a single pick-one response. Returns the promise.
 *
 * `host_id` is required only for Wasl send-gate responses (`waslApprove` /
 * `waslDeny`). All other gate kinds ignore it. GateOverlay supplies it from
 * `selectedSession.host_id`, which is always present in the SessionView projection.
 */
export function responseToAnswer(
  session_id: string,
  decision_id: string,
  request_id: string,
  response: GateResponse,
  text?: string,
  host_id?: string
): Promise<void> {
  switch (response.type) {
    case "allow":
      return answerGate({ session_id, decision_id, allow: true });
    case "deny":
      return answerGate({
        session_id,
        decision_id,
        allow: false,
        deny_reason: text?.trim() || undefined,
      });
    case "acpOption":
      // The chosen ACP optionId MUST travel inside `permission_updates` — the
      // agent reads it from `permission_updates.optionId` (ipc.rs), and the wire
      // AnswerGate has no top-level `optionId` field (a top-level one is silently
      // dropped, which left every opencode gate answered with optionId=None).
      return answerGate({
        session_id,
        decision_id,
        allow: true,
        permission_updates: { optionId: response.optionId },
      });
    case "elicitation":
      return answerGate({
        session_id,
        decision_id,
        allow: true,
        elicitation_content: { [response.key]: response.value },
      });
    case "permissionRule":
      return answerGate({
        session_id,
        decision_id,
        allow: true,
        permission_updates: [response.suggestion],
      });
    case "answer":
      // A lone single-select single-question fires immediately with one answer.
      return answerQuestion({
        session_id,
        request_id,
        answers: { [response.question]: response.label },
      });
    case "skipQuestions":
      return answerQuestion({ session_id, request_id, answers: {} });
    // X1/X2: Wasl A2A send-gate — route to answerWaslGate (host-scoped), NOT
    // the normal answerGate (session-scoped). `host_id` is required here; if it
    // is somehow missing (programming error), log and no-op rather than crash.
    case "waslApprove":
      if (!host_id) {
        console.error("[gateModel] waslApprove fired without host_id — BUG");
        return Promise.resolve();
      }
      return answerWaslGate({ host_id, gate_id: response.gate_id, outcome: "approved" });
    case "waslDeny":
      if (!host_id) {
        console.error("[gateModel] waslDeny fired without host_id — BUG");
        return Promise.resolve();
      }
      return answerWaslGate({
        host_id,
        gate_id: response.gate_id,
        outcome: "denied",
        reason: text?.trim() || undefined,
      });
  }
}

/** Submit aggregated question answers (collect mode). */
export function submitAnswers(
  session_id: string,
  request_id: string,
  answers: Record<string, string>
): Promise<void> {
  return answerQuestion({ session_id, request_id, answers });
}
