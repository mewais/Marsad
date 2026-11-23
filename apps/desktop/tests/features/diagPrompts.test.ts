import { describe, expect, it } from "vitest";
import type { TrailEntry, TrailItem, UiEvent } from "../../src/bridge/wire";
import { parseRecord, type DiagRecord } from "../../src/lib/diagRecord";
import { connectionAttempts } from "../../src/features/diagnostics/attempts";
import { promptRows, promptTraces, stepTime, type PromptSources } from "../../src/features/diagnostics/prompts";
import { panelText } from "../../src/features/diagnostics/text";
import type { AppEventRec } from "../../src/state";

const T = "u-aaaaaaaaaaaaaaaa";
const T0 = 1_700_000_000_000;

let cursor = 0;
const item = (t: number, entry: TrailEntry): TrailItem => ({ cursor: (cursor += 65536), t, ...entry }) as TrailItem;
const applied = (t: number, input: number, o: Partial<Extract<TrailEntry, { ev: "input.applied" }>> = {}) => item(t, { ev: "input.applied", input, kind: "prompt", chars: 40, attachments: 0, bytes: 0, status: "sent", trace: T, ...o });
const app = (t: number, event: UiEvent): AppEventRec => ({ t, event });
const prompt = (trace = T, session = "s1", host = "hz"): UiEvent => ({ ev: "ui.prompt", trace, host, session, kind: "prompt", chars: 40, attachments: 0, bytes: 0 });
const log = (t: number, ev: string, fields: Record<string, unknown> = {}, seq = 1): DiagRecord => parseRecord(JSON.stringify({ b: "b", seq, t, m: 0, src: "daemon", lvl: "info", ev, ...fields }))!;

const remote = { host: "hz", id: "s1", hostName: "hetzner", local: false };
const local = { host: "x1", id: "s1", hostName: "thinkpad", local: true };
const base = (o: Partial<PromptSources> = {}): PromptSources => ({ trail: [], app: [], hub: [], host: [], session: remote, loaded: { hub: true, host: true }, ...o });
const states = (r: ReturnType<typeof promptRows>[number]) => Object.fromEntries(r.steps.map((s) => [s.id, s.state]));

