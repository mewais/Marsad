import type { Command, CommandReply, GateRequest, HostId, InputDropReason, InputId, PhaseCause, RpcError, SessionEvent, SessionSnapshot, TrailItem } from "../wire";
import { conversationFacts } from "./catalog";
import { projectEvent } from "./diagnostics";
import type { SessionEventKind } from "./transcripts";

/** Fixture-only: one scripted session per entry; reacts to commands like an agent would. */

export interface Entry {
  snap: SessionSnapshot;
  events: SessionEvent[];
  subs: Set<(e: unknown) => void>;
  /** Open `session.trail:<id>` subscriptions: each new event's trail entries go to them. */
  trailSubs?: Set<(item: TrailItem) => void>;
  host: HostId;
  /** External gates withdrawn without an answer (a late answer is refused, typed). */
  withdrawn?: Set<string>;
  /**
   * Prompts that wait for the agent, oldest first: held by the protocol until the turn ends (`held` false:
   * journaled queued) or already written and held by the CLI itself (`held`: journaled sent).
   */
  queue?: { input: InputId; text: string; held: boolean }[];
  /** Counts the turns started: a scripted step of a turn that was interrupted or ended does nothing. */
  turn?: number;
}

/** The trace of the request a command arrived with, when it had one. */
const traced = (trace?: string) => (trace ? { trace } : {});

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

  /** Like the real runtime: a command to a session whose process is not running is refused, not journaled. */
  command(e: Entry, c: Command, trace?: string): CommandReply {
    const answers = c.command === "answer_gate" || c.command === "answer_question";
    if (answers && !e.snap.pending_gates.some((x) => x.gate.request_id === c.request_id) && e.withdrawn?.has(c.request_id)) {
      throw err("gate_withdrawn", `the gate ${c.request_id} is no longer needed (withdrawn)`, { error: "gate_withdrawn", request_id: c.request_id });
    }
    if (!["starting", "idle", "working", "needs_you"].includes(e.snap.phase.state)) {
      const reason = `the session is ${e.snap.phase.state}`;
      throw err("not_running", `the session is not running: ${reason}`, { error: "not_running", reason });
    }
    if (c.command === "shutdown") throw err("rejected", "rejected: use sessions.stop to end a session", { error: "rejected", reason: "use sessions.stop to end a session" });
    switch (c.command) {
      case "prompt":
      case "run_slash": {
        const text = c.command === "prompt" ? c.text : `/${c.name} ${c.args}`.trim();
        const input = this.nextInput(e);
        const phase = e.snap.phase.state;
        // What a prompt does while the agent is busy is the driver's capability, as in the daemon.
        const busy = e.snap.capabilities.prompt_while_busy;
        if (phase === "idle") {
          this.emit(e, { kind: "input", input, command: c, status: { status: "sent" }, ...traced(trace) });
          this.startTurn(e, text, input);
          return { outcome: "sent" };
        }
        if (phase === "starting" || busy === "queued_by_protocol") {
          // Held by the protocol: journaled queued, `input_sent` when it is written.
          this.emit(e, { kind: "input", input, command: c, status: { status: "queued" }, ...traced(trace) });
          (e.queue ??= []).push({ input, text, held: false });
          this.h.publish(e);
          return { outcome: "queued" };
        }
        this.emit(e, { kind: "input", input, command: c, status: { status: "sent" }, ...traced(trace) });
        if (busy === "steered") this.emit(e, { kind: "agent", event: { type: "input_acknowledged", input, steered: true } });
        else (e.queue ??= []).push({ input, text, held: true }); // written; the CLI runs it after the current turn
        this.h.publish(e);
        return { outcome: "sent" };
      }
      case "answer_gate":
      case "answer_question": {
        const g = e.snap.pending_gates.find((x) => x.gate.request_id === c.request_id);
        if (!g) throw err("rejected", `Gate ${c.request_id} is not pending.`, { error: "rejected", reason: "gate not pending" });
        const picked = c.command === "answer_gate" ? g.gate.gate_type !== "question" && g.gate.choices.find((x) => x.id === c.decision.choice) : undefined;
        if (c.command === "answer_gate" && !picked) throw err("rejected", `Gate ${c.request_id} offers no choice ${c.decision.choice}.`, { error: "rejected", reason: "no such choice" });
        const denied = !!picked && !picked.kind.startsWith("allow");
        this.journal(e, c, trace);
        this.resolveGate(e, c.request_id, denied ? "Understood, I will not do that." : "Thanks, continuing.");
        return { outcome: "sent" };
      }
      case "interrupt":
        this.journal(e, c, trace);
        this.dropQueued(e, "interrupted");
        e.turn = (e.turn ?? 0) + 1;
        e.snap = { ...e.snap, pending_gates: [], turn: null };
        this.setPhase(e, { state: "idle" });
        return { outcome: "sent" };
      case "set_model":
        this.journal(e, c, trace);
        this.emit(e, { kind: "agent", event: { type: "model_changed", model: c.model } });
        this.patch(e, { model: c.model });
        return { outcome: "sent" };
      case "set_mode":
        this.journal(e, c, trace);
        this.emit(e, { kind: "agent", event: { type: "modes_updated", current_mode: c.mode } });
        this.patch(e, { mode: c.mode });
        return { outcome: "sent" };
    }
  }

  /** The id the next journaled input gets: the journal seq of its `input` event, like the daemon's. */
  private nextInput(e: Entry): InputId {
    return Math.floor((e.events[e.events.length - 1]?.cursor ?? 0) / 65536) + 1;
  }

  /** Journal a command the agent took at once. */
  private journal(e: Entry, c: Command, trace?: string) {
    this.emit(e, { kind: "input", input: this.nextInput(e), command: c, status: { status: "sent" }, ...traced(trace) });
  }

  /** The first prompt of a session, journaled with its start like the daemon does; returns its id. */
  firstPrompt(e: Entry, text: string, trace?: string): InputId {
    const input = this.nextInput(e);
    this.emit(e, { kind: "started", generation: 1, resume: "fresh", first_input: input, ...traced(trace) });
    this.emit(e, { kind: "input", input, command: { command: "prompt", text }, status: { status: "sent" }, ...traced(trace) });
    return input;
  }

  /** The prompts still waiting will never run: say so for each. `process_ended` is how the runtime reports the ones already written. */
  dropQueued(e: Entry, reason: InputDropReason) {
    for (const q of e.queue ?? []) this.emit(e, { kind: "agent", event: { type: "input_dropped", input: q.input, reason: q.held && reason === "shutdown" ? "process_ended" : reason } });
    e.queue = [];
  }

  /** The agent is ready for its next prompt: the oldest waiting one starts a turn. */
  private drain(e: Entry) {
    const next = e.queue?.shift();
    if (!next) return;
    // The protocol writes its queued prompt now; the CLI says it took the one it was holding.
    this.emit(e, { kind: "agent", event: next.held ? { type: "input_acknowledged", input: next.input } : { type: "input_sent", input: next.input } });
    this.startTurn(e, next.text, next.input);
  }

  /** The agent is idle (its handshake is done, or its turn ended). */
  ready(e: Entry) {
    this.setPhase(e, { state: "idle" }, { kind: "handshake_done" });
    this.drain(e);
  }

  /** The process ends (stop requested): what still waits in the queue is dropped. */
  end(e: Entry) {
    this.dropQueued(e, "shutdown");
    e.turn = (e.turn ?? 0) + 1;
    this.setPhase(e, { state: "exited", code: 0 }, { kind: "exited", requested: true, handshake_timeout: false });
  }

  startTurn(e: Entry, prompt: string, input?: InputId) {
    const now = this.h.now();
    const turn = (e.turn = (e.turn ?? 0) + 1);
    const live = (fn: () => void) => () => e.turn === turn && fn();
    e.snap = { ...e.snap, turn: { started_at_ms: now, prompt, tool_calls: 0 }, last_activity_ms: now };
    this.setPhase(e, { state: "working" }, { kind: "turn_started", input });
    this.emit(e, { kind: "agent", event: { type: "turn_started", input } });
    this.h.after(450, live(() => this.emit(e, { kind: "agent", event: { type: "assistant_delta", text: `On it: ${prompt.slice(0, 80)}`, message_id: `r${now}` } })));
    this.h.after(900, live(() => this.fakeTool(e, "Read", { file_path: "README.md" }, true, "# Project\n")));
    this.h.after(1700, live(() => this.finishTurn(e, `Done: ${prompt.slice(0, 80)}. Nothing else needs your attention.`)));
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
    this.setPhase(e, { state: "idle" }, { kind: "turn_completed", interrupted: false });
    this.drain(e);
  }

  resolveGate(e: Entry, requestId: string, reply: string) {
    const gates = e.snap.pending_gates.filter((g) => g.gate.request_id !== requestId);
    this.emit(e, { kind: "agent", event: { type: "gate_resolved", request_id: requestId } });
    e.snap = { ...e.snap, pending_gates: gates, last_activity_ms: this.h.now() };
    if (gates.length) return this.h.publish(e);
    this.setPhase(e, { state: "working" });
    const turn = e.turn;
    this.h.after(1200, () => e.turn === turn && this.finishTurn(e, reply));
  }

  /** The request behind an external gate went away: the gate leaves without an answer. */
  withdrawGate(e: Entry, requestId: string) {
    (e.withdrawn ??= new Set()).add(requestId);
    const gates = e.snap.pending_gates.filter((g) => g.gate.request_id !== requestId);
    this.emit(e, { kind: "agent", event: { type: "gate_resolved", request_id: requestId, reason: "withdrawn" } });
    e.snap = { ...e.snap, pending_gates: gates };
    if (gates.length) return this.h.publish(e);
    this.setPhase(e, { state: e.snap.turn ? "working" : "idle" });
  }

  raiseGate(e: Entry, gate: GateRequest) {
    this.emit(e, { kind: "agent", event: { type: "gate_requested", gate } });
    this.setPhase(e, { state: "needs_you" });
  }

  patch(e: Entry, p: Partial<SessionSnapshot>) {
    const next = { ...e.snap, ...p, cursor: e.events[e.events.length - 1]?.cursor ?? e.snap.cursor };
    e.snap = { ...next, ...conversationFacts(next) };
    this.h.publish(e);
  }

  setPhase(e: Entry, phase: SessionSnapshot["phase"], cause: PhaseCause = { kind: "other" }) {
    this.emit(e, { kind: "phase", phase, cause });
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
    if (e.trailSubs?.size) for (const item of projectEvent(e.events, e.events.length - 1)) e.trailSubs.forEach((s) => s(item));
  }
}
