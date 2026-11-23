import { describe, expect, it } from "vitest";
import type { DriverInventory, EndCode, NotContinuable, SessionSnapshot } from "../../src/bridge/wire";
import { commandLine, quoteArg } from "../../src/lib/commandLine";
import { endedSentence, endWords } from "../../src/lib/endWords";
import { backgroundOnly, backgroundWords, canInterrupt, continueState, escapeInterrupts, interruptWords, notContinuableWords, sessionLine } from "../../src/state/domain";
import { phaseToStatus, STATUS, STATUS_ORDER } from "../../src/ui/status";
import { ownerNote, resumeRows, startTarget, validate, type NewSessionForm } from "../../src/features/newsession/model";
import { nodeStatus, summary } from "../../src/features/workstreams/model";
import type { WorkstreamView } from "../../src/bridge/wire";

describe("the daemon's verdict on continuing, in words", () => {
  const WHY: NotContinuable[] = ["not_ended", "loading", "agent_cannot_resume", "no_conversation_id", "nothing_saved"];

  it("every reason has its own sentence", () => {
    const words = WHY.map(notContinuableWords);
    expect(new Set(words).size).toBe(WHY.length);
    expect(notContinuableWords("loading")).toBe("The session is still loading; wait for it to finish.");
    expect(notContinuableWords("agent_cannot_resume")).toBe("This agent cannot pick a conversation back up.");
    expect(notContinuableWords("no_conversation_id")).toBe("The agent has not said which conversation this was, so it cannot be picked up again.");
    expect(notContinuableWords("nothing_saved")).toBe("The agent has not saved this conversation yet (it saves it with the first message), so there is nothing to continue.");
  });

  it("is read from the snapshot only: running offers nothing, continuable is ready, any other reason is blocked with it", () => {
    expect(continueState({ continuable: false, not_continuable: "not_ended" })).toEqual({ kind: "running" });
    expect(continueState({ continuable: true, not_continuable: null })).toEqual({ kind: "ready" });
    for (const why of WHY.filter((w) => w !== "not_ended")) expect(continueState({ continuable: false, not_continuable: why })).toEqual({ kind: "blocked", why });
    // The same objects every time: views that carry them compare shallowly.
    expect(continueState({ continuable: false, not_continuable: "nothing_saved" })).toBe(continueState({ continuable: false, not_continuable: "nothing_saved" }));
  });
});

describe("interrupt and Esc", () => {
  /** `turn`: a turn is open; `background`: tasks running in the background. */
  const snap = (state: SessionSnapshot["phase"]["state"], interrupt = true, turn = true, background = 0) =>
    ({ capabilities: { interrupt }, phase: { state }, turn: turn ? { started_at_ms: 0, trigger: "input", tool_calls: 0 } : null, background_running: background }) as Pick<SessionSnapshot, "capabilities" | "phase" | "turn" | "background_running">;

  it("the button interrupts a turn that starts, runs or waits for a decision, when the agent can be interrupted", () => {
    for (const s of ["starting", "working", "needs_you"] as const) expect(canInterrupt(snap(s))).toBe(true);
    // Working with no turn open (only background work): the daemon stops the background tasks, so the button applies.
    expect(canInterrupt(snap("working", true, false, 2))).toBe(true);
    for (const s of ["idle", "exited", "failed", "lost"] as const) expect(canInterrupt(snap(s))).toBe(false);
    expect(canInterrupt(snap("working", false))).toBe(false);
  });

  it("Esc interrupts only a turn that is running: never one that waits on you (interrupting declines what it asks)", () => {
    expect(escapeInterrupts(snap("working"))).toBe(true);
    for (const s of ["starting", "needs_you", "idle", "exited", "failed", "lost"] as const) expect(escapeInterrupts(snap(s))).toBe(false);
    expect(escapeInterrupts(snap("working", false))).toBe(false);
  });

  it("Esc never ends background work: with no turn open the daemon's interrupt would stop the background tasks, so only the button does", () => {
    expect(escapeInterrupts(snap("working", true, false, 2))).toBe(false);
    // A turn running next to background tasks: Esc stops the turn and leaves the tasks (the daemon's rule).
    expect(escapeInterrupts(snap("working", true, true, 2))).toBe(true);
  });

  it("the tooltip says what the daemon will stop", () => {
    expect(interruptWords(snap("working"))).toBe("Cancel the current turn; the agent keeps running");
    expect(interruptWords(snap("working", true, true, 2))).toBe("Cancel the current turn; the agent stays open and the 2 background tasks keep running.");
    expect(interruptWords(snap("working", true, true, 1))).toBe("Cancel the current turn; the agent stays open and the 1 background task keeps running.");
    expect(interruptWords(snap("working", true, false, 2))).toBe("No turn is running. Stop the 2 background tasks; the agent stays open.");
  });
});

