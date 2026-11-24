import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { SessionEvent, SessionSnapshot } from "../../src/bridge/wire";
import { createTranscriptFold } from "../../src/features/session/transcript";
import { cachePercent, usageSummary } from "../../src/features/session/usage";
import { quietBackend, renderApp, sessionByName } from "../helpers";

async function open(name: string, tab?: string) {
  const backend = quietBackend();
  const s = sessionByName(backend, name);
  const r = await renderApp(`/session/${s.host}/${s.id}${tab ? `?tab=${tab}` : ""}`, backend);
  await screen.findByRole("heading", { name });
  return { ...r, s };
}

const side = () => screen.getByRole("complementary", { name: "Session details" });

describe("session sidebar: what the Usage tab used to show", () => {
  it("has no Usage tab, and an old ?tab=usage link opens Activity", async () => {
    await open("upgrade-tauri-2-5", "usage");
    const tabs = screen.getByRole("tablist", { name: "Session sections" });
    expect(within(tabs).queryByRole("tab", { name: "Usage" })).toBeNull();
    expect(within(tabs).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Activity", "Changes", "Config", "Terminal"]);
    // Activity is selected and its transcript is on screen; the Terminal did not open.
    expect(within(tabs).getByRole("tab", { name: "Activity" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("textbox", { name: "Message" })).toBeInTheDocument();
    expect(screen.queryByRole("tabpanel")).toBeNull();
  });

  it("any other unknown tab falls back to Activity too, not to the Terminal", async () => {
    await open("upgrade-tauri-2-5", "nonsense");
    expect(screen.getByRole("textbox", { name: "Message" })).toBeInTheDocument();
    // Only a section other than Activity has a tab panel (the Terminal is one of them).
    expect(screen.queryByRole("tabpanel")).toBeNull();
    expect(screen.getByRole("tab", { name: "Activity" })).toHaveAttribute("aria-selected", "true");
  });

  it("shows cost, context window, tokens, cache and the per-model cost next to the model and mode", async () => {
    const { s } = await open("migrate-auth-middleware");
    const snap = s.snap;
    const u = usageSummary(snap);
    const el = within(side());
    expect(el.getByText("claude-sonnet-4.5", { selector: "dt" })).toBeInTheDocument();
    expect(el.getByTestId("side-cost")).toHaveTextContent(`$${snap.usage.cost_usd.toFixed(2)}`);
    expect(el.getByTestId("side-cost")).toHaveTextContent(`${snap.turns_completed} turns`);
    const meter = el.getByRole("meter", { name: "Context window used" });
    expect(meter).toHaveAttribute("aria-valuenow", String(Math.round(snap.context_pct!)));
    expect(el.getByTestId("side-context")).toHaveTextContent(`${Math.round(snap.context_pct!)}%`);
    expect(el.getByTestId("side-tokens")).toHaveTextContent("/");
    expect(el.getByTestId("side-cache")).toHaveTextContent(`${u.cachePct}%`);
    const per = within(el.getByTestId("side-per-model"));
    expect(per.getByText("claude-sonnet-4.5")).toBeInTheDocument();
    expect(per.getByText("claude-haiku-4.5")).toBeInTheDocument();
    expect(el.getByText("Cost by model")).toBeInTheDocument();
  });

  it("moves subagents out of the sidebar into the expandable strip above the composer", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const s = [...backend.hostIds()].flatMap((h) => backend.sessionIds(h).map((id) => ({ h, id }))).find(({ h, id }) => backend.snapshot(h, id).subagents.length)!;
    expect(s).toBeDefined();
    await renderApp(`/session/${s.h}/${s.id}`, backend);
    const toggle = await screen.findByTestId("subagent-strip-toggle");
    expect(within(side()).queryByTestId("side-subagents")).toBeNull();
    expect(within(side()).queryByText(/^Subagents/)).toBeNull();
    expect(toggle).toHaveTextContent("Subagents · 1 finished");
    await user.click(toggle);
    const list = screen.getByTestId("subagent-strip-list");
    expect(list).toHaveTextContent("Explore");
    expect(list).toHaveTextContent("Finished");
  });
});

describe("one cache percentage", () => {
  it("is cache reads over every prompt token (uncached + read + written), whole percent", () => {
    expect(cachePercent({ input_tokens: 170, cache_read_tokens: 120, cache_creation_tokens: 9 })).toBe(40);
    expect(cachePercent({ input_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 })).toBe(0);
    expect(cachePercent({ input_tokens: 0, cache_read_tokens: 50, cache_creation_tokens: 0 })).toBe(100);
  });

  it("the sidebar shows exactly usageSummary's figure, taken from the live report once it has caught up", () => {
    const base = { usage: { input_tokens: 100, output_tokens: 10, cache_read_tokens: 100, cache_creation_tokens: 100, cost_usd: 1 }, turns_completed: 2, capabilities: { cost: true, usage: true }, context_pct: 30 } as unknown as SessionSnapshot;
    expect(usageSummary(base)).toMatchObject({ promptTokens: 300, cachePct: 33, cost: 1, turns: 2, contextPct: 30 });
    const live = { ...base, live_usage: { input_tokens: 100, output_tokens: 20, cache_read_tokens: 300, cache_creation_tokens: 100, cost_usd: 1.5 } } as SessionSnapshot;
    expect(usageSummary(live)).toMatchObject({ promptTokens: 500, cachePct: 60, outputTokens: 20, cost: 1.5 });
    // A live report older than the totals does not win.
    const behind = { ...base, live_usage: { input_tokens: 10, output_tokens: 1, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0.1 } } as SessionSnapshot;
    expect(usageSummary(behind).cost).toBe(1);
  });

  it("says when the agent does not report cost or usage instead of showing zeros", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, "docs-search-index");
    // The fixture agents all report; an agent that does not is a snapshot whose capabilities say so.
    const snap = backend.snapshot(s.host, s.id);
    snap.capabilities = { ...snap.capabilities, cost: false, usage: false };
    await renderApp(`/session/${s.host}/${s.id}`, backend);
    await screen.findByRole("heading", { name: "docs-search-index" });
    expect(within(side()).getByTestId("side-cost")).toHaveTextContent(/does not report cost/);
    expect(within(side()).getByTestId("side-tokens")).toHaveTextContent(/does not report usage/);
    expect(within(side()).queryByTestId("side-cache")).toBeNull();
  });
});

