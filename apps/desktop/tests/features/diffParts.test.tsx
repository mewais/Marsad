import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ContentPart, SessionEvent } from "../../src/bridge/wire";
import { TranscriptItem } from "../../src/features/session/TranscriptView";
import { groupToolRuns, type RowItem } from "../../src/features/session/toolRuns";
import { createTranscriptFold } from "../../src/features/session/transcript";
import { toolFileRefs } from "../../src/features/media/refs";
import { MAIN } from "../../src/state/scope";
import { AppProvider, createApp } from "../../src/state";
import { quietBackend } from "../helpers";

let n = 0;
const ev = (t: number, extra: Record<string, unknown>) => ({ cursor: ++n * 65536, t, ...extra }) as unknown as SessionEvent;
const agent = (t: number, event: Record<string, unknown>) => ev(t, { kind: "agent", event });

/** A finished tool call, folded the way a transcript folds it. */
function toolRow(input: Record<string, unknown>, parts: ContentPart[] = [], result: unknown = "Edited"): RowItem {
  const events = [
    agent(1, { type: "tool_call_started", name: "Edit", call_id: "c1", input, is_subagent: false }),
    agent(2, { type: "tool_call_finished", name: "Edit", call_id: "c1", ok: true, result, parts }),
  ];
  return groupToolRuns(createTranscriptFold(MAIN).update(events))[0];
}

/** Renders the row and opens the tool call (its body is what a diff is drawn in). */
async function openTool(row: RowItem) {
  const user = userEvent.setup();
  // A tool that changed files looks their paths up on the host, so the row is drawn in the app's context as it is in the app.
  const app = createApp(quietBackend());
  render(<AppProvider value={app}><TranscriptItem item={row} /></AppProvider>);
  await user.click(screen.getByRole("button", { name: /Edit/ }));
}

const lines = (cls: "d" | "a") => document.querySelectorAll(`.ln.${cls}`).length;

describe("diff parts of a tool result", () => {
  it("an edit the driver reported shows its removed lines as - and its added lines as +", async () => {
    await openTool(toolRow({ file_path: "src/app.ts" }, [{ type: "diff", path: "src/app.ts", old_text: "const a = 1\nconst b = 2", new_text: "const a = 3" }]));
    expect(screen.getByTestId("diff-path")).toHaveTextContent("src/app.ts");
    expect(lines("d")).toBe(2);
    expect(lines("a")).toBe(1);
    expect(screen.getByTestId("tool-diff")).toBeInTheDocument();
  });

  it("a created file has no old text, so every line is an addition", async () => {
    await openTool(toolRow({ file_path: "notes.md" }, [{ type: "diff", path: "notes.md", new_text: "# Notes\n\nfirst line" }]));
    expect(lines("d")).toBe(0);
    expect(lines("a")).toBe(3);
  });

  it("each file change is its own block, in the order the driver reported them", async () => {
    await openTool(
      toolRow({}, [
        { type: "diff", path: "a.ts", old_text: "x", new_text: "y" },
        { type: "diff", path: "b.ts", new_text: "z" },
      ]),
    );
    const blocks = screen.getAllByTestId("tool-diff-part");
    expect(blocks).toHaveLength(2);
    expect(blocks[0]).toHaveTextContent("a.ts");
    expect(blocks[1]).toHaveTextContent("b.ts");
  });

  it("a hunk is shown as the unified diff the driver gave, not as the two texts", async () => {
    await openTool(toolRow({}, [{ type: "diff", path: "c.ts", old_text: "old line", new_text: "new line", hunk: "@@ -1 +1 @@\n-old line\n+new line" }]));
    expect(screen.getByTestId("tool-diff-text")).toBeInTheDocument();
    expect(screen.getByTestId("tool-diff-text")).toHaveTextContent("@@ -1 +1 @@");
    expect(lines("d")).toBe(1);
    expect(lines("a")).toBe(1);
  });

  it("an edit with old and new strings in its input but no diff part shows no diff", async () => {
    // Claude-shaped input is not a contract: only the diff parts a driver reports are drawn.
    await openTool(toolRow({ file_path: "src/app.ts", old_string: "return 1", new_string: "return 2" }, [], "Edited src/app.ts"));
    expect(screen.queryByTestId("tool-diff")).toBeNull();
    expect(screen.queryByTestId("diff-path")).toBeNull();
    expect(document.querySelector(".tool-b")).toHaveTextContent("Edited src/app.ts");
  });
});

describe("files a tool call changed", () => {
  it("the file chips take their paths from the diff parts", () => {
    expect(toolFileRefs("Edit", {}, [{ type: "diff", path: "src/app.ts", new_text: "x" }])).toEqual(["src/app.ts"]);
  });

  it("a write-like tool without a diff part still names its file_path, and a string pair names nothing", () => {
    expect(toolFileRefs("Edit", { file_path: "src/app.ts", old_string: "a", new_string: "b" })).toEqual(["src/app.ts"]);
    expect(toolFileRefs("Edit", { old_string: "a", new_string: "b" }, [])).toEqual([]);
  });

  it("a path named by a diff part and by the input is listed once", () => {
    expect(toolFileRefs("Write", { file_path: "a.ts" }, [{ type: "diff", path: "a.ts", new_text: "x" }])).toEqual(["a.ts"]);
  });
});