describe("the path of a prompt", () => {
  it("a prompt that went all the way: every step seen, times relative to the first, other machines' clocks marked approximate", () => {
    const trail = [applied(T0 + 5_010, 3), item(T0 + 5_020, { ev: "input.written", input: 3 }), item(T0 + 5_300, { ev: "input.acknowledged", input: 3, steered: false }), item(T0 + 5_310, { ev: "turn.started", input: 3 })];
    const [row] = promptRows(
      base({
        trail,
        app: [app(T0, prompt()), app(T0 + 41, { ev: "ui.rpc", trace: T, method: "session.command", ms: 41, outcome: "ok" })],
        hub: [log(T0 + 4, "rpc.traced", { trace: T, method: "session.command", host: "hz", hops: 1, ms: 40, outcome: "ok" })],
        host: [log(T0 + 5_000, "rpc.traced", { trace: T, method: "session.command", hops: 0, ms: 20, outcome: "ok" })],
      }),
    );
    expect(row.steps.map((s) => s.id)).toEqual(["app", "hub", "host", "recorded", "sent", "written", "acknowledged", "turn"]);
    expect(row.steps.every((s) => s.state === "seen")).toBe(true);
    // The app and the hub are one computer: exact. The host is another machine: approximate.
    expect(row.steps.map(stepTime)).toEqual(["start", "+4 ms", "≈+5.0 s", "≈+5.0 s", "≈+5.0 s", "≈+5.0 s", "≈+5.3 s", "≈+5.3 s"]);
    expect(row.steps.map((s) => s.source)).toEqual(["app", "hub", "hetzner", "hetzner", "hetzner", "hetzner", "hetzner", "hetzner"]);
    expect(row.steps[0].detail).toBe("answered ok in 41 ms");
    expect(row.steps[1].detail).toBe("session.command · ok · 40 ms");
    expect(row).toMatchObject({ input: 3, trace: T, kind: "prompt", chars: 40, key: "input:3" });
  });

  it("on the local host the hub is the host: no host step, every time exact", () => {
    const [row] = promptRows(base({ session: local, trail: [applied(T0 + 30, 3), item(T0 + 31, { ev: "input.written", input: 3 }), item(T0 + 90, { ev: "input.acknowledged", input: 3, steered: false })], app: [app(T0, prompt(T, "s1", "x1"))], hub: [log(T0 + 10, "rpc.traced", { trace: T, method: "session.command", hops: 0, ms: 12, outcome: "ok" })], host: [] }));
    expect(row.steps.map((s) => s.id)).toEqual(["app", "hub", "recorded", "sent", "written", "acknowledged"]);
    expect(row.steps.map(stepTime)).toEqual(["start", "+10 ms", "+30 ms", "+30 ms", "+31 ms", "+90 ms"]);
    expect(row.steps.some((s) => s.approx)).toBe(false);
  });

  it("a step nothing observed is not seen when a later one was, and not yet after the last one seen; nothing is guessed", () => {
    // Recorded and sent, but the hub's log has nothing and nothing was written or acknowledged yet.
    const a = promptRows(base({ trail: [applied(T0, 3)], app: [app(T0 - 50, prompt())], host: [log(T0 - 10, "rpc.traced", { trace: T, method: "session.command", hops: 0, ms: 5, outcome: "ok" })] }))[0];
    expect(states(a)).toEqual({ app: "seen", hub: "not_seen", host: "seen", recorded: "seen", sent: "seen", written: "pending", acknowledged: "pending" });
    expect(a.steps.map(stepTime)).toEqual(["start", "not seen", "≈+40 ms", "≈+50 ms", "≈+50 ms", "not yet", "not yet"]);
    // Acknowledged without a written entry: written was not seen.
    const b = promptRows(base({ trail: [applied(T0, 3), item(T0 + 5, { ev: "input.acknowledged", input: 3, steered: false })] }))[0];
    expect(states(b)).toMatchObject({ written: "not_seen", acknowledged: "seen", app: "not_seen" });
  });

  it("a prompt without a trace id says so instead of pretending the legs were not seen", () => {
    const [row] = promptRows(base({ trail: [applied(T0, 3, { trace: undefined })] }));
    expect(row.trace).toBeNull();
    expect(row.steps.find((s) => s.id === "app")).toMatchObject({ state: "not_seen", detail: "no trace id" });
    expect(row.steps.find((s) => s.id === "hub")).toMatchObject({ state: "not_seen", detail: "no trace id" });
  });

  it("a leg whose log has not been read yet is pending, not not seen", () => {
    const [row] = promptRows(base({ trail: [applied(T0, 3)], loaded: { hub: false, host: false } }));
    expect(states(row)).toMatchObject({ hub: "pending", host: "pending", recorded: "seen" });
    expect(row.steps.find((s) => s.id === "hub")!.detail).toBe("log not read yet");
  });

  it("a queued prompt that was interrupted ends at Dropped, with the reason; later steps do not exist", () => {
    const [row] = promptRows(base({ trail: [applied(T0, 3, { status: "queued" }), item(T0 + 1_200, { ev: "input.dropped", input: 3, reason: "interrupted" })] }));
    expect(row.steps.map((s) => s.id)).toEqual(["app", "hub", "host", "recorded", "queued", "dropped"]);
    expect(row.steps.find((s) => s.id === "dropped")).toMatchObject({ label: "Dropped (interrupted)", tone: "bad", state: "seen" });
    expect(stepTime(row.steps[row.steps.length - 1])).toBe("+1.2 s");
  });

  it("a queued prompt that was sent later shows the queue and the send", () => {
    const [row] = promptRows(base({ trail: [applied(T0, 3, { status: "queued" }), item(T0 + 500, { ev: "input.sent", input: 3 }), item(T0 + 501, { ev: "input.written", input: 3 })] }));
    expect(states(row)).toMatchObject({ queued: "seen", sent: "seen", written: "seen", acknowledged: "pending" });
  });

  it("a failed write ends the path", () => {
    const [row] = promptRows(base({ trail: [applied(T0, 3), item(T0 + 7, { ev: "input.write_failed", input: 3, error: { code: "stdin_write", text: "broken pipe" } })] }));
    const last = row.steps[row.steps.length - 1];
    expect(last).toMatchObject({ id: "write_failed", label: "Write failed", detail: "stdin_write: broken pipe" });
    expect(row.steps.some((s) => s.id === "written" || s.id === "acknowledged")).toBe(false);
  });

  it("a write failure of no known input is not put on any prompt", () => {
    const [row] = promptRows(base({ trail: [applied(T0, 3), item(T0 + 7, { ev: "input.write_failed", error: { code: "stdin_write" } })] }));
    expect(row.steps.some((s) => s.id === "write_failed")).toBe(false);
  });

  it("a command the session did not accept ends at Not accepted", () => {
    const [row] = promptRows(base({ trail: [applied(T0, 3, { status: "not_running", reason: { code: "not_running", len: 20, fp: "00ff00ff" } })] }));
    expect(row.steps[row.steps.length - 1]).toMatchObject({ id: "not_accepted", label: "Not accepted (not_running)", tone: "bad" });
    expect(row.steps.some((s) => s.id === "sent" || s.id === "written")).toBe(false);
  });

  it("a prompt the host refused before recording it is a row of its own, ending at Refused", () => {
    const rows = promptRows(base({ app: [app(T0, prompt())], hub: [log(T0 + 3, "rpc.traced", { trace: T, method: "session.command", host: "hz", hops: 1, ms: 9, outcome: "error", code: "not_running" })], host: [log(T0 + 90, "rpc.traced", { trace: T, method: "session.command", hops: 0, ms: 2, outcome: "error", code: "not_running" }), log(T0 + 91, "session.command_refused", { session: "s1", trace: T, kind: "prompt", code: "not_running" }, 2)] }));
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ input: null, key: `trace:${T}` });
    expect(rows[0].steps.map((s) => [s.id, s.state])).toEqual([["app", "seen"], ["hub", "seen"], ["host", "seen"], ["refused", "seen"]]);
    expect(rows[0].steps[1].tone).toBe("bad");
    expect(rows[0].steps[3].label).toBe("Refused (not_running)");
  });

  it("a prompt the app sent that nobody recorded yet is waiting at the first step nothing saw", () => {
    const [row] = promptRows(base({ app: [app(T0, prompt())] }));
    expect(states(row)).toEqual({ app: "seen", hub: "pending", host: "pending", recorded: "pending" });
    expect(row.facts).toContainEqual({ k: "host", v: "the host has not recorded this prompt" });
  });

  it("an upload and a timed-out request are facts of the row", () => {
    const [row] = promptRows(base({ trail: [applied(T0, 3)], app: [app(T0 - 400, { ev: "ui.upload", trace: T, files: 2, bytes: 2048, ms: 380, outcome: "ok" }), app(T0 - 10, prompt())], host: [log(T0 + 1, "session.command_timeout", { session: "s1", trace: T, kind: "prompt" })] }));
    expect(row.facts).toContainEqual({ k: "upload", v: "2 files, 2.0 KB, 380 ms: ok" });
    expect(row.facts.some((f) => f.v.includes("timed out waiting"))).toBe(true);
  });

  it("only prompts and slash commands are rows; answers and interrupts are not; rows keep the host's order, unrecorded ones after", () => {
    const rows = promptRows(base({ trail: [applied(T0, 3), item(T0 + 1, { ev: "input.applied", input: 4, kind: "interrupt", chars: 0, attachments: 0, bytes: 0, status: "sent" }), applied(T0 + 2, 5, { kind: "run_slash", trace: "u-bbbbbbbbbbbbbbbb" })], app: [app(T0 + 9, prompt("u-cccccccccccccccc"))] }));
    expect(rows.map((r) => [r.key, r.kind])).toEqual([["input:3", "prompt"], ["input:5", "slash"], ["trace:u-cccccccccccccccc", "prompt"]]);
  });

  it("a prompt of another session is not this session's", () => {
    expect(promptRows(base({ app: [app(T0, prompt(T, "other"))] }))).toEqual([]);
    expect(promptRows(base({ app: [app(T0, prompt(T, "s1", "gpu"))] }))).toEqual([]);
  });

  it("a launch prompt takes its hub leg from the launch when no session.command was traced", () => {
    const [row] = promptRows(base({ trail: [applied(T0 + 100, 3)], app: [app(T0, { ev: "ui.launch", trace: T, host: "hz", driver: "claude", resume: "fresh", first_prompt: true, attachments: 0 })], hub: [log(T0 + 5, "rpc.traced", { trace: T, method: "sessions.launch", hops: 0, ms: 80, outcome: "ok" }), log(T0 + 6, "launch", { trace: T, host: "hz", driver: "claude", resume: "fresh", first_prompt: true, ms: 80, outcome: "ok" }, 2)] }));
    expect(row.steps[0].state).toBe("seen");
    expect(row.steps[1]).toMatchObject({ id: "hub", state: "seen", detail: "sessions.launch · ok · 80 ms" });
  });
});

