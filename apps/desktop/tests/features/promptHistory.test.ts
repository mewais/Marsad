import { describe, expect, it } from "vitest";
import type { SessionEvent } from "../../src/bridge/wire";
import { caretAtEnd, caretAtStart, mayStepBack, mayStepForward, newer, older, promptsOf } from "../../src/features/session/promptHistory";

let n = 0;
const input = (command: unknown, status: unknown = { status: "sent" }) => ({ cursor: ++n * 65536, t: n, kind: "input", input: n, command, status }) as unknown as SessionEvent;
const prompt = (text: string, status?: unknown) => input({ command: "prompt", text }, status);

describe("promptsOf", () => {
  it("lists the prompts of the session oldest first, the first prompt included", () => {
    const events = [prompt("first"), { cursor: 1, t: 1, kind: "agent", event: { type: "turn_started" } } as SessionEvent, prompt("second", { status: "queued" }), prompt("third")];
    expect(promptsOf(events)).toEqual(["first", "second", "third"]);
  });

  it("leaves out prompts the agent did not accept, commands, empty prompts and repeats in a row", () => {
    const events = [
      prompt("a"), prompt("a"), prompt("b"), prompt("a"),
      prompt("nope", { status: "rejected", reason: "x" }), prompt("gone", { status: "not_running", reason: "x" }), prompt("unsup", { status: "unsupported", operation: "p" }),
      input({ command: "interrupt" }), input({ command: "run_slash", name: "init", args: "" }), prompt("   "), prompt(""),
    ];
    expect(promptsOf(events)).toEqual(["a", "b", "a"]);
  });

  it("keeps a prompt that was dropped later: it is what the user would want to send again", () => {
    const dropped = prompt("again", { status: "queued" });
    const events = [dropped, { cursor: 99, t: 1, kind: "agent", event: { type: "input_dropped", input: 1, reason: "interrupted" } } as unknown as SessionEvent];
    expect(promptsOf(events)).toEqual(["again"]);
  });
});

describe("walking back and forth", () => {
  const prompts = ["one", "two", "three"];

  it("Up goes to ever older prompts and stashes the draft; Down returns and restores it", () => {
    const a = older(prompts, null, "my draft");
    expect(a).toEqual({ browse: { depth: 1, draft: "my draft" }, text: "three" });
    if (a === "end") throw new Error("unreachable");
    const b = older(prompts, a.browse, "three");
    expect(b).toEqual({ browse: { depth: 2, draft: "my draft" }, text: "two" });
    if (b === "end") throw new Error("unreachable");
    const c = older(prompts, b.browse, "two");
    expect(c).toMatchObject({ text: "one", browse: { depth: 3 } });
    if (c === "end") throw new Error("unreachable");
    expect(older(prompts, c.browse, "one")).toBe("end");
    const d = newer(prompts, c.browse);
    expect(d).toEqual({ browse: { depth: 2, draft: "my draft" }, text: "two" });
    const e = newer(prompts, d.browse!);
    expect(e.text).toBe("three");
    const f = newer(prompts, e.browse!);
    expect(f).toEqual({ browse: null, text: "my draft" });
  });

  it("an empty history has nothing older", () => {
    expect(older([], null, "x")).toBe("end");
  });

  it("depth counts from the newest, so older prompts loaded in front do not move what is shown", () => {
    const a = older(["b", "c"], null, "");
    if (a === "end") throw new Error("unreachable");
    expect(a.text).toBe("c");
    expect(older(["a", "b", "c"], a.browse, "c")).toMatchObject({ text: "b" });
  });
});

describe("caret rules", () => {
  const at = (value: string, start: number, end = start) => ({ value, selectionStart: start, selectionEnd: end });

  it("Up only at offset 0 with nothing selected, Down only at the very end", () => {
    expect(caretAtStart(at("abc", 0))).toBe(true);
    expect(caretAtStart(at("abc", 1))).toBe(false);
    expect(caretAtStart(at("abc", 0, 2))).toBe(false);
    expect(caretAtStart(at("", 0))).toBe(true);
    expect(caretAtEnd(at("abc", 3))).toBe(true);
    expect(caretAtEnd(at("abc", 2))).toBe(false);
    expect(caretAtEnd(at("abc", 1, 3))).toBe(false);
    expect(caretAtEnd(at("a\nb", 1))).toBe(false);
    expect(mayStepBack(at("a\nb", 2), false)).toBe(false);
    expect(mayStepForward(at("a\nb", 1), false)).toBe(false);
  });

  it("a recalled one-line prompt can be walked from anywhere; a recalled several-line one keeps the strict rule", () => {
    expect(mayStepBack(at("one line", 4), true)).toBe(true);
    expect(mayStepForward(at("one line", 4), true)).toBe(true);
    expect(mayStepBack(at("one line", 2, 5), true)).toBe(false);
    expect(mayStepBack(at("a\nb", 2), true)).toBe(false);
    expect(mayStepForward(at("a\nb", 1), true)).toBe(false);
    expect(mayStepBack(at("a\nb", 0), true)).toBe(true);
    expect(mayStepForward(at("a\nb", 3), true)).toBe(true);
    // Not walking: a draft in the middle of a line is left alone.
    expect(mayStepBack(at("draft", 3), false)).toBe(false);
  });
});
