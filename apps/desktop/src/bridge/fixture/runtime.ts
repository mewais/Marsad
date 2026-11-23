import type { AgentEvent, Command, CommandReply, GateReason, GateRequest, HostId, InputDropReason, InputId, NotContinuable, PhaseCause, RpcError, Scope, SessionEvent, SessionSnapshot, TrailItem, TurnOrigin } from "../wire";
import { conversationFacts } from "./catalog";
import { projectEvent } from "./diagnostics";
import type { SessionEventKind } from "./transcripts";
import { endAllWork, reduceWork } from "./work";

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
  /** The next start of a process for this session fails with this reason (a test sets it; it clears itself). */
  continueFails?: string;
}

/** A gate raised by Wasl (spawn, handoff): taken down with a reason when its request goes away. The CLI's own gates are not. */
const isExternal = (g: GateRequest) => g.gate_type === "spawn" || g.gate_type === "handoff";

/** The trace of the request a command arrived with, when it had one. */
const traced = (trace?: string) => (trace ? { trace } : {});

export const err = (code: string, message: string, data?: unknown): RpcError => ({ code, message, data: (data ?? null) as RpcError["data"] });

export interface SimHooks {
  now(): number;
  /** Schedule after `ms` scaled by the fixture's speed. */
  after(ms: number, fn: () => void): void;
  /** A snapshot changed: tell fleet subscribers. */
  publish(e: Entry): void;
  /** An external gate left its session without an answer (the hub's views of the request follow). */
  gateEnded?(e: Entry, gate: GateRequest, reason: GateReason): void;
}

/** The runtime's words for why a continue was refused (`begin_continue`), as the error a `sessions.continue` carries. */
const CONTINUE_REFUSED: Record<NotContinuable, { code: string; error: string; message: string }> = {
  not_ended: { code: "invalid_state", error: "invalid_state", message: "the session is still running" },
  loading: { code: "invalid_state", error: "invalid_state", message: "the session is still loading" },
  agent_cannot_resume: { code: "unsupported", error: "unsupported", message: "this agent cannot resume sessions" },
  no_conversation_id: { code: "invalid_state", error: "invalid_state", message: "the CLI never reported a native session id; nothing to resume" },
  nothing_saved: { code: "invalid_state", error: "invalid_state", message: "the agent has not saved this conversation yet (it saves it with its first message), so there is nothing to resume" },
};

export class SessionSim {
  constructor(private h: SimHooks) {}