describe("which traces to ask the daemons about", () => {
  it("those of the session's prompts and its start, and of the app's prompts for it; each once, newest last, at most 64", () => {
    const trail = [item(T0, { ev: "process.started", generation: 1, resume: "fresh", trace: "u-start0000000000" }), applied(T0, 3, { trace: "u-one00000000000" }), applied(T0, 4, { kind: "interrupt", trace: "u-interrupt000000" }), applied(T0, 5, { trace: "u-one00000000000" })];
    expect(promptTraces(trail, [app(T0, prompt("u-app0000000000", "s1", "hz")), app(T0, prompt("u-other0000000", "s2", "hz")), app(T0, { ev: "ui.command", trace: "u-cmd0000000000", host: "hz", session: "s1", command: "stop" })], { host: "hz", id: "s1" })).toEqual(["u-start0000000000", "u-one00000000000", "u-app0000000000"]);
    const many = Array.from({ length: 100 }, (_, i) => applied(T0, i, { trace: `u-t${String(i).padStart(10, "0")}` }));
    const t = promptTraces(many, [], { host: "hz", id: "s1" });
    expect(t).toHaveLength(64);
    expect(t[63]).toBe("u-t0000000099");
  });
});

describe("copy as text", () => {
  it("holds the facts, the paths with what was not seen, the trail, and says what it does not hold", () => {
    const row = promptRows(base({ trail: [applied(T0, 3)], app: [app(T0 - 50, prompt())] }))[0];
    const text = panelText({ facts: [{ k: "Session id", v: "s1" }, { k: "Model", v: null }], prompts: [row], trail: [applied(T0, 3)], trailHasMore: true, hostLog: null, at: T0 });
    expect(text).toContain("Session id: s1");
    expect(text).toContain("Model: not reported");
    expect(text).toContain(`#3 prompt, 40 characters, trace ${T}`);
    expect(text).toContain("App start (app) > Hub not seen > hetzner not seen > Recorded ≈+50 ms (hetzner)");
    expect(text).toContain("Written not yet");
    expect(text).toContain("(older entries exist and are not included)");
    expect(text).toContain("A prompt was sent (40 characters) — #3, handed to the agent");
    expect(text).toContain("Host log for this session\n  not opened");
  });
  it("an empty panel says so in every section", () => {
    const text = panelText({ facts: [], prompts: [], trail: null, trailHasMore: false, hostLog: [], at: T0 });
    expect(text).toContain("none seen");
    expect(text).toContain("Timeline (the host's clock)\n  not loaded");
    expect(text).toContain("no records");
  });
});

