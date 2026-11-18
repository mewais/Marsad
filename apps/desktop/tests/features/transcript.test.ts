import { describe, expect, it } from "vitest";
import { buildItems, resultText, toolArg } from "../../src/features/session/transcript";
import type { SessionEvent } from "../../src/bridge/wire";

const ev = (cursor: number, event: unknown): SessionEvent => ({ cursor, t: cursor, kind: "agent", event } as unknown as SessionEvent);

describe("transcript", () => {
  it("takes a tool's arguments from an update when the start had none", () => {
    const items = buildItems([
      ev(1, { type: "tool_call_started", name: "bash", call_id: "b", input: { cwd: "/w" }, is_subagent: false }),
      ev(2, { type: "tool_call_updated", name: "bash", call_id: "b", status: "in_progress", input: { command: "ls -la", cwd: "/w" } }),
      ev(3, { type: "tool_call_updated", name: "bash", call_id: "b", status: "in_progress" }),
    ]);
    const tool = items.find((i) => i.kind === "tool");
    expect(tool && tool.kind === "tool" && toolArg(tool.input)).toBe("ls -la");
  });

  it("shows the text of ACP tool content", () => {
    expect(resultText([{ type: "content", content: { type: "text", text: "(no output)" } }])).toBe("(no output)");
    expect(resultText({ a: 1 })).toContain('"a": 1');
  });
});