describe("background work: the session is Working, and says why when no turn is open", () => {
  const s = (phase: string, turn: boolean, background: number) => ({ phase: { state: phase }, turn: turn ? { started_at_ms: 0, trigger: "input", tool_calls: 0 } : null, background_running: background, plan: [] }) as unknown as SessionSnapshot;

  it("only a working session with no turn open and tasks running is working in the background", () => {
    expect(backgroundOnly(s("working", false, 2))).toBe(2);
    expect(backgroundOnly(s("working", true, 2))).toBe(0);
    expect(backgroundOnly(s("working", false, 0))).toBe(0);
    expect(backgroundOnly(s("idle", false, 0))).toBe(0);
  });

  it("the line the Inbox shows says so; the status stays the phase", () => {
    expect(backgroundWords(1)).toBe("Working in the background: 1 task");
    expect(sessionLine(s("working", false, 3))).toBe("Working in the background: 3 tasks");
    expect(sessionLine(s("working", true, 3))).toBe("Working");
    expect(phaseToStatus("working")).toBe("working");
  });
});

describe("status: ended is its own", () => {
  it("an exited session is ended, apart from idle; unseen finished work keeps it under Finished; lost stays with failed", () => {
    expect(phaseToStatus("exited")).toBe("ended");
    expect(phaseToStatus("idle")).toBe("idle");
    expect(phaseToStatus("exited", true)).toBe("done");
    expect(phaseToStatus("idle", true)).toBe("done");
    expect(phaseToStatus("lost")).toBe("failed");
    expect(phaseToStatus("failed")).toBe("failed");
  });

  it("has its word, group and a place after idle in every list", () => {
    expect(STATUS.ended).toMatchObject({ label: "Ended", group: "Ended" });
    expect(STATUS_ORDER).toEqual(["needs", "failed", "done", "working", "idle", "ended"]);
  });

  it("a workstream node and its summary say ended, not idle", () => {
    const ended = { phase: { state: "exited" }, pending_gates: [] } as unknown as SessionSnapshot;
    expect(nodeStatus(undefined, ended)).toMatchObject({ status: "ended", word: "Ended" });
    const node = (state: string) => ({ host: "h", session: { phase: { state }, pending_gates: [], usage: { cost_usd: 0 } }, launching: false });
    const v = { workstream: { state: "running", nodes: [] }, nodes: [node("exited"), node("exited"), node("idle")], edges: [], cost: { sessions_usd: 0 }, pending_spawns: [] } as unknown as WorkstreamView;
    expect(summary(v).parts.map((p) => p.text)).toEqual(["2 ended"]);
  });
});

describe("a workstream node working only in the background", () => {
  const working = (turn: boolean, background: number) => ({ phase: { state: "working" }, pending_gates: [], turn: turn ? { started_at_ms: 0, trigger: "input", tool_calls: 0 } : null, background_running: background }) as unknown as SessionSnapshot;

  it("stays Working, and says in the background when no turn is open", () => {
    expect(nodeStatus(undefined, working(false, 2))).toMatchObject({ status: "working", word: "Working in the background" });
    expect(nodeStatus(undefined, working(true, 2))).toMatchObject({ status: "working", word: "Working" });
  });
});

describe("how a request ended, in words", () => {
  const CODES: EndCode[] = ["launched", "denied", "turn_ended", "turn_interrupted", "cancelled", "caller_gone", "session_ended", "session_gone", "gate_gone", "deadline_passed", "task_ended", "refused", "launch_failed", "gate_unavailable", "decided_elsewhere", "unrecorded"];

  it("says every code of the vocabulary, and the runtime's own three, with a sentence each", () => {
    const all = [...CODES, "process_ended", "session_stopped", "withdrawn"].map(endWords);
    expect(new Set(all).size).toBe(all.length);
    for (const w of all) expect(w).toMatch(/^[A-Z].*\.$/);
    expect(endWords("turn_interrupted")).toBe("The turn that asked was interrupted before the request was answered.");
    expect(endWords("session_stopped")).toBe("The session was ended while it was waiting.");
    expect(endWords("withdrawn")).toBe("The request was taken down by its owner; no reason was given.");
  });

  it("says a code it does not know generically, with the code", () => {
    expect(endWords("quota_exceeded")).toBe("It ended for a reason this version does not know (quota_exceeded).");
    expect(endWords("constructor")).toBe("It ended for a reason this version does not know (constructor).");
  });

  it("a request that went away: what it was, that it ended without an answer, why, and what the resolver said", () => {
    expect(endedSentence("the request to start scout", { code: "turn_interrupted" })).toBe("The request to start scout ended without your answer. The turn that asked was interrupted before the request was answered.");
    expect(endedSentence("a request", { code: "gate_gone", detail: "Cancelled from the CLI." })).toBe("A request ended without your answer. The request was taken down by something other than the agent that raised it. Cancelled from the CLI.");
  });
});