describe("connection attempts", () => {
  const r = (t: number, ev: string, fields: Record<string, unknown>, seq: number) => log(t, ev, fields, seq);
  const link = "hosts:win";
  it("groups the records by attempt, puts the ones without a number in the open attempt, and keeps what failed", () => {
    const records = [
      r(0, "link.attempt_begin", { link, n: 1 }, 1),
      r(5, "transport.spawned", { link, n: 1, program: "ssh", argc: 9 }, 2),
      r(900, "boot.ok", { host: "win", n: 1, ms: 30 }, 3),
      r(1000, "link.attempt_end", { link, n: 1, outcome: "connected", ms: 1000 }, 4),
      r(1001, "link.state", { link, from: "connecting", to: "connected" }, 5),
      r(7000, "link.closed", { link, reason: "io", connected_ms: 6000 }, 6),
      r(7100, "link.attempt_begin", { link, n: 2 }, 7),
      r(7110, "auth.prompt", { host: "win", prompt: 1, kind: "secret", attempt: 1 }, 8),
      r(17_100, "boot.failed", { host: "win", n: 2, stage: "hello", kind: "unreachable", exit: 255, stderr: "Connection timed out", error: { code: "unreachable", text: "could not reach the host" } }, 9),
      r(17_101, "link.attempt_end", { link, n: 2, outcome: "backoff", error: { code: "unreachable", text: "could not reach the host" }, ms: 10_001 }, 10),
      r(17_102, "link.state", { link, from: "connecting", to: "reconnecting", attempt: 2 }, 11),
      r(27_000, "link.attempt_begin", { link, n: 3 }, 12),
    ];
    const a = connectionAttempts(records);
    expect(a.map((x) => [x.n, x.outcome ?? null, x.records.length])).toEqual([[1, "connected", 4], [null, null, 2], [2, "backoff", 4], [null, null, 1], [3, null, 1]]);
    expect(a[2]).toMatchObject({ begin: 7100, ms: 10_001, error: "unreachable: could not reach the host", failure: { stage: "hello", kind: "unreachable", exit: 255, stderr: "Connection timed out", error: "unreachable: could not reach the host" } });
    // The sign-in prompt has no attempt number: it belongs to the attempt that was open.
    expect(a[2].records.map((x) => x.ev)).toEqual(["link.attempt_begin", "auth.prompt", "boot.failed", "link.attempt_end"]);
    expect(a[4].outcome).toBeUndefined();
  });
  it("records of an attempt whose beginning is not among them still make a block", () => {
    const a = connectionAttempts([r(10, "boot.ok", { host: "win", n: 9, ms: 3 }, 1), r(20, "link.attempt_end", { link, n: 9, outcome: "park", ms: 40 }, 2)]);
    expect(a).toHaveLength(1);
    expect(a[0]).toMatchObject({ n: 9, outcome: "park", ms: 40 });
    expect(a[0].begin).toBeUndefined();
  });
  it("nothing is nothing", () => expect(connectionAttempts([])).toEqual([]));
});