  /** Like the real runtime: a command to a session whose process is not running is refused, not journaled. */
  command(e: Entry, c: Command, trace?: string): CommandReply {
    const answers = c.command === "answer_gate" || c.command === "answer_question";
    if (answers && !e.snap.pending_gates.some((x) => x.gate.request_id === c.request_id) && e.withdrawn?.has(c.request_id)) {
      throw err("gate_withdrawn", `the gate ${c.request_id} is no longer needed (withdrawn)`, { error: "gate_withdrawn", request_id: c.request_id });
    }
    if (c.command === "shutdown") throw err("rejected", "rejected: use sessions.stop to end a session", { error: "rejected", reason: "use sessions.stop to end a session" });
    if (!this.running(e)) {
      // Like the daemon: a prompt (or slash command) continues the session first and is then delivered, in that order; any other command to an ended session is refused.
      if (c.command !== "prompt" && c.command !== "run_slash") {
        const reason = `the session is ${e.snap.phase.state}`;
        throw err("not_running", `the session is not running: ${reason}`, { error: "not_running", reason });
      }
      this.continueForCommand(e);
    }
    switch (c.command) {
      case "prompt":
      case "run_slash": {
        const text = c.command === "prompt" ? c.text : `/${c.name} ${c.args}`.trim();
        const input = this.nextInput(e);
        const phase = e.snap.phase.state;
        // What a prompt does while the agent is busy is the driver's capability, as in the daemon. Background work alone (no turn open) is not busy: the prompt starts a turn.
        const busy = e.snap.capabilities.prompt_while_busy;
        if (phase === "idle" || (phase === "working" && !e.snap.turn)) {
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
      case "interrupt": {
        this.journal(e, c, trace);
        this.dropQueued(e, "interrupted");
        this.declineGates(e, { code: "turn_interrupted" });
        // The daemon stops the running turn and leaves background work alone; with no turn open it stops the background tasks.
        const turnOpen = !!e.snap.turn || e.snap.pending_gates.length > 0 || ["starting", "needs_you"].includes(e.snap.phase.state);
        if (turnOpen) {
          e.turn = (e.turn ?? 0) + 1;
          e.snap = { ...e.snap, turn: null };
          this.settle(e, { kind: "turn_completed", interrupted: true }, true);
        } else for (const t of e.snap.tasks.filter((x) => x.running && x.background)) this.stopTask(e, t.task_id);
        return { outcome: "sent" };
      }
      case "stop_task":
        this.journal(e, c, trace);
        this.stopTask(e, c.task_id);
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

  /** A process is running (or starting) for the session. */
  private running(e: Entry): boolean {
    return ["starting", "idle", "working", "needs_you"].includes(e.snap.phase.state);
  }

  /**
   * The gates still waiting leave without an answer: the CLI's own are declined by the protocol (no reason), the external ones
   * are taken down with `reason`; the hub is told about the external ones.
   */
  private declineGates(e: Entry, reason: GateReason) {
    const gates = e.snap.pending_gates.map((g) => g.gate);
    for (const g of gates) this.emit(e, { kind: "agent", event: { type: "gate_resolved", request_id: g.request_id, ...(isExternal(g) ? { reason } : {}) } });
    e.snap = { ...e.snap, pending_gates: [] };
    for (const g of gates.filter(isExternal)) {
      (e.withdrawn ??= new Set()).add(g.request_id);
      this.h.gateEnded?.(e, g, reason);
    }
  }

  /**
   * The daemon's refusal to start another process: `not_continuable` (the rule), or `continueFails` (the spawn did not work).
   * Null when a process can be started now.
   */
  private refusal(e: Entry): { why: NotContinuable | null; reason: string } | null {
    const why = conversationFacts(e.snap).not_continuable;
    if (why) return { why, reason: CONTINUE_REFUSED[why].message };
    if (e.continueFails) {
      const reason = e.continueFails;
      e.continueFails = undefined;
      return { why: null, reason };
    }
    return null;
  }

  /** A prompt reached an ended session: start the next process, or refuse as the daemon does (nothing is journaled). */
  private continueForCommand(e: Entry) {
    const no = this.refusal(e);
    if (no?.why === "not_ended") {
      const reason = `the session is ${e.snap.phase.state}`;
      throw err("not_running", `the session is not running: ${reason}`, { error: "not_running", reason });
    }
    if (no) throw err("continue_failed", `the session had ended and could not be continued, so nothing was sent: ${no.reason}`, { error: "continue_failed", reason: no.reason, ...(no.why ? { blocked: no.why } : {}) });
    this.startProcess(e);
  }

  /** `sessions.continue`: the next process of an ended session, or the runtime's error saying why not. */
  continueSession(e: Entry) {
    const no = this.refusal(e);
    if (no?.why) {
      const r = CONTINUE_REFUSED[no.why];
      throw err(r.code, r.code === "unsupported" ? `not supported: ${r.message}` : `not possible in the session's current state: ${r.message}`, { error: r.error, message: r.message });
    }
    if (no) throw err("spawn_failed", `could not start the session: ${no.reason}`, { error: "spawn", message: no.reason });
    this.startProcess(e);
  }

  /** A new process generation: the session is `starting` until its handshake is done (`ready`). */
  private startProcess(e: Entry) {
    const generation = e.snap.generation + 1;
    const pid = (e.snap.pid ?? 1000) + 1;
    this.emit(e, { kind: "spawned", generation, pid, argv: [e.snap.command ?? "agent"] });
    this.emit(e, { kind: "started", generation, resume: "resume", resume_id: e.snap.native_session_id });
    this.patch(e, { generation, pid, exit: null });
    this.setPhase(e, { state: "starting" });
    const turn = (e.turn = (e.turn ?? 0) + 1);
    this.h.after(300, () => e.turn === turn && this.ready(e));
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
    this.declineGates(e, { code: "session_stopped" });
    e.snap = { ...e.snap, ...endAllWork(e.snap, this.h.now()), turn: null, exit: { code: 0, at_ms: this.h.now(), requested: true } };
    this.setPhase(e, { state: "exited", code: 0 }, { kind: "exited", requested: true, handshake_timeout: false });
  }

  startTurn(e: Entry, prompt: string, input?: InputId) {
    const now = this.h.now();
    const turn = (e.turn = (e.turn ?? 0) + 1);
    const live = (fn: () => void) => () => e.turn === turn && fn();
    e.snap = { ...e.snap, turn: { started_at_ms: now, input, trigger: "input", prompt, tool_calls: 0 }, last_activity_ms: now };
    this.setPhase(e, { state: "working" }, { kind: "turn_started", input, trigger: "input" });
    this.emit(e, { kind: "agent", event: { type: "turn_started", input, trigger: "input" } });
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
    this.settle(e, { kind: "turn_completed", interrupted: false });
    this.drain(e);
  }

  /**
   * The phase follows what runs: Working while a turn is open or a background task runs, else Idle (a phase that is
   * `needs_you` or ended is left to its own causes unless `force`). Said (journaled) only when it changes, with the cause.
   */
  private settle(e: Entry, cause: PhaseCause, force = false) {
    const now = e.snap.phase.state;
    if (!force && now !== "working" && now !== "idle") return this.h.publish(e);
    const next = e.snap.turn || e.snap.background_running > 0 ? "working" : "idle";
    if (next !== now) this.setPhase(e, { state: next }, cause);
    else this.h.publish(e);
  }

  // ── what the agent does besides the turns we start: scripted by tests and by the demo scenario ──

  /** A task starts (a background subagent, a shell command): the session is working while it runs, with or without a turn. */
  startTask(e: Entry, task: { task_id: string; kind: string; description?: string; call_id?: string; subagent_type?: string; background?: boolean; run_id?: string }) {
    const background = task.background ?? true;
    this.emit(e, { kind: "agent", event: { type: "task_started", task_id: task.task_id, kind: task.kind, call_id: task.call_id, description: task.description, subagent_type: task.subagent_type, background, run_id: task.run_id } });
    this.settle(e, { kind: "task_started", task_id: task.task_id });
  }

  /** The agent ends a task with its status word. */
  endTask(e: Entry, taskId: string, status = "completed", summary?: string) {
    const call_id = e.snap.tasks.find((t) => t.task_id === taskId)?.call_id;
    this.emit(e, { kind: "agent", event: { type: "task_ended", task_id: taskId, call_id, status, ok: status === "completed", summary } });
    this.settle(e, { kind: "task_ended", task_id: taskId });
  }

  /** `stop_task`: the agent ends a task it runs as stopped; a task it does not know is a success that does nothing. */
  private stopTask(e: Entry, taskId: string) {
    if (e.snap.tasks.some((t) => t.task_id === taskId && t.running)) this.endTask(e, taskId, "stopped");
  }

  /** A line of a subagent's own conversation. */
  scoped(e: Entry, scope: Scope, event: AgentEvent) {
    this.emit(e, { kind: "agent", event: { type: "scoped", scope, event } });
    this.h.publish(e);
  }

  /** The agent starts a turn by itself (nobody sent an input): it is working without a prompt. */
  startOwnTurn(e: Entry) {
    const now = this.h.now();
    e.turn = (e.turn ?? 0) + 1;
    e.snap = { ...e.snap, turn: { started_at_ms: now, trigger: "agent", tool_calls: 0 }, last_activity_ms: now };
    this.emit(e, { kind: "agent", event: { type: "turn_started", trigger: "agent" } });
    this.settle(e, { kind: "turn_started", trigger: "agent" });
  }

  /** The turn the agent started by itself ends, with the origin the agent gives for it. */
  finishOwnTurn(e: Entry, reply: string, origin?: TurnOrigin) {
    const now = this.h.now();
    this.emit(e, { kind: "agent", event: { type: "assistant_delta", text: reply, message_id: `o${now}` } });
    this.emit(e, { kind: "agent", event: { type: "turn_completed", was_interrupted: false, num_turns: 1, usage: e.snap.usage, reply, origin } });
    e.snap = { ...e.snap, turn: null, turns_completed: e.snap.turns_completed + 1, last_turn: { ended_at_ms: now, was_interrupted: false, num_turns: 1, reply, origin }, last_activity_ms: now };
    this.settle(e, { kind: "turn_completed", interrupted: false });
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

  /** The request behind an external gate went away: the gate leaves without an answer, with `reason` (its owner's, legacy `withdrawn` when it said none). */
  withdrawGate(e: Entry, requestId: string, reason: GateReason = { code: "withdrawn" }) {
    (e.withdrawn ??= new Set()).add(requestId);
    const gone = e.snap.pending_gates.find((g) => g.gate.request_id === requestId)?.gate;
    const gates = e.snap.pending_gates.filter((g) => g.gate.request_id !== requestId);
    this.emit(e, { kind: "agent", event: { type: "gate_resolved", request_id: requestId, reason } });
    e.snap = { ...e.snap, pending_gates: gates };
    if (gone) this.h.gateEnded?.(e, gone, reason);
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
    if (kind.kind === "agent") e.snap = { ...e.snap, ...reduceWork(e.snap, kind.event, ev.t) };
    e.subs.forEach((s) => s(ev));
    if (e.trailSubs?.size) for (const item of projectEvent(e.events, e.events.length - 1)) e.trailSubs.forEach((s) => s(item));
  }
}
