import type { AgentEvent, EarlierGaps, EarlierHistory, EarlierSource, EarlierUnavailable, JsonValue, SessionEvent } from "../wire";
import type { Entry, SessionSim } from "./runtime";
import { SPECS, type Spec } from "./sessions";

/**
 * Fixture-only: a resumed conversation's earlier history, as the engine delivers it (`wasit-runtime` `wire.rs` `EarlierHistory`,
 * ARCHITECTURE.md section 3.8). The items are `History` events with the time the conversation says (`at_ms`), kept apart from the
 * session's own events: they are paged with `session.history {earlier: true}`, never reach the live stream, and each record that brings
 * some also says so there as an `earlier` event (and in the snapshot). Cursors are journal cursors, in the conversation's order.
 *
 * Three sessions are scripted (`?earlier=1`): one whose history arrived whole, one still arriving (it goes on while the fixture runs live;
 * a test steps it), one whose history cannot be had.
 */

const MIN = 60_000;

/** One item of the earlier history: an event of the conversation and when it happened. */
export interface HistoryItem {
  at?: number;
  event: AgentEvent;
}

export const NO_EARLIER: EarlierHistory = { state: { kind: "none" }, items: 0, gaps: emptyGaps() };

export function emptyGaps(): EarlierGaps {
  return { omitted: 0, shortened: 0, unreadable: 0, unrecognised: 0, unrecognised_kinds: [], missing_files: 0, truncated_tail: false, interrupted: false, replays_ignored: 0 };
}

/** What an agent that keeps its history in storage and one that replays it over its protocol report as the source. */
export const sourceOf = (delivery: "none" | "wire" | "storage"): EarlierSource | undefined => (delivery === "storage" ? "agent_storage" : delivery === "wire" ? "agent_replay" : undefined);

/** `rounds` ordinary exchanges ending at `end`, a minute and a half apart: the bulk of a long conversation. */
export function historyRounds(rounds: number, end: number, first = 0): HistoryItem[] {
  const out: HistoryItem[] = [];
  let at = end - rounds * 1.5 * MIN;
  const item = (event: AgentEvent, dt = 12_000): void => {
    at += dt;
    out.push({ at, event });
  };
  for (let i = first; i < first + rounds; i++) {
    item({ type: "user_message", text: i % 4 === 0 ? `Round ${i + 1}: please look at module ${i} and tidy the **imports**.` : `Round ${i + 1}: now check the tests for module ${i}.` });
    item({ type: "assistant_delta", text: `Round ${i + 1}: reading module ${i} first.`, message_id: `h${i}a` }, 3000);
    item({ type: "tool_call_started", name: "Read", call_id: `hr${i}`, input: { file_path: `src/module-${i}.ts` } as JsonValue, is_subagent: false }, 1500);
    item({ type: "tool_call_finished", name: "Read", call_id: `hr${i}`, ok: true, result: `// module ${i}\nexport const value${i} = ${i};\n` }, 400);
    item({ type: "assistant_delta", text: `Round ${i + 1}: done, module ${i} is tidy.`, message_id: `h${i}b` }, 2500);
  }
  return out;
}

