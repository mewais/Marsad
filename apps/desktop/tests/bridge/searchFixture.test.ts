import { describe, expect, it } from "vitest";
import { methods as M } from "../../src/bridge/wire";
import type { SearchPage, SearchRequest, SessionEvent } from "../../src/bridge/wire";
import { searchable } from "../../src/bridge/fixture/search";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import type { Spec } from "../../src/bridge/fixture/sessions";
import { quietBackend } from "../helpers";

const SPEC: Spec = { n: 901, name: "search-target", driver: "claude", host: "x1", proj: "ledgerline", branch: "main", agoMin: 1, cost: 1, kind: "working", line: "", rounds: 120 };

function session(): { b: FixtureBackend; host: string; id: string } {
  const b = quietBackend({ specs: [SPEC] });
  const host = b.hostIds().find((h: string) => b.sessionIds(h).length > 0) as string;
  const id = b.sessionIds(host).find((x: string) => b.snapshot(host, x).title === SPEC.name) ?? b.sessionIds(host)[0];
  return { b, host, id };
}

/** The default query matches the prompts of modules 1, 10 to 19 and 100 to 119: thirty-one user prompts. */
const req = (r: Partial<SearchRequest> & { session: string }): SearchRequest => ({ query: "summarise module 1", direction: "backward", part: "live", limit: 50, ...r }) as SearchRequest;
const search = (b: FixtureBackend, host: string, r: SearchRequest) => b.request<SearchPage>(M.SESSION_SEARCH, r, { host });

describe("searchable mirrors the Rust rules (the shared fixture's cases)", () => {
  const ev = (e: Record<string, unknown>) => ({ cursor: 1, t: 1, ...e }) as unknown as SessionEvent;

  it("prompts, assistant prose and reasoning are searched by field", () => {
    expect(searchable(ev({ kind: "input", input: 3, command: { command: "prompt", text: "Find the CAFÉ bug", attachments: [] }, status: { status: "sent" } }))).toEqual([["user_text", "Find the CAFÉ bug"]]);
    expect(searchable(ev({ kind: "agent", event: { type: "assistant_delta", text: "Hello wörld" } }))).toEqual([["assistant_text", "Hello wörld"]]);
    expect(searchable(ev({ kind: "agent", event: { type: "thinking_delta", text: "Considering options" } }))).toEqual([["thinking", "Considering options"]]);
  });

  it("empty texts and slash commands contribute no find text; object keys and type labels are not text", () => {
    expect(searchable(ev({ kind: "agent", event: { type: "assistant_delta", text: "" } }))).toEqual([]);
    expect(searchable(ev({ kind: "input", input: 4, command: { command: "run_slash", name: "compact", args: "keep the plan" }, status: { status: "sent" } }))).toEqual([]);
    expect(searchable(ev({ kind: "agent", event: { type: "tool_call_started", name: "Read", call_id: "c", input: { type: "text", file_path: "a.ts" }, is_subagent: false } }))).toEqual([["tool_input", "a.ts"]]);
  });
});

describe("session.search over the fixture's derived log", () => {
  it("a hit names its event, and its snippet marks the match", async () => {
    const { b, host, id } = session();
    const page = await search(b, host, req({ session: id }));
    expect(page.hits.length).toBeGreaterThan(0);
    const hit = page.hits[0];
    expect(hit.field).toBe("user_text");
    expect(hit.part).toBe("live");
    expect(hit.snippet.slice(hit.start, hit.start + hit.len).toLowerCase()).toBe("summarise module 1");
  });

  it("paging with the resume token returns every hit once, newest first", async () => {
    const { b, host, id } = session();
    const all = (await search(b, host, req({ session: id, limit: 50 }))).hits.map((h) => h.cursor);
    const paged: number[] = [];
    let page = await search(b, host, req({ session: id, limit: 2 }));
    paged.push(...page.hits.map((h) => h.cursor));
    while (page.next) {
      page = await search(b, host, req({ session: id, limit: 2, resume: page.next }));
      paged.push(...page.hits.map((h) => h.cursor));
    }
    expect(paged).toEqual(all);
    expect([...paged].sort((x, y) => y - x)).toEqual(paged);
  });

  it("an anchor is exclusive: a backward search from a hit's cursor starts at the hit before it", async () => {
    const { b, host, id } = session();
    const all = (await search(b, host, req({ session: id }))).hits;
    const second = all[1].cursor;
    const after = await search(b, host, req({ session: id, anchor: second }));
    expect(after.hits.map((h) => h.cursor)).toEqual(all.slice(2).map((h) => h.cursor));
  });

  it("kinds restrict the fields matched", async () => {
    const { b, host, id } = session();
    const page = await search(b, host, req({ session: id, query: "module 1", kinds: ["assistant_text"] }));
    expect(page.hits.length).toBeGreaterThan(0);
    expect(page.hits.every((h) => h.field === "assistant_text")).toBe(true);
  });

  it("an empty query is refused, as the daemon refuses it", async () => {
    const { b, host, id } = session();
    await expect(search(b, host, req({ session: id, query: "" }))).rejects.toMatchObject({ message: "the query is empty" });
  });
});
