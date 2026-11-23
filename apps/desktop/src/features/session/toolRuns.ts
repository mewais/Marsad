import type { Item, ThinkingItem, ToolItem } from "./transcript";

/** Two or more consecutive tool calls shown as one row. Thinking between the calls belongs to the run. */
export type ToolRunItem = {
  kind: "toolrun";
  /** The first call's key: it never changes while the run grows, so the row keeps its expanded state. */
  key: string;
  /** The calls and the thinking between them, in order. */
  entries: (ToolItem | ThinkingItem)[];
  calls: ToolItem[];
};

/** What the transcript renders: the folded items, with tool runs collapsed. */
export type RowItem = Item | ToolRunItem;

const sameMembers = (a: readonly unknown[], b: readonly unknown[]) => a.length === b.length && a.every((x, i) => x === b[i]);

/**
 * Second stage over the folded items: a maximal run of tool calls becomes one `toolrun` row (a run of
 * one stays the plain tool item). Everything else breaks a run: prompts, responses, media, notices
 * and subagent cards (so do questions and permission requests, which are not items at all). Thinking
 * between two calls is part of the run; thinking before the first or after the last call is not.
 *
 * `prev` is the previous result: a run whose members are all unchanged is returned as the same object,
 * so a growing run replaces only its own row.
 */
export function groupToolRuns(items: readonly Item[], prev: readonly RowItem[] = []): RowItem[] {
  const before = new Map<string, ToolRunItem>();
  for (const r of prev) if (r.kind === "toolrun") before.set(r.key, r);

  const out: RowItem[] = [];
  let run: (ToolItem | ThinkingItem)[] = [];
  let calls = 0;
  let thinking: ThinkingItem[] = [];

  const flush = () => {
    if (calls === 1) out.push(run[0]);
    else if (calls > 1) {
      const old = before.get(run[0].key);
      out.push(old && sameMembers(old.entries, run) ? old : { kind: "toolrun", key: run[0].key, entries: run, calls: run.filter((x): x is ToolItem => x.kind === "tool") });
    }
    out.push(...thinking);
    run = [];
    calls = 0;
    thinking = [];
  };

  for (const it of items) {
    if (it.kind === "tool" && !it.subagent) {
      run.push(...thinking, it);
      thinking = [];
      calls++;
    } else if (it.kind === "thinking" && calls > 0) {
      thinking.push(it);
    } else {
      flush();
      out.push(it);
    }
  }
  flush();
  return out;
}
