import type { ChoiceText, GateChoice as WireChoice, GateDecision, GateRequest } from "../../bridge/wire";

/**
 * One place that turns a gate's choices into the buttons the human sees. The choices are the
 * agent's own (Wasit `GateChoice`): its ids, its kinds, and its wording whenever its protocol has
 * wording (ACP option names, MCP enum names, Marsad's handoff wording). When the protocol has no
 * wording (claude `can_use_tool`, codex approvals) the driver supplies documented neutral labels and
 * marks them `neutral_label`; the UI says so instead of pretending they are the agent's words.
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
  /** What the choice does beyond its label (e.g. the rules it remembers). */
  description: string;
  /** Free text sent to the agent with this choice. */
  text: ChoiceText;
  decision: (text?: string) => GateDecision;
}

export interface ChoiceSet {
  choices: GateChoice[];
  /** `agent`: the labels are the agent's (or the orchestration layer's) own words; `neutral`: the protocol has no wording. */
  source: "agent" | "neutral" | "handoff";
}

export function labelParts(label: string): LabelPart[] {
  return label.split(/(`[^`]+`)/g).filter(Boolean).map((t) => (t.startsWith("`") ? { text: t.slice(1, -1), code: true } : { text: t }));
}
export const labelText = (parts: LabelPart[]) => parts.map((p) => p.text).join("");

const variant = (k: WireChoice["kind"]): GateChoice["kind"] => (k === "allow_once" ? "allow" : k === "allow_always" ? "always" : "deny");

export function toChoice(c: WireChoice): GateChoice {
  const text = c.text ?? "none";
  return {
    id: c.id,
    label: labelParts(c.label),
    kind: variant(c.kind),
    description: c.description ?? "",
    text,
    decision: (t) => (text !== "none" && t?.trim() ? { choice: c.id, text: t.trim() } : { choice: c.id }),
  };
}

export function gateChoices(g: Extract<GateRequest, { gate_type: "permission" | "plan" | "handoff" }>): ChoiceSet {
  const choices = g.choices.map(toChoice);
  if (g.gate_type === "handoff") return { choices, source: "handoff" };
  return { choices, source: g.choices.length && g.choices.every((c) => c.neutral_label) ? "neutral" : "agent" };
}

/** The first choice of one of these wire kinds (tried in order). */
export function firstOfKind(g: { choices: WireChoice[] }, kinds: WireChoice["kind"][]): WireChoice | undefined {
  for (const k of kinds) {
    const c = g.choices.find((x) => x.kind === k);
    if (c) return c;
  }
  return undefined;
}
