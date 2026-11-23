import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { SessionEvent } from "../../src/bridge/wire";
import { TranscriptItem } from "../../src/features/session/TranscriptView";
import { groupToolRuns, type RowItem } from "../../src/features/session/toolRuns";
import { createTranscriptFold } from "../../src/features/session/transcript";

let n = 0;
const ev = (e: Record<string, unknown>) => ({ cursor: ++n * 65536, t: n, ...e }) as unknown as SessionEvent;
const agent = (event: unknown) => ev({ kind: "agent", event });
const call = (id: string, name = "Read", extra: Record<string, unknown> = {}) => agent({ type: "tool_call_started", name, call_id: id, input: { file_path: `${id}.ts` }, is_subagent: false, ...extra });
const done = (id: string, ok = true) => agent({ type: "tool_call_finished", name: "Read", call_id: id, ok, result: "r" });
const text = (t: string, id = "m") => agent({ type: "assistant_delta", text: t, message_id: id });
const think = () => agent({ type: "thinking_delta", text: "hmm", message_id: "th" });
const user = (t: string) => ev({ kind: "input", input: ++n, command: { command: "prompt", text: t }, status: { status: "sent" } });
const rows = (events: SessionEvent[], prev: readonly RowItem[] = []) => groupToolRuns(createTranscriptFold().update(events), prev);
const shape = (r: readonly RowItem[]) => r.map((x) => (x.kind === "toolrun" ? `run(${x.calls.length})` : x.kind));

describe("groupToolRuns", () => {
  it("collapses consecutive tool calls; a run of one stays a plain tool row", () => {
    expect(shape(rows([call("a"), done("a"), call("b"), done("b"), call("c")]))).toEqual(["run(3)"]);
    expect(shape(rows([call("a"), done("a")]))).toEqual(["tool"]);
    expect(shape(rows([]))).toEqual([]);
  });

  it("prompts, responses, notices, errors, media and subagent cards break a run", () => {
    const broken = rows([
      call("a"), call("b"), user("go"), call("c"), call("d"), text("hi"), call("e"), call("f"),
      ev({ kind: "notice", message: "n", code: "other" }), call("g"), call("h"),
      call("s", "Task", { is_subagent: true }), call("i"), call("j"),
      agent({ type: "assistant_content", parts: [{ type: "text", text: "p" }] }), call("k"), call("l"),
      agent({ type: "command_rejected", command: "x", message: "no" }), call("m"),
    ]);
    expect(shape(broken)).toEqual(["run(2)", "user", "run(2)", "assistant", "run(2)", "notice", "run(2)", "tool", "run(2)", "media", "run(2)", "notice", "tool"]);
    const s = broken[7];
    expect(s).toMatchObject({ kind: "tool", subagent: true });
  });

  it("thinking between two calls is part of the run, thinking at either end is not", () => {
    const r = rows([think(), call("a"), call("b"), think(), call("c"), think()]);
    expect(shape(r)).toEqual(["thinking", "run(3)", "thinking"]);
    const run = r[1];
    expect(run.kind === "toolrun" && run.entries.map((e) => e.kind)).toEqual(["tool", "tool", "thinking", "tool"]);
    // A single call followed by thinking is not a run.
    expect(shape(rows([call("a"), think(), text("x")]))).toEqual(["tool", "thinking", "assistant"]);
  });

  it("a run keeps its key while it grows, and rows nothing touched are the same objects", () => {
    const f = createTranscriptFold();
    const e1 = [user("go"), call("a"), done("a"), call("b")];
    const r1 = groupToolRuns(f.update(e1));
    const e2 = [...e1, done("b"), call("c")];
    const r2 = groupToolRuns(f.update(e2), r1);
    expect(r1[1].key).toBe(r2[1].key);
    expect(r2[1]).not.toBe(r1[1]);
    expect(r2[0]).toBe(r1[0]);
    // Events that change nothing in the run return the very same run.
    const r3 = groupToolRuns(f.update([...e2, ev({ kind: "stderr", text: "x" })]), r2);
    expect(r3[1]).toBe(r2[1]);
    // A run that ended is the same object while later rows are added after it.
    const e4 = [...e2, text("done"), user("next")];
    const r4 = groupToolRuns(f.update(e4), r3);
    expect(r4[1]).toBe(r3[1]);
    expect(shape(r4)).toEqual(["user", "run(3)", "assistant", "user"]);
  });
});

describe("tool run row", () => {
  const render_ = (rs: readonly RowItem[]) => rs.map((r) => <TranscriptItem key={r.key} item={r} />);

  it("shows the count and the latest call, expands to the calls, and a growing run keeps what the user chose", async () => {
    const u = userEvent.setup();
    const f = createTranscriptFold();
    const e1 = [call("a"), done("a"), call("b"), done("b")];
    let rs = groupToolRuns(f.update(e1));
    const view = render(<>{render_(rs)}</>);
    const head = () => screen.getByRole("button", { name: /tool calls/ });
    expect(head()).toHaveTextContent("2 tool calls");
    expect(head()).toHaveTextContent("Read b.ts");
    expect(head()).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: /Read a\.ts/ })).toBeNull();

    // Open it: the individual calls are there, each its own row.
    await u.click(head());
    const body = document.querySelector(".run-b") as HTMLElement;
    expect(within(body).getAllByRole("button").map((b) => b.textContent)).toEqual([expect.stringContaining("a.ts"), expect.stringContaining("b.ts")]);

    // The agent keeps calling tools: the same row updates, still open.
    const e2 = [...e1, call("c")];
    rs = groupToolRuns(f.update(e2), rs);
    view.rerender(<>{render_(rs)}</>);
    expect(head()).toHaveTextContent("3 tool calls");
    expect(head()).toHaveTextContent("Read c.ts");
    expect(head()).toHaveTextContent("running");
    expect(head()).toHaveAttribute("aria-expanded", "true");
    expect(within(document.querySelector(".run-b") as HTMLElement).getAllByRole("button")).toHaveLength(3);

    // Close it, and let the next calls arrive: it stays closed.
    await u.click(head());
    const e3 = [...e2, done("c", false), call("d"), done("d")];
    rs = groupToolRuns(f.update(e3), rs);
    view.rerender(<>{render_(rs)}</>);
    expect(head()).toHaveTextContent("4 tool calls");
    expect(head()).toHaveTextContent("1 failed");
    expect(head()).toHaveAttribute("aria-expanded", "false");
    expect(document.querySelector(".run-b")).toBeNull();
  });

  it("only the changed run re-renders (the rows before it are the same elements)", () => {
    const f = createTranscriptFold();
    const e1 = [call("a"), call("b"), text("hello"), call("c"), call("d")];
    const r1 = groupToolRuns(f.update(e1));
    const r2 = groupToolRuns(f.update([...e1, call("e")]), r1);
    expect(r2[0]).toBe(r1[0]);
    expect(r2[1]).toBe(r1[1]);
    expect(r2[2]).not.toBe(r1[2]);
  });
});
