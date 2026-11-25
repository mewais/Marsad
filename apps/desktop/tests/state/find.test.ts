import { describe, expect, it } from "vitest";
import type { SearchPage, SessionEvent } from "../../src/bridge/wire";
import { groupToolRuns, type RowItem } from "../../src/features/session/toolRuns";
import { createTranscriptFold } from "../../src/features/session/transcript";
import { cursorIndex, pagingLeft, pickOccurrence, planJump, PAGING_LIMITS, stepIndex, streamPages, textMatches } from "../../src/state/find";
import { MAIN } from "../../src/state/scope";

const ev = (cursor: number, e: Record<string, unknown>) => ({ cursor, t: cursor, kind: "agent", event: e }) as unknown as SessionEvent;
const prompt = (cursor: number, text: string) =>
  ({ cursor, t: cursor, kind: "input", input: cursor, command: { command: "prompt", text, attachments: [] }, status: { status: "sent" } }) as unknown as SessionEvent;
const keyOf = (r: RowItem) => r.key;
const rowsOf = (events: SessionEvent[]) => groupToolRuns(createTranscriptFold(MAIN).update(events));

describe("cursorIndex: a cursor maps to the row it was folded into", () => {
  it("a merged delta cursor (not the first of its message) maps to the message's row", () => {
    const rows = rowsOf([
      prompt(10, "hello"),
      ev(20, { type: "assistant_delta", text: "a", message_id: "m" }),
      ev(30, { type: "assistant_delta", text: "b", message_id: "m" }),
      ev(40, { type: "assistant_delta", text: "c", message_id: "m" }),
    ]);
    const idx = cursorIndex(rows, keyOf);
    expect(idx.at(20)?.key).toBe("20");
    expect(idx.at(30)?.key).toBe("20");
    expect(idx.at(40)?.key).toBe("20");
    expect(idx.at(10)?.key).toBe("10");
  });

  it("a cursor after the last row's start maps to that row, and a cursor before the first row maps to nothing", () => {
    const idx = cursorIndex(rowsOf([prompt(10, "a"), prompt(50, "b")]), keyOf);
    expect(idx.at(55)?.key).toBe("50");
    expect(idx.at(5)).toBeUndefined();
  });

  it("a tool run maps every cursor of its calls to the run's key", () => {
    const rows = rowsOf([
      ev(60, { type: "tool_call_started", name: "Read", call_id: "a", input: { file_path: "a.ts" }, is_subagent: false }),
      ev(61, { type: "tool_call_started", name: "Read", call_id: "b", input: { file_path: "b.ts" }, is_subagent: false }),
      ev(62, { type: "tool_call_finished", name: "Read", call_id: "a", ok: true, result: "x" }),
      ev(63, { type: "tool_call_finished", name: "Read", call_id: "b", ok: true, result: "y" }),
    ]);
    const idx = cursorIndex(rows, keyOf);
    expect(rows).toHaveLength(1);
    expect(idx.at(62)?.key).toBe("60");
    expect(idx.at(63)?.key).toBe("60");
  });
});

describe("planJump: loaded, older than the window, or detached", () => {
  const rows = rowsOf([prompt(100, "old"), prompt(150, "newer"), prompt(200, "newest")]);
  const idx = cursorIndex(rows, keyOf);

  it("a cursor inside the loaded window is a row", () => {
    expect(planJump(150, idx, { oldest: 100, hasMore: true, loading: false })).toEqual({ kind: "row", extent: expect.objectContaining({ key: "150" }) });
  });

  it("a cursor older than the window pages back while the host has more", () => {
    expect(planJump(50, idx, { oldest: 100, hasMore: true, loading: false })).toEqual({ kind: "older" });
  });

  it("a cursor older than the window with nothing more on the host is detached", () => {
    expect(planJump(50, idx, { oldest: 100, hasMore: false, loading: false })).toEqual({ kind: "detached" });
  });

  it("waits while a page is loading", () => {
    expect(planJump(50, idx, { oldest: 100, hasMore: true, loading: true })).toBe("wait");
  });
});

describe("paging budget and stepping", () => {
  it("paging stops at 2000 events or 8 MiB, whichever comes first", () => {
    expect(PAGING_LIMITS).toEqual({ events: 2000, bytes: 8 << 20 });
    expect(pagingLeft({ events: 1999, bytes: 0 })).toBe(true);
    expect(pagingLeft({ events: 2000, bytes: 0 })).toBe(false);
    expect(pagingLeft({ events: 0, bytes: 8 << 20 })).toBe(false);
  });

  it("stepping wraps around the hits, and starts at the first (or last, going back) when none is active", () => {
    expect(stepIndex(-1, 3, 1)).toBe(0);
    expect(stepIndex(-1, 3, -1)).toBe(2);
    expect(stepIndex(0, 3, 1)).toBe(1);
    expect(stepIndex(2, 3, 1)).toBe(0);
    expect(stepIndex(0, 3, -1)).toBe(2);
    expect(stepIndex(0, 0, 1)).toBe(-1);
  });
});

describe("streamPages: every page in order, following the resume token", () => {
  const page = (hits: number[], next?: string): SearchPage => ({ hits: hits.map((c) => ({ cursor: c, part: "live", field: "user_text", snippet: "", start: 0, len: 0, at: c })) as SearchPage["hits"], next, scanned_bytes: 1 });

  it("asks for the next page with the token until there is none", async () => {
    const seen: number[] = [];
    const tokens: string[] = [];
    await streamPages(
      async () => page([1, 2], "t1"),
      async (token) => {
        tokens.push(token);
        return page([3]);
      },
      () => true,
      (p) => seen.push(...p.hits.map((h) => h.cursor)),
    );
    expect(tokens).toEqual(["t1"]);
    expect(seen).toEqual([1, 2, 3]);
  });

  it("stops asking once the stream is no longer live", async () => {
    let live = true;
    const pages: number[] = [];
    await streamPages(
      async () => page([1], "a"),
      async () => page([2], "b"),
      () => live,
      (p) => {
        pages.push(p.hits[0].cursor);
        live = false;
      },
    );
    expect(pages).toEqual([1]);
  });

  it("a failed page rejects, and no later page is asked for", async () => {
    let asked = 0;
    await expect(
      streamPages(
        async () => page([1], "a"),
        async () => {
          asked++;
          throw new Error("boom");
        },
        () => true,
        () => undefined,
      ),
    ).rejects.toThrow("boom");
    expect(asked).toBe(1);
  });
});

describe("text matches in a row's text", () => {
  it("matches are case-insensitive and non-overlapping", () => {
    expect(textMatches("Module 7 and module 7", "MODULE 7")).toEqual([
      { start: 0, end: 8 },
      { start: 13, end: 21 },
    ]);
    expect(textMatches("abc", "")).toEqual([]);
  });

  it("the occurrence whose surrounding text is the hit's snippet is the active one", () => {
    const text = "x module 1 here. module 12 there. module 1 end";
    const occ = textMatches(text, "module 1");
    // The third match (at 33) has the snippet "there. module 1 end", found at 26, with the match 7 characters into it.
    expect(pickOccurrence(text, occ, { context: "there. module 1 end", start: 7 })).toEqual(occ[2]);
    // Without a usable snippet the first occurrence is the active one.
    expect(pickOccurrence(text, occ, undefined)).toEqual(occ[0]);
  });
});