describe("a command line for display", () => {
  it("quotes only what a shell would split or expand, and keeps the argument list intact", () => {
    expect(commandLine(["claude", "--resume", "7d977a39-1b5e"])).toBe("claude --resume 7d977a39-1b5e");
    expect(commandLine(["codex", "resume", "a b", "it's", "$HOME", ""])).toBe(`codex resume 'a b' 'it'\\''s' '$HOME' ''`);
    expect(quoteArg("--flag=value")).toBe("--flag=value");
    expect(quoteArg("/home/me/my project")).toBe("'/home/me/my project'");
  });
});

describe("the new-session dialog: whose conversation a row is", () => {
  const snap = (id: string, native: string | null, over: Partial<SessionSnapshot> = {}) =>
    ({ id, driver: "claude", native_session_id: native, display_name: `session ${id}`, last_activity_ms: 1, continuable: false, not_continuable: "not_ended", ...over }) as unknown as SessionSnapshot;
  const inv = { driver: "claude", sessions: [{ id: "n1", project: "p", title: "one", modified_ms: 3 }, { id: "n2", project: "p", title: "two", modified_ms: 2 }, { id: "t1", project: "p", title: "terminal", modified_ms: 1 }] } as unknown as DriverInventory;
  const form = (resumeId: string, over: Partial<NewSessionForm> = {}): NewSessionForm => ({ host: "h", name: "", driver: "claude", cwd: "~/p", model: "", mode: "", start: "continue", resumeId, latest: false, prompt: "", ...over });

  it("a row is a Marsad session's when one carries its native id; the one to offer is running, else continuable, else the latest", () => {
    const rows = resumeRows(inv, [
      snap("a", "n1", { continuable: true, not_continuable: null, last_activity_ms: 9 }),
      snap("b", "n1", { last_activity_ms: 1 }), // running: preferred
      snap("c", "n2", { continuable: false, not_continuable: "nothing_saved", last_activity_ms: 5 }),
      snap("d", "n2", { continuable: true, not_continuable: null, last_activity_ms: 1 }), // continuable beats blocked
      { ...snap("e", "other"), driver: "codex" } as SessionSnapshot,
    ]);
    expect(rows.map((r) => [r.id, r.startedIn, r.session?.id])).toEqual([["n1", "marsad", "b"], ["n2", "marsad", "d"], ["t1", "terminal", undefined]]);
  });

  it("starting: a conversation of a running session is opened, of an ended one continued, of a blocked one refused with the reason; the agent's own storage launches", () => {
    const rows = resumeRows(inv, [snap("run", "n1"), snap("end", "n2", { continuable: true, not_continuable: null })]);
    expect(startTarget(form("n1"), rows)).toMatchObject({ kind: "open", session: { id: "run" } });
    expect(startTarget(form("n2"), rows)).toMatchObject({ kind: "continue", session: { id: "end" } });
    expect(startTarget(form("t1"), rows)).toEqual({ kind: "launch" });
    expect(startTarget(form("n2", { start: "new" }), rows)).toEqual({ kind: "launch" });
    expect(startTarget(form("", { latest: true }), rows)).toEqual({ kind: "launch" });
    const blocked = startTarget(form("n1"), resumeRows(inv, [snap("x", "n1", { continuable: false, not_continuable: "agent_cannot_resume" })]));
    expect(blocked).toMatchObject({ kind: "blocked", why: "This agent cannot pick a conversation back up." });
    expect(validate(form("n1"), { installed: true, capabilities: { resume: true } } as unknown as DriverInventory, true, blocked)).toBe("This agent cannot pick a conversation back up.");
    expect(ownerNote(startTarget(form("n1"), rows))).toBe("This conversation is running in your session “session run”. Open it instead of starting a second one.");
    expect(ownerNote({ kind: "launch" })).toBeNull();
  });
});
