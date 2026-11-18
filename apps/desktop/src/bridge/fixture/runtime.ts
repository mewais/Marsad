import type { Command, CommandReply, GateRequest, HostId, RpcError, SessionEvent, SessionSnapshot } from "../wire";
import type { SessionEventKind } from "./transcripts";

/** Fixture-only: one scripted session per entry; reacts to commands like an agent would. */

export interface Entry {
  snap: SessionSnapshot;
  events: SessionEvent[];
  subs: Set<(e: unknown) => void>;
  host: HostId;
}

export const err = (code: string, message: string, data?: unknown): RpcError => ({ code, message, data: (data ?? null) as RpcError["data"] });

export interface SimHooks {
  now(): number;
  /** Schedule after `ms` scaled by the fixture's speed. */
  after(ms: number, fn: () => void): void;
  /** A snapshot changed: tell fleet subscribers. */
  publish(e: Entry): void;
}

export class SessionSim {
  constructor(private h: SimHooks) {}

  command(e: Entry, c: Command): CommandReply {
    const live = ["starting", "idle", "working", "needs_you"].includes(e.snap.phase.state);
    if (!live && c.command !== "prompt") throw err("not_running", "The session is not running.", { error: "not_running", reason: e.snap.phase.state });
    this.emit(e, { kind: "input", command: c, status: { status: "sent" } });
    switch (c.command) {
      case "prompt":
        this.startTurn(e, c.text);
        return { outcome: e.snap.phase.state === "working" ? "queued" : "sent" };
      case "run_slash":
        this.startTurn(e, `/${c.name} ${c.args}`.trim());
        return { outcome: "sent" };
      case "answer_gate":
      case "answer_question": {
        const g = e.snap.pending_gates.find((x) => x.gate.request_id === c.request_id);
        if (!g) throw err("rejected", `Gate ${c.request_id} is not pending.`, { error: "rejected", reason: "gate not pending" });
        const picked = c.command === "answer_gate" ? g.gate.gate_type !== "question" && g.gate.choices.find((x) => x.id === c.decision.choice) : undefined;
        if (c.command === "answer_gate" && !picked) throw err("rejected", `Gate ${c.request_id} offers no choice ${c.decision.choice}.`, { error: "rejected", reason: "no such choice" });
        const denied = !!picked && !picked.kind.startsWith("allow");
        this.resolveGate(e, c.request_id, denied ? "Understood, I will not do that." : "Thanks, continuing.");
        return { outcome: "sent" };
      }
      case "interrupt":
        e.snap = { ...e.snap, pending_gates: [], turn: null };
        this.setPhase(e, { state: "idle" });
        return { outcome: "sent" };
      case "set_model":
        this.emit(e, { kind: "agent", event: { type: "model_changed", model: c.model } });
        this.patch(e, { model: c.model });
        return { outcome: "sent" };
      case "set_mode":
        this.emit(e, { kind: "agent", event: { type: "modes_updated", current_mode: c.mode } });
        this.patch(e, { mode: c.mode });
        return { outcome: "sent" };
      case "shutdown":
        this.setPhase(e, { state: "exited", code: 0 });
        return { outcome: "sent" };
    }
  }

  startTurn(e: Entry, prompt: string) {
    const now = this.h.now();
    e.snap = { ...e.snap, turn: { started_at_ms: now, prompt, tool_calls: 0 }, last_activity_ms: now };
    this.setPhase(e, { state: "working" });
    this.emit(e, { kind: "agent", event: { type: "turn_started" } });
    this.h.after(450, () => this.emit(e, { kind: "agent", event: { type: "assistant_delta", text: `On it: ${prompt.slice(0, 80)}`, message_id: `r${now}` } }));
    this.h.after(900, () => this.fakeTool(e, "Read", { file_path: "README.md" }, true, "# Project\n"));
    this.h.after(1700, () => this.finishTurn(e, `Done: ${prompt.slice(0, 80)}. Nothing else needs your attention.`));
  }

  fakeTool(e: Entry, name: string, input: unknown, ok: boolean, result: unknown) {
    const id = `t${this.h.now()}${Math.floor(Math.random() * 1e4)}`;
    this.emit(e, { kind: "agent", event: { type: "tool_call_started", name, call_id: id, input: input as never, is_subagent: false } });
    this.emit(e, { kind: "agent", event: { type: "tool_call_finished", name, call_id: id, ok, result: result as never } });
    e.snap = { ...e.snap, turn: e.snap.turn ? { ...e.snap.turn, tool_calls: e.snap.turn.tool_calls + 1, last_tool: name } : e.snap.turn };
    this.h.publish(e);
  }

  finishTurn(e: Entry, reply: string) {
    const now = this.h.now();
    const usage = { ...e.snap.usage, cost_usd: +(e.snap.usage.cost_usd + 0.06).toFixed(2), output_tokens: e.snap.usage.output_tokens + 900 };
    this.emit(e, { kind: "agent", event: { type: "assistant_delta", text: reply, message_id: `d${now}` } });
    this.emit(e, { kind: "agent", event: { type: "turn_completed", was_interrupted: false, num_turns: 1, usage, reply } });
    e.snap = { ...e.snap, usage, turn: null, turns_completed: e.snap.turns_completed + 1, last_turn: { ended_at_ms: now, was_interrupted: false, num_turns: 1, reply }, last_activity_ms: now };
    this.setPhase(e, { state: "idle" });
  }

  resolveGate(e: Entry, requestId: string, reply: string) {
    const gates = e.snap.pending_gates.filter((g) => g.gate.request_id !== requestId);
    this.emit(e, { kind: "agent", event: { type: "gate_resolved", request_id: requestId } });
    e.snap = { ...e.snap, pending_gates: gates, last_activity_ms: this.h.now() };
    if (gates.length) return this.h.publish(e);
    this.setPhase(e, { state: "working" });
    this.h.after(1200, () => this.finishTurn(e, reply));
  }

  raiseGate(e: Entry, gate: GateRequest) {
    this.emit(e, { kind: "agent", event: { type: "gate_requested", gate } });
    this.setPhase(e, { state: "needs_you" });
  }

  patch(e: Entry, p: Partial<SessionSnapshot>) {
    e.snap = { ...e.snap, ...p, cursor: e.events[e.events.length - 1]?.cursor ?? e.snap.cursor };
    this.h.publish(e);
  }

  setPhase(e: Entry, phase: SessionSnapshot["phase"]) {
    this.emit(e, { kind: "phase", phase });
    this.patch(e, { phase, last_activity_ms: this.h.now() });
  }

  emit(e: Entry, kind: SessionEventKind) {
    const last = e.events[e.events.length - 1]?.cursor ?? 0;
    const ev = { cursor: (Math.floor(last / 65536) + 1) * 65536, t: this.h.now(), ...kind } as SessionEvent;
    e.events.push(ev);
    e.snap = { ...e.snap, cursor: ev.cursor };
    if (kind.kind === "agent" && kind.event.type === "gate_requested") {
      e.snap = { ...e.snap, pending_gates: [...e.snap.pending_gates, { gate: kind.event.gate, raised_at_ms: ev.t, cursor: ev.cursor }] };
    }
    e.subs.forEach((s) => s(ev));
  }
}
