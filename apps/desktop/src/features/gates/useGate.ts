import { useEffect, useMemo, useRef, useState } from "react";
import type { GateRequest, PendingGate, QuestionAnswer, QuestionPrompt } from "../../bridge/wire";
import { gatePendingKey, useActions, useApp, usePending, useSessionReach, type SessionView } from "../../state";
import { toast } from "../../ui/toast";
import { gateChoices, type GateChoice } from "./choices";

/** What a gate asked for, in a few words (for "no longer needed"). */
export function gateGist(g: GateRequest): string {
  switch (g.gate_type) {
    case "spawn":
      return `${g.from} no longer waits to start ${g.name}.`;
    case "handoff":
      return `The handoff from ${g.from} to ${g.to} was withdrawn.`;
    case "question":
      return "The agent no longer waits for this answer.";
    default:
      return "The agent no longer waits for this decision.";
  }
}

/**
 * A gate the user has open (an expanded Inbox row, a session or inspector card) can disappear
 * without an answer from here: its request was withdrawn (the asking turn was interrupted, the
 * task cancelled, the session ended) or it was answered elsewhere. Say so instead of letting the
 * card vanish silently. Unmounting for any other reason (navigation, a collapsed row, our own
 * answer) says nothing.
 */
export function useGoneNotice(view: SessionView, pg: PendingGate, open: boolean) {
  const { store } = useApp();
  const rid = pg.gate.request_id;
  const answered = usePending(gatePendingKey(view.key, rid));
  const st = useRef({ open, answered: false, gate: pg.gate });
  st.current.open = open;
  st.current.gate = pg.gate;
  if (answered) st.current.answered = true;
  useEffect(
    () => () => {
      const { open: wasOpen, answered: ours, gate } = st.current;
      if (!wasOpen || ours) return;
      const snap = store.get().sessions[view.key];
      if (snap?.pending_gates.some((g) => g.gate.request_id === rid)) return;
      toast({ tone: "info", title: "No longer needed", detail: gateGist(gate) });
    },
    [store, view.key, rid],
  );
}

export function useGateCommon(view: SessionView, pg: PendingGate) {
  const actions = useActions();
  const pending = usePending(gatePendingKey(view.key, pg.gate.request_id));
  const blocked = useSessionReach(view);
  return { actions, pending, busy: !!pending, blocked };
}

/** Permission / plan / handoff / spawn gates: the gate's own choices, with the text flow for choices that take text. */
export function usePermissionGate(view: SessionView, pg: PendingGate & { gate: Extract<GateRequest, { gate_type: "permission" | "plan" | "handoff" | "spawn" }> }) {
  const c = useGateCommon(view, pg);
  const set = useMemo(() => gateChoices(pg.gate), [pg.gate]);
  const [reason, setReason] = useState("");
  const [armed, setArmed] = useState(false);
  const choose = (choice: GateChoice, focusReason?: () => void) => {
    if (c.busy || c.blocked) return;
    if (choice.text === "required" && !reason.trim()) {
      setArmed(true);
      focusReason?.();
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

  const answerFor = (qi: number): string[] => {
    const labels = [...(st.picked[qi] ?? [])];
    if (st.otherOn[qi] && st.other[qi]?.trim()) labels.push(st.other[qi].trim());
    return labels;
  };
  const complete = qs.every((_, i) => answerFor(i).length > 0);
  const send = () => {
    if (!complete || c.busy || c.blocked) return;
    const answer: QuestionAnswer = { answers: Object.fromEntries(qs.map((q: QuestionPrompt, i) => [q.id, answerFor(i)])) };
    void c.actions.answerQuestion(view.key, pg.gate.request_id, answer);
  };
  return { ...c, qs, st, toggle, pickOther, setOther, complete, send };
}
