import type { GateDecision, GateRequest, PermissionChoice } from "../../bridge/types";

/**
 * One place that turns a gate into the buttons the human sees. The agent's own wording always wins:
 *   1. `choices` carried by the gate (provisional wire field),
 *   2. ACP-style options in `permission_suggestions` ({optionId, name, kind}),
 *   3. `elicitation_choices`,
 *   4. only then Marsad's generic Allow / Deny, flagged `source: "marsad"` so the UI says so.
 * Handoffs are Marsad's own gate and always offer the same three choices.
 */
export type GateKind = "permission" | "plan" | "question" | "handoff";

export interface LabelPart {
  text: string;
  code?: boolean;
}

export interface GateChoice {
  id: string;
  /** Label split into text and `code` runs (the agent wraps commands in backticks). */
  label: LabelPart[];
  kind: "allow" | "always" | "deny";
  asksReason: boolean;
  decision: (reason?: string) => GateDecision;
}

export interface ChoiceSet {
  choices: GateChoice[];
  source: "agent" | "marsad";
}

export function labelParts(label: string): LabelPart[] {
  return label.split(/(`[^`]+`)/g).filter(Boolean).map((t) => (t.startsWith("`") ? { text: t.slice(1, -1), code: true } : { text: t }));
}
export const labelText = (parts: LabelPart[]) => parts.map((p) => p.text).join("");

const approve = (extra: Partial<Extract<GateDecision, { action: "approve" }>> = {}): GateDecision => ({ action: "approve", ...extra });
const deny = (reason?: string): GateDecision => (reason ? { action: "deny", reason } : { action: "deny" });

function fromChoices(g: Extract<GateRequest, { gate_type: "permission" | "plan" }>, cs: PermissionChoice[]): GateChoice[] {
  const suggestions = g.gate_type === "permission" ? g.permission_suggestions : undefined;
  return cs.map((c) => ({
    id: c.id,
    label: labelParts(c.label),
    kind: c.kind,
    asksReason: !!c.asks_reason,
    decision: (reason) => (c.kind === "deny" ? deny(reason) : c.kind === "always" ? approve({ permission_updates: suggestions?.length ? suggestions : true }) : approve()),
  }));
}

interface AcpOption {
  optionId: string;
  name: string;
  kind?: string;
}
const isAcp = (o: unknown): o is AcpOption => !!o && typeof o === "object" && "optionId" in o && "name" in o;

export function permissionChoices(g: Extract<GateRequest, { gate_type: "permission" | "plan" }>): ChoiceSet {
  if (g.choices?.length) return { choices: fromChoices(g, g.choices), source: "agent" };
  if (g.gate_type === "permission") {
    const acp = (g.permission_suggestions ?? []).filter(isAcp);
    if (acp.length) {
      return {
        source: "agent",
        choices: acp.map((o) => {
          const kind: GateChoice["kind"] = /reject|deny/i.test(o.kind ?? o.optionId) ? "deny" : /always|session/i.test(o.kind ?? o.optionId) ? "always" : "allow";
          return {
            id: o.optionId,
            label: labelParts(o.name),
            kind,
            asksReason: false,
            decision: (reason) => (kind === "deny" ? deny(reason) : approve({ elicitation_content: { optionId: o.optionId } })),
          };
        }),
      };
    }
    if (g.is_elicitation && g.elicitation_choices?.length) {
      return {
        source: "agent",
        choices: [
          ...g.elicitation_choices.map<GateChoice>((c) => ({
            id: `${c.key}:${c.value}`,
            label: labelParts(c.label),
            kind: "allow",
            asksReason: false,
            decision: () => approve({ elicitation_content: { [c.key]: c.value } }),
          })),
          { id: "decline", label: [{ text: "Decline" }], kind: "deny", asksReason: false, decision: () => deny() },
        ],
      };
    }
  }
  const plan = g.gate_type === "plan";
  return {
    source: "marsad",
    choices: [
      { id: "allow", label: [{ text: plan ? "Approve plan" : "Allow" }], kind: "allow", asksReason: false, decision: () => approve() },
      { id: "deny", label: [{ text: plan ? "Keep planning" : "Deny" }], kind: "deny", asksReason: true, decision: deny },
    ],
  };
}

export const HANDOFF_CHOICES: { id: "allow" | "always" | "deny"; label: string; kind: GateChoice["kind"] }[] = [
  { id: "allow", label: "Allow once", kind: "allow" },
  { id: "always", label: "Always allow on this connection", kind: "always" },
  { id: "deny", label: "Deny", kind: "deny" },
];