describe("compaction and rate-limit notices are transcript items", () => {
  const agent = (cursor: number, event: unknown) => ({ cursor, t: Date.UTC(2026, 0, 1, 12, 0), kind: "agent", event }) as unknown as SessionEvent;
  const compaction = agent(1, { type: "compaction", info: { trigger: "auto", pre_tokens: 168_400, post_tokens: 41_200, cumulative_dropped_tokens: 127_200, duration_ms: 9_400 } });
  const limit = agent(2, { type: "rate_limit_notice", info: { status: "allowed_warning", resets_at: 1_900_000_000, rate_limit_type: "five_hour", overage_status: "allowed", is_using_overage: true } });

  it("fold them in stream order, a rate limit as a warning", () => {
    const items = createTranscriptFold().update([compaction, agent(3, { type: "assistant_delta", text: "hi", message_id: "m" }), limit]);
    expect(items.map((i) => i.kind)).toEqual(["notice", "assistant", "notice"]);
    expect(items[0]).toMatchObject({ kind: "notice", tone: "info", text: expect.stringMatching(/History compacted at .*\(auto\): 168k → 41k tokens in 9\.4s · 127k dropped so far/) });
    expect(items[2]).toMatchObject({ kind: "notice", tone: "warn", text: expect.stringMatching(/Rate limit: allowed_warning \(five_hour\) · resets .* · using overage/) });
  });

  it("are not lost when they arrive live, one at a time (the incremental fold)", () => {
    const fold = createTranscriptFold();
    const a = fold.update([compaction]);
    const b = fold.update([compaction, limit]);
    expect(b).toHaveLength(2);
    expect(b[0]).toBe(a[0]);
  });

  it("show in the session's Activity view without any Usage tab", async () => {
    await open("upgrade-tauri-2-5");
    expect(await screen.findByText(/History compacted at/)).toBeInTheDocument();
    const warn = await screen.findByText(/Rate limit: allowed_warning/);
    expect(warn.closest("[role=status]")).not.toBeNull();
    await waitFor(() => expect(screen.queryByText("Usage")).toBeNull());
  });
});