/** The last part of a conversation, with the kinds of item a real one holds: thinking, a run of tool calls, a subagent's own transcript, a compaction. */
export function historyTail(end: number): HistoryItem[] {
  const out: HistoryItem[] = [];
  let at = end - 12 * MIN;
  const item = (event: AgentEvent, dt = 9_000): void => {
    at += dt;
    out.push({ at, event });
  };
  const sub = { call_id: "h-sub", agent_id: "agent-history-1" };
  item({ type: "user_message", text: "The invoice totals are off by a cent on multi-currency invoices. Find where we round and fix it." });
  item({ type: "thinking_delta", text: "Rounding probably happens per line item and again on the total." }, 4000);
  item({ type: "assistant_delta", text: "I'll look at where **totals** are computed, then at the currency helpers.", message_id: "t1" }, 2000);
  item({ type: "tool_call_started", name: "Grep", call_id: "ht1", input: { pattern: "round\\(", path: "packages/invoices" } as JsonValue, is_subagent: false }, 1500);
  item({ type: "tool_call_finished", name: "Grep", call_id: "ht1", ok: true, result: "packages/invoices/src/totals.ts:41  return Math.round(sum * 100) / 100" }, 500);
  item({ type: "tool_call_started", name: "Read", call_id: "ht2", input: { file_path: "packages/invoices/src/totals.ts" } as JsonValue, is_subagent: false }, 1500);
  item({ type: "tool_call_finished", name: "Read", call_id: "ht2", ok: true, result: "export function total(lines: Line[]) {\n  const sum = lines.reduce((a, l) => a + l.amount, 0)\n  return Math.round(sum * 100) / 100\n}" }, 500);
  item({ type: "tool_call_started", name: "Task", call_id: "h-sub", input: { description: "Find every caller of total()", subagent_type: "Explore" } as JsonValue, is_subagent: true }, 2500);
  item({ type: "scoped", scope: sub, event: { type: "assistant_delta", text: "Searching for callers of `total`.", message_id: "hs1" } }, 1500);
  item({ type: "scoped", scope: sub, event: { type: "tool_call_started", name: "Grep", call_id: "hs-g1", input: { pattern: "total\\(" } as JsonValue, is_subagent: false } }, 1500);
  item({ type: "scoped", scope: sub, event: { type: "tool_call_finished", name: "Grep", call_id: "hs-g1", ok: true, result: "7 matches in 3 files" } }, 700);
  item({ type: "scoped", scope: sub, event: { type: "assistant_delta", text: "Three callers: the invoice list, the PDF and the CSV export.", message_id: "hs2" } }, 1800);
  item({ type: "tool_call_finished", name: "Task", call_id: "h-sub", ok: true, result: "- 3 callers: invoice list, PDF, CSV export.\n- Only the PDF rounds again." }, 1500);
  item({ type: "compaction", info: { trigger: "auto", pre_tokens: 168_400, post_tokens: 41_200, cumulative_dropped_tokens: 127_200, duration_ms: 9_400 } }, 2000);
  item({ type: "assistant_delta", text: "The PDF rounds a second time. I'll round **once**, at the end.", message_id: "t2" }, 2500);
  item({ type: "tool_call_started", name: "Edit", call_id: "ht3", input: { file_path: "packages/invoices/src/pdf.ts", old_string: "Math.round(total * 100) / 100", new_string: "total" } as JsonValue, is_subagent: false }, 1500);
  item({ type: "tool_call_finished", name: "Edit", call_id: "ht3", ok: true, result: "Edited packages/invoices/src/pdf.ts",
    parts: [{ type: "diff", path: "packages/invoices/src/pdf.ts", old_text: "Math.round(total * 100) / 100", new_text: "total" }] }, 500);
  item({ type: "assistant_delta", text: "Fixed: totals are rounded once, in `total()`.", message_id: "t3" }, 2000);
  return out;
}

/** Which of the three scripted histories a spec has. */
export type EarlierKind = "complete" | "loading" | "unavailable";

/** What the engine's import leaves out of the scripted history that arrives with gaps. */
const GAPS: Partial<EarlierGaps> = { omitted: 120, shortened: 2, unrecognised: 3, unrecognised_kinds: ["queue-operation", "file-history-snapshot"] };

export const EARLIER_SPECS: Spec[] = [
  { n: 51, name: "resumed-invoice-rounding", driver: "claude", host: "x1", proj: "ledgerline", branch: "fix/invoice-rounding", agoMin: 30, cost: 0.41, kind: "idle", line: "Resumed from a terminal conversation; its earlier messages come with it", earlier: "complete" },
  { n: 52, name: "resumed-stripe-webhook", driver: "claude", host: "x1", proj: "ledgerline", branch: "fix/stripe-idempotency", agoMin: 20, cost: 0.12, kind: "idle", line: "Resumed from a terminal conversation; its earlier messages are still arriving", earlier: "loading" },
  { n: 53, name: "resumed-orphan-notes", driver: "claude", host: "x1", proj: "ledgerline", branch: "chore/orphan-notes", agoMin: 50, cost: 0.05, kind: "idle", line: "Resumed from a conversation whose file is gone", earlier: "unavailable" },
];