describe("the host's clock offset", () => {
  const SKEW = 5_000;
  /** A prompt whose host entries were written by a clock 5 s ahead of this computer's. */
  const sources = (o: Partial<PromptSources> = {}) =>
    base({
      trail: [applied(T0 + 12 + SKEW, 3), item(T0 + 15 + SKEW, { ev: "input.written", input: 3 })],
      app: [app(T0, prompt())],
      hub: [log(T0 + 4, "rpc.traced", { trace: T, method: "session.command", host: "hz", hops: 1, ms: 40, outcome: "ok" })],
      host: [log(T0 + 8 + SKEW, "rpc.traced", { trace: T, method: "session.command", hops: 0, ms: 20, outcome: "ok" })],
      ...o,
    });

  it("without a measured offset the host's times are the raw differences, approximate", () => {
    const [row] = promptRows(sources());
    expect(row.steps.map(stepTime)).toEqual(["start", "+4 ms", "≈+5.0 s", "≈+5.0 s", "≈+5.0 s", "≈+5.0 s", "not yet"]);
    expect(row.steps.some((s) => s.corrected)).toBe(false);
  });

  it("with the offset the host's times are moved onto this computer's clock, and stay marked approximate", () => {
    const [row] = promptRows(sources({ offset: { offsetMs: SKEW, rttMs: 12 } }));
    expect(row.steps.map(stepTime)).toEqual(["start", "+4 ms", "≈+8 ms", "≈+12 ms", "≈+12 ms", "≈+15 ms", "not yet"]);
    // Only what another machine's clock wrote was moved.
    expect(row.steps.filter((s) => s.state === "seen").map((s) => !!s.corrected)).toEqual([false, false, true, true, true, true]);
    expect(row.steps.filter((s) => s.state === "seen").slice(2).every((s) => s.approx)).toBe(true);
    // The order of the steps is the path's order, whatever the times.
    expect(row.steps.map((s) => s.id)).toEqual(["app", "hub", "host", "recorded", "sent", "written", "acknowledged"]);
  });

  it("a host behind this computer is corrected the same way", () => {
    const [row] = promptRows(sources({ offset: { offsetMs: -3_000, rttMs: 4 }, host: [log(T0 + 8 - 3_000, "rpc.traced", { trace: T, method: "session.command", hops: 0, ms: 20, outcome: "ok" })], trail: [applied(T0 + 12 - 3_000, 3)] }));
    expect(row.steps.map(stepTime).slice(0, 4)).toEqual(["start", "+4 ms", "≈+8 ms", "≈+12 ms"]);
  });

  it("a prompt to the local host is never corrected", () => {
    const [row] = promptRows(base({ session: local, trail: [applied(T0 + 30, 3)], app: [app(T0, prompt(T, "s1", "x1"))], offset: { offsetMs: 9_999, rttMs: 1 } }));
    expect(row.steps.some((s) => s.corrected)).toBe(false);
    expect(row.steps.map(stepTime)[0]).toBe("start");
  });

  it("a corrected host time earlier than the hub's record shows as earlier, not later", () => {
    // Nothing of this computer's was seen (an old session): the host's own entries come first, then the hub's record.
    const [row] = promptRows(base({ trail: [applied(T0 + 12 + SKEW, 3), item(T0 + 20 + SKEW, { ev: "input.written", input: 3 })], hub: [log(T0 + 30, "rpc.traced", { trace: T, method: "session.command", hops: 1, ms: 40, outcome: "ok" })], offset: { offsetMs: SKEW, rttMs: 0 } }));
    expect(row.steps.filter((s) => s.state === "seen").map((s) => [s.id, stepTime(s)])).toEqual([["hub", "start"], ["recorded", "≈-18 ms"], ["sent", "≈-18 ms"], ["written", "≈-10 ms"]]);
  });
});
