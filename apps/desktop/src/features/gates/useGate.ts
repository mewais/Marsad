import { useMemo, useState } from "react";
import type { GateRequest, PendingGate, QuestionAnswer, QuestionPrompt } from "../../bridge/wire";
import { gatePendingKey, useActions, usePending, type SessionView } from "../../state";
import { permissionChoices, type GateChoice } from "./choices";

export function useGateCommon(view: SessionView, pg: PendingGate) {
  const actions = useActions();
  const pending = usePending(gatePendingKey(view.key, pg.gate.request_id));
  return { actions, pending, busy: !!pending };
}

/** Permission / plan gates: the agent's choices, with the "tell it what to do differently" reason flow. */
export function usePermissionGate(view: SessionView, pg: PendingGate & { gate: Extract<GateRequest, { gate_type: "permission" | "plan" }> }) {
  const c = useGateCommon(view, pg);
  const set = useMemo(() => permissionChoices(pg.gate), [pg.gate]);
  const [reason, setReason] = useState("");
  const [armed, setArmed] = useState(false);
  const choose = (choice: GateChoice, focusReason?: () => void) => {
    if (c.busy) return;
    if (choice.asksReason && !reason.trim() && !armed && focusReason) {
      setArmed(true);
      focusReason();
      return;
    }
    void c.actions.answerGate(view.key, pg.gate.request_id, choice.decision(reason.trim() || undefined));
  };
  const first = (kind: GateChoice["kind"]) => set.choices.find((x) => x.kind === kind);
  return { ...c, set, reason, setReason, armed, choose, first };
}

export interface QState {
  /** Selected option labels per question index. */
  picked: Record<number, string[]>;
  other: Record<number, string>;
  /** Question index whose "Other" is chosen. */
  otherOn: Record<number, boolean>;
}

export function useQuestionGate(view: SessionView, pg: PendingGate & { gate: Extract<GateRequest, { gate_type: "question" }> }) {
  const c = useGateCommon(view, pg);
  const qs = pg.gate.questions;
  const [st, setSt] = useState<QState>({ picked: {}, other: {}, otherOn: {} });
  const toggle = (qi: number, label: string) =>
    setSt((s) => {
      const q = qs[qi];
      const cur = s.picked[qi] ?? [];
      if (q.multi_select) return { ...s, picked: { ...s.picked, [qi]: cur.includes(label) ? cur.filter((x) => x !== label) : [...cur, label] } };
      return { ...s, picked: { ...s.picked, [qi]: [label] }, otherOn: { ...s.otherOn, [qi]: false } };
    });
  const pickOther = (qi: number) =>
    setSt((s) => ({ ...s, otherOn: { ...s.otherOn, [qi]: true }, picked: qs[qi].multi_select ? s.picked : { ...s.picked, [qi]: [] } }));
  const setOther = (qi: number, text: string) => setSt((s) => ({ ...s, other: { ...s.other, [qi]: text }, otherOn: { ...s.otherOn, [qi]: true }, picked: qs[qi].multi_select ? s.picked : { ...s.picked, [qi]: [] } }));

  const answerFor = (qi: number): string => {
    const labels = [...(st.picked[qi] ?? [])];
    if (st.otherOn[qi] && st.other[qi]?.trim()) labels.push(st.other[qi].trim());
    return labels.join(", ");
  };
  const complete = qs.every((_, i) => answerFor(i) !== "");
  const send = () => {
    if (!complete || c.busy) return;
    const answer: QuestionAnswer = { answers: Object.fromEntries(qs.map((q: QuestionPrompt, i) => [q.question, answerFor(i)])) };
    void c.actions.answerQuestion(view.key, pg.gate.request_id, answer);
  };
  return { ...c, qs, st, toggle, pickOther, setOther, complete, send };
}
