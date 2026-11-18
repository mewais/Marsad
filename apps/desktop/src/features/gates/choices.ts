import type { GateDecision, GateRequest, JsonValue } from "../../bridge/wire";

/**
 * One place that turns a gate into the buttons the human sees. The agent's own wording always wins:
 *   1. ACP-style options in `permission_suggestions` ({optionId, name, kind}),
 *   2. `elicitation_choices`,
 *   3. only then Marsad's generic Allow / Deny, flagged `source: "marsad"` so the UI says so. When the
 *      agent sent "always allow" suggestions (claude rule objects), one extra choice echoes them back
 *      verbatim as `permission_updates`, labelled with the agent's own rule text.
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
void 0;

interface AcpOption {
  optionId: string;
  name: string;
  kind?: string;
}
/** The agent's own rule text inside a claude suggestion object (`rules[].ruleContent`), verbatim. */
function ruleTexts(o: unknown): string[] {
  const rules = (o as { rules?: { toolName?: string; ruleContent?: string }[] } | null)?.rules;
  return Array.isArray(rules) ? rules.map((r) => (r.ruleContent ? `${r.toolName ?? ""}(${r.ruleContent})` : r.toolName ?? "")).filter(Boolean) : [];
}

const isAcp = (o: unknown): o is AcpOption => !!o && typeof o === "object" && "optionId" in o && "name" in o;

export function permissionChoices(g: Extract<GateRequest, { gate_type: "permission" | "plan" }>): ChoiceSet {
  if (g.gate_type === "permission") {
    const acp = ((g.permission_suggestions ?? []) as unknown[]).filter(isAcp);
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
  const suggestions = g.gate_type === "permission" ? (g.permission_suggestions ?? []) : [];
  const rules = suggestions.flatMap(ruleTexts);
  return {
    source: "marsad",
    choices: [
      { id: "allow", label: [{ text: plan ? "Approve plan" : "Allow" }], kind: "allow", asksReason: false, decision: () => approve() },
      ...(suggestions.length
        ? [{ id: "always", label: [{ text: "Always allow " }, ...(rules.length ? rules.flatMap((r, i) => [...(i ? [{ text: ", " }] : []), { text: r, code: true }]) : [{ text: "as suggested" }])], kind: "always" as const, asksReason: false, decision: () => approve({ permission_updates: suggestions as JsonValue }) }]
        : []),
      { id: "deny", label: [{ text: plan ? "Keep planning" : "Deny" }], kind: "deny", asksReason: true, decision: deny },
    ],
  };
}

export const HANDOFF_CHOICES: { id: "allow" | "always" | "deny"; label: string; kind: GateChoice["kind"] }[] = [
  { id: "allow", label: "Allow once", kind: "allow" },
  { id: "always", label: "Always allow on this connection", kind: "always" },
  { id: "deny", label: "Deny", kind: "deny" },
];
