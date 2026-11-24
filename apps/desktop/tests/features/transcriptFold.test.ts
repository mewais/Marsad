import { describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../../src/bridge/wire";
import { createTranscriptFold, deliveryNote, type Item, type UserItem } from "../../src/features/session/transcript";

let cursor = 0;
const base = () => ({ cursor: ++cursor * 65536, t: cursor });
const agent = (event: unknown) => ({ ...base(), kind: "agent", event }) as unknown as SessionEvent;
const raw = (e: Record<string, unknown>) => ({ ...base(), ...e }) as unknown as SessionEvent;
const prompt = (input: number, text: string, status: unknown = { status: "sent" }) => raw({ kind: "input", input, command: { command: "prompt", text }, status });
const users = (items: readonly Item[]) => items.filter((i): i is UserItem => i.kind === "user");
const fold = (events: SessionEvent[]) => createTranscriptFold().update(events);

describe("transcript fold: what happened to each prompt", () => {
  it("shows the first prompt of a session (an input event journaled with the start)", () => {
    const items = fold([raw({ kind: "spawned", generation: 1, argv: [] }), raw({ kind: "started", generation: 1, resume: "fresh", first_input: 7 }), prompt(7, "Add a health check")]);
    expect(users(items)).toMatchObject([{ text: "Add a health check", input: 7, delivery: { state: "sent" } }]);
  });

  it("a queued prompt becomes sent when the driver takes it out of its queue", () => {
    const events = [prompt(1, "first"), prompt(2, "second", { status: "queued" })];
    expect(users(fold(events))[1].delivery).toEqual({ state: "queued" });
    const sent = fold([...events, agent({ type: "input_sent", input: 2 })]);
    expect(users(sent)[1].delivery).toEqual({ state: "sent" });
    expect(deliveryNote(users(sent)[1].delivery)).toBeNull();
  });

  it.each([
    ["interrupted", "Not sent — interrupted"],
    ["shutdown", "Not sent — session ended"],
    ["process_ended", "Not sent — session ended"],
    ["unencodable", "Not sent — the agent could not take it"],
    ["refused", "Not sent — the agent refused it"],
  ])("a queued prompt dropped as %s says %s", (reason, text) => {
    const items = fold([prompt(2, "later", { status: "queued" }), agent({ type: "input_dropped", input: 2, reason, error: reason === "refused" ? "busy" : null })]);
    const u = users(items)[0];
    expect(u.delivery).toMatchObject({ state: "not_sent", cause: reason });
    expect(deliveryNote(u.delivery)).toEqual({ text, detail: reason === "refused" ? "busy" : undefined });
  });

  it("a drop after the prompt was sent still marks it, and a later sent does not undo a drop", () => {
    const items = fold([prompt(3, "x", { status: "queued" }), agent({ type: "input_dropped", input: 3, reason: "interrupted" }), agent({ type: "input_sent", input: 3 })]);
    expect(users(items)[0].delivery).toMatchObject({ state: "not_sent", cause: "interrupted" });
    const refused = fold([prompt(4, "y"), agent({ type: "input_dropped", input: 4, reason: "refused", error: "no" }), agent({ type: "input_dropped", input: 4, reason: "shutdown" })]);
    expect(users(refused)[0].delivery).toMatchObject({ cause: "refused", detail: "no" });
  });

  it("a write failure marks the prompt it belongs to; without one it is a notice", () => {
    const items = fold([prompt(5, "hello"), raw({ kind: "write_failed", input: 5, error: "broken pipe" })]);
    expect(users(items)[0].delivery).toEqual({ state: "not_sent", cause: "write_failed", detail: "broken pipe" });
    expect(deliveryNote(users(items)[0].delivery)?.text).toBe("Not sent — writing to the agent failed");
    const loose = fold([raw({ kind: "write_failed", input: null, error: "closed" })]);
    expect(loose).toMatchObject([{ kind: "notice", tone: "err", text: "Writing to the agent failed: closed" }]);
  });

  it.each([
    [{ status: "rejected", reason: "gate not pending" }, "rejected", "gate not pending"],
    [{ status: "not_running", reason: "the session is exited" }, "not_running", "the session is exited"],
    [{ status: "unsupported", operation: "prompt" }, "unsupported", "prompt"],
    [{ status: "withdrawn", request_id: "r1" }, "withdrawn", undefined],
  ])("an input whose status is %j is not delivered", (status, cause, detail) => {
    const u = users(fold([prompt(6, "hi", status)]))[0];
    expect(u.delivery).toEqual({ state: "not_sent", cause, ...(detail ? { detail } : {}) });
    expect(deliveryNote(u.delivery)).not.toBeNull();
  });

  it("a dropped answer to a gate is said in the transcript, since it has no bubble of its own", () => {
    const answer = raw({ kind: "input", input: 9, command: { command: "answer_gate", request_id: "g", decision: { choice: "allow" } }, status: { status: "queued" } });
    const items = fold([answer, agent({ type: "input_dropped", input: 9, reason: "unencodable", error: "gate gone" })]);
    expect(items).toMatchObject([{ kind: "notice", tone: "err", text: "Your answer was not sent: the agent could not take it (gate gone)" }]);
  });

  it("slash commands are user messages with the same statuses", () => {
    const items = fold([raw({ kind: "input", input: 2, command: { command: "run_slash", name: "init", args: "" }, status: { status: "queued" } }), agent({ type: "input_dropped", input: 2, reason: "interrupted" })]);
    expect(users(items)[0]).toMatchObject({ text: "/init", delivery: { cause: "interrupted" } });
  });
});

describe("transcript fold: events it does not show or does not know", () => {
  it("ignores acknowledgements, writes, starts, anomalies, stderr and unknown kinds without breaking", () => {
    const items = fold([
      prompt(1, "go"),
      agent({ type: "input_acknowledged", input: 1, steered: true }),
      raw({ kind: "written", input: 1 }),
      raw({ kind: "started", generation: 1, resume: "fresh" }),
      raw({ kind: "anomaly", anomaly: "activity_while_idle", first: "tool_call_started" }),
      raw({ kind: "stderr", text: "warn" }),
      raw({ kind: "something_new", x: 1 }),
      agent({ type: "something_new_too" }),
      raw({ kind: "agent" }),
      agent({ type: "input_sent", input: 999 }),
      agent({ type: "input_dropped", input: 999, reason: "shutdown" }),
    ]);
    expect(items).toHaveLength(1);
    expect(users(items)[0].delivery).toEqual({ state: "sent" });
  });

  it("an input without an id (an older daemon) still shows, and nothing can attach to it", () => {
    const items = fold([raw({ kind: "input", command: { command: "prompt", text: "old" }, status: { status: "sent" } }), agent({ type: "input_dropped", input: 0, reason: "shutdown" })]);
    expect(users(items)).toMatchObject([{ text: "old", delivery: { state: "sent" } }]);
  });
});

describe("transcript fold: identity", () => {
  const turn = (n: number) => [
    prompt(n, `prompt ${n}`),
    agent({ type: "assistant_delta", text: `hello ${n}`, message_id: `m${n}` }),
    agent({ type: "tool_call_started", name: "Read", call_id: `c${n}`, input: { file_path: `f${n}` }, is_subagent: false }),
    agent({ type: "tool_call_finished", name: "Read", call_id: `c${n}`, ok: true, result: "x" }),
  ];

  it("appending events keeps every item they do not touch the same object", () => {
    const f = createTranscriptFold();
    const events = [...turn(1), ...turn(2)];
    const a = f.update(events);
    const more = [...events, agent({ type: "assistant_delta", text: " more", message_id: "m3" })];
    const b = f.update(more);
    expect(b).toHaveLength(a.length + 1);
    a.forEach((it, i) => expect(b[i]).toBe(it));
    // A delta to the last message replaces that one item only.
    const c = f.update([...more, agent({ type: "assistant_delta", text: " and more", message_id: "m3" })]);
    expect(c[c.length - 1]).not.toBe(b[b.length - 1]);
    expect(c[c.length - 1]).toMatchObject({ text: " more and more" });
    b.slice(0, -1).forEach((it, i) => expect(c[i]).toBe(it));
  });

  it("a tool finishing or a prompt being sent replaces only that item", () => {
    const f = createTranscriptFold();
    const events = [prompt(1, "a"), prompt(2, "b", { status: "queued" }), agent({ type: "tool_call_started", name: "Bash", call_id: "t", input: { command: "ls" }, is_subagent: false }), agent({ type: "assistant_delta", text: "x", message_id: "m" })];
    const a = f.update(events);
    // One event object per event: the stream is the same events every time it is folded.
    const sent = agent({ type: "input_sent", input: 2 });
    const finished = agent({ type: "tool_call_finished", name: "Bash", call_id: "t", ok: true, result: "out" });
    const b = f.update([...events, sent]);
    expect(b[0]).toBe(a[0]);
    expect(b[1]).not.toBe(a[1]);
    expect(b[2]).toBe(a[2]);
    expect(b[3]).toBe(a[3]);
    const c = f.update([...events, sent, finished]);
    expect(c[2]).not.toBe(b[2]);
    expect(c[2]).toMatchObject({ done: true });
    expect([c[0], c[1], c[3]]).toEqual([b[0], b[1], b[3]]);
    expect(c[0]).toBe(b[0]);
    expect(c[1]).toBe(b[1]);
    expect(c[3]).toBe(b[3]);
  });

  it("the same array and events that change nothing give the same result", () => {
    const f = createTranscriptFold();
    const events = turn(1);
    const a = f.update(events);
    expect(f.update(events)).toBe(a);
    expect(f.update([...events, raw({ kind: "stderr", text: "noise" })])).toBe(a);
  });

  it("prepending an older page folds again but hands back the items that came out the same", () => {
    const f = createTranscriptFold();
    const all = [...turn(1), ...turn(2), ...turn(3)];
    const recent = all.slice(8);
    const a = f.update(recent);
    const b = f.update(all);
    expect(b).toEqual(fold(all));
    // Everything after the older page is the very same object.
    a.forEach((it) => expect(b.find((x) => x.key === it.key)).toBe(it));
    expect(b.length).toBeGreaterThan(a.length);
  });

  it("older events that finish what the loaded events started are applied (a drop arrives for a prompt before the page)", () => {
    const all = [prompt(1, "x", { status: "queued" }), agent({ type: "assistant_delta", text: "..." }), agent({ type: "input_dropped", input: 1, reason: "interrupted" })];
    const f = createTranscriptFold();
    expect(users(f.update(all.slice(1)))).toHaveLength(0);
    expect(users(f.update(all))[0].delivery).toMatchObject({ cause: "interrupted" });
  });
});

// A seeded generator of event streams that hit every branch of the fold, including ones that make no sense.
function rng(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const SHARED_INPUT = { command: "x" };

function randomEvents(seed: number, n: number): SessionEvent[] {
  const r = rng(seed);
  const pick = <T,>(xs: T[]): T => xs[Math.floor(r() * xs.length)];
  const out: SessionEvent[] = [];
  const inputs: number[] = [];
  const calls: string[] = [];
  let input = 0;
  let call = 0;
  for (let i = 0; i < n; i++) {
    const roll = r();
    if (roll < 0.1) {
      const id = ++input;
      inputs.push(id);
      out.push(prompt(id, `p${id}`, pick([{ status: "sent" }, { status: "queued" }, { status: "rejected", reason: "no" }, { status: "not_running", reason: "x" }])));
    } else if (roll < 0.13) out.push(raw({ kind: "input", input: ++input, command: { command: "interrupt" }, status: { status: "sent" } }));
    else if (roll < 0.28) out.push(agent({ type: "assistant_delta", text: pick(["a", "bc", "d"]), message_id: r() < 0.7 ? pick(["m1", "m2"]) : undefined }));
    else if (roll < 0.34) out.push(agent({ type: "thinking_delta", text: "t", message_id: pick(["m1", "m2"]) }));
    else if (roll < 0.5) {
      const id = `c${++call}`;
      calls.push(id);
      out.push(agent({ type: "tool_call_started", name: "Read", call_id: id, input: { file_path: id }, is_subagent: r() < 0.1 }));
    } else if (roll < 0.58 && calls.length) out.push(agent({ type: "tool_call_updated", name: "Read", call_id: pick(calls), status: "in_progress", parts: r() < 0.3 ? [{ type: "text", text: "p" }] : undefined, input: r() < 0.2 ? SHARED_INPUT : undefined }));
    else if (roll < 0.68 && calls.length) out.push(agent({ type: "tool_call_finished", name: "Read", call_id: pick(calls), ok: r() < 0.8, result: "done" }));
    else if (roll < 0.72) out.push(agent({ type: pick(["assistant_content", "user_content"]), parts: [{ type: "text", text: "m" }] }));
    else if (roll < 0.78 && inputs.length) out.push(agent({ type: "input_sent", input: pick(inputs) }));
    else if (roll < 0.84 && inputs.length) out.push(agent({ type: "input_dropped", input: pick(inputs), reason: pick(["interrupted", "shutdown", "unencodable", "refused", "process_ended"]), error: r() < 0.3 ? "why" : null }));
    else if (roll < 0.86 && inputs.length) out.push(raw({ kind: "write_failed", input: r() < 0.6 ? pick(inputs) : null, error: "e" }));
    else if (roll < 0.89) out.push(raw({ kind: "notice", message: "n", code: "other" }));
    else if (roll < 0.91) out.push(agent({ type: "command_rejected", command: "x", message: "y" }));
    else if (roll < 0.93) out.push(raw({ kind: "phase", phase: pick([{ state: "failed", reason: "r" }, { state: "idle" }]), cause: { kind: "other" } }));
    else if (roll < 0.95) out.push(agent({ type: "input_acknowledged", input: pick(inputs.length ? inputs : [1]) }));
    else if (roll < 0.97) out.push(raw({ kind: pick(["written", "stderr", "brand_new_kind"]), input: 1, text: "s" }));
    else out.push(agent({ type: "brand_new_event" }));
  }
  return out;
}

describe("transcript fold: incremental equals from scratch", () => {
  const same = (a: readonly Item[], b: readonly Item[]) => expect(JSON.parse(JSON.stringify(a))).toEqual(JSON.parse(JSON.stringify(b)));

  it("for random streams, however the events arrive in chunks", () => {
    for (let seed = 1; seed <= 60; seed++) {
      const events = randomEvents(seed, 40 + (seed % 5) * 40);
      const r = rng(seed * 977);
      const f = createTranscriptFold();
      let upto = 0;
      let prev: readonly Item[] = [];
      while (upto < events.length) {
        upto = Math.min(events.length, upto + 1 + Math.floor(r() * 7));
        const items = f.update(events.slice(0, upto));
        same(items, fold(events.slice(0, upto)));
        // Whatever did not change is the same object as before.
        prev.forEach((p, i) => {
          if (JSON.stringify(p) === JSON.stringify(items[i])) expect(items[i], `seed ${seed}: item ${i} was replaced by an equal copy`).toBe(p);
        });
        prev = items;
      }
    }
  });

  it("for random streams, when an older page is prepended after the newer events", () => {
    for (let seed = 100; seed < 140; seed++) {
      const events = randomEvents(seed, 120);
      const cut = 10 + (seed % 7) * 12;
      const f = createTranscriptFold();
      f.update(events.slice(cut));
      same(f.update(events), fold(events));
      // …and live events after that.
      const more = [...events, ...randomEvents(seed + 1000, 15)];
      same(f.update(more), fold(more));
    }
  });

  it("never throws, whatever the stream holds", () => {
    const spy = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    for (let seed = 200; seed < 230; seed++) expect(() => fold(randomEvents(seed, 200))).not.toThrow();
    spy.mockRestore();
  });
});