export function applyEarlier() {
  for (const s of EARLIER_SPECS) if (!SPECS.some((x) => x.n === s.n)) SPECS.push(s);
}

/** What a scripted session's earlier history is at the start, and what is still to arrive (`plan`: the next records, oldest first, then how it ends). */
export interface EarlierScript {
  info: EarlierHistory;
  items: HistoryItem[];
  plan?: { chunks: HistoryItem[][]; final: (info: EarlierHistory) => EarlierHistory };
}

/** A scripted session's earlier history as its journal holds it: the state it starts in, the history items with their cursors, what is still to arrive. */
export interface BuiltEarlier {
  info: EarlierHistory;
  items: SessionEvent[];
  plan?: EarlierScript["plan"];
}

/** The script of a spec's earlier history, the conversation having ended at `end`. */
export function earlierScript(spec: Spec, end: number): EarlierScript | undefined {
  const source: EarlierSource = "agent_storage";
  switch (spec.earlier) {
    case "complete": {
      const items = [...historyRounds(28, end - 13 * MIN), ...historyTail(end)];
      return { info: { state: { kind: "loaded", complete: true }, source, items: items.length, gaps: emptyGaps() }, items };
    }
    case "loading": {
      const all = historyRounds(24, end);
      const first = all.slice(0, 40);
      const rest = all.slice(40);
      const chunks = [rest.slice(0, 40), rest.slice(40)];
      return {
        info: { state: { kind: "loading" }, source, items: first.length, gaps: emptyGaps() },
        items: first,
        plan: { chunks, final: (info) => ({ ...info, state: { kind: "loaded", complete: false }, gaps: { ...info.gaps, ...GAPS } }) },
      };
    }
    case "unavailable":
      return { info: { state: { kind: "unavailable", reason: { kind: "source_missing" } }, source, items: 0, gaps: emptyGaps() }, items: [] };
    default:
      return undefined;
  }
}

/** The events of one history record as the journal numbers them: after the record's own event, in order. */
export function itemsAt(base: number, t: number, items: readonly HistoryItem[]): SessionEvent[] {
  return items.map((it, i) => ({ cursor: base + 1 + i, t, kind: "agent", event: { type: "history", at_ms: it.at, event: it.event } }));
}

/**
 * The earlier history of `e` moves: a record brings `items` (oldest first) and the state becomes `next` (its `items` count follows).
 * Like the engine's journal record: one `earlier` event in the live stream (progress, never items) and the items beside it, which only
 * `session.history {earlier: true}` serves. The snapshot follows.
 */
export function journalEarlier(sim: SessionSim, e: Entry, items: readonly HistoryItem[], next: Partial<EarlierHistory> = {}) {
  const info: EarlierHistory = { ...e.snap.earlier, ...next, items: e.snap.earlier.items + items.length };
  sim.emit(e, { kind: "earlier", earlier: info });
  const ev = e.events[e.events.length - 1];
  (e.earlier ??= []).push(...itemsAt(ev.cursor, ev.t, items));
  sim.patch(e, { earlier: info });
}

/**
 * The loading of `e`'s history takes its next step: the next record of items, or at the end the import's report (what it left out) and
 * the final state. A test steps a scripted session with this; a live fixture does it by itself. False when nothing is left.
 */
export function advanceEarlier(sim: SessionSim, e: Entry): boolean {
  const plan = e.earlierPlan;
  if (!plan) return false;
  const chunk = plan.chunks.shift();
  if (chunk) {
    journalEarlier(sim, e, chunk);
    return true;
  }
  e.earlierPlan = undefined;
  const done = plan.final(e.snap.earlier);
  journalEarlier(sim, e, [], { state: done.state, gaps: done.gaps });
  return true;
}

/** A history that fails after some of it arrived, or never could: the reason is the engine's. */
export function failEarlier(sim: SessionSim, e: Entry, reason: EarlierUnavailable) {
  e.earlierPlan = undefined;
  journalEarlier(sim, e, [], { state: { kind: "unavailable", reason } });
}
