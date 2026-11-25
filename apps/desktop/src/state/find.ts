import type { SearchPage } from "../bridge/wire";
import type { RowItem } from "../features/session/toolRuns";

/*
 * Find in a transcript (B6d) and its outline (B7b): the pure parts. A hit is an event's cursor; a row of the transcript is the fold of
 * a cursor range. The index maps a cursor to the row it was folded into, the planner decides how a jump to a cursor is made, and the
 * paging budget bounds how far a jump may page back before it opens a detached window instead.
 */

/** How far a jump may page back into the transcript: 2000 events, or 8 MiB of them, whichever comes first (then a detached window). */
export const PAGING_LIMITS = { events: 2000, bytes: 8 << 20 } as const;

export interface PagingUsed {
  events: number;
  bytes: number;
}

/** Whether a jump may page back once more. */
export function pagingLeft(used: PagingUsed): boolean {
  return used.events < PAGING_LIMITS.events && used.bytes < PAGING_LIMITS.bytes;
}

/** The cursor range one row was folded from: `from` is its first event, `to` its latest. */
export interface Extent {
  key: string;
  row: RowItem;
  from: number;
  to: number;
}

export interface CursorIndex {
  /** The rows in cursor order. */
  readonly extents: readonly Extent[];
  /**
   * The row a cursor was folded into: the last row starting at or before it (a merged delta's later cursors map to the message's
   * row). Undefined only before the first row.
   */
  at(cursor: number): Extent | undefined;
}

/** A row's cursor range: a tool run spans its calls and the thinking between them. */
function spanOf(row: RowItem): { from: number; to: number } {
  if (row.kind !== "toolrun") return row.span;
  let from = Number.POSITIVE_INFINITY;
  let to = Number.NEGATIVE_INFINITY;
  for (const e of row.entries) {
    from = Math.min(from, e.span.from);
    to = Math.max(to, e.span.to);
  }
  return { from, to };
}

/**
 * The index of rows by cursor. `rows` are in cursor order (the fold's order); `keyOf` names a row as the list shows it. Lookup is a
 * binary search over the rows' first cursors.
 */
export function cursorIndex(rows: readonly RowItem[], keyOf: (row: RowItem) => string): CursorIndex {
  const extents: Extent[] = rows.map((row) => {
    const s = spanOf(row);
    return { key: keyOf(row), row, from: s.from, to: s.to };
  });
  return {
    extents,
    at(cursor) {
      let lo = 0;
      let hi = extents.length;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (extents[mid].from <= cursor) lo = mid + 1;
        else hi = mid;
      }
      return lo === 0 ? undefined : extents[lo - 1];
    },
  };
}

/** What is loaded of one list of a transcript: the oldest cursor held (undefined: nothing held), whether the host has older, and whether a page is loading. */
export interface LoadedWindow {
  oldest: number | undefined;
  hasMore: boolean;
  loading: boolean;
}

export type JumpPlan = { kind: "row"; extent: Extent } | { kind: "older" } | { kind: "detached" };

/**
 * How a jump to `cursor` is made: to a row that is loaded (the row it was folded into; a cursor loaded but with no row of its own
 * goes to the row after it), by paging back (the host has older events), or into a detached window (nothing loaded reaches it and
 * the host has nothing more). `wait` while a page is loading, so the plan is made on what is then held.
 */
export function planJump(cursor: number, index: CursorIndex, win: LoadedWindow): JumpPlan | "wait" {
  if (win.loading) return "wait";
  if (win.oldest !== undefined && cursor >= win.oldest) {
    const extent = index.at(cursor) ?? index.extents[0];
    if (extent) return { kind: "row", extent };
  }
  return win.hasMore ? { kind: "older" } : { kind: "detached" };
}

/** The next hit in a list of `count` hits from `active` (-1: none yet), wrapping; -1 when there are none. */
export function stepIndex(active: number, count: number, dir: 1 | -1): number {
  if (count === 0) return -1;
  if (active < 0) return dir > 0 ? 0 : count - 1;
  return (active + dir + count) % count;
}

/**
 * Every page of a search, one after another: `first` asks for the first page, `next` for the page after a resume token. `onPage`
 * gets each page in order, and nothing more is asked for once `live` says the stream is over. A failed page rejects.
 */
export async function streamPages(first: () => Promise<SearchPage>, next: (token: string) => Promise<SearchPage>, live: () => boolean, onPage: (page: SearchPage) => void): Promise<void> {
  let page = await first();
  for (;;) {
    if (!live()) return;
    onPage(page);
    if (!page.next) return;
    page = await next(page.next);
  }
}

/** Case-insensitive, one character at a time (a character whose lowercase is longer than one unit keeps its own case): offsets stay put. */
const foldText = (s: string): string => s.replace(/[\s\S]/g, (c) => (c.toLowerCase().length === 1 ? c.toLowerCase() : c));

/** The non-overlapping matches of `query` in `text`, as UTF-16 offsets. An empty query matches nothing. */
export function textMatches(text: string, query: string): { start: number; end: number }[] {
  const q = foldText(query);
  if (!q) return [];
  const t = foldText(text);
  const out: { start: number; end: number }[] = [];
  let i = t.indexOf(q);
  while (i >= 0) {
    out.push({ start: i, end: i + q.length });
    i = t.indexOf(q, i + q.length);
  }
  return out;
}

/**
 * The match a hit is: the one whose surrounding text is the hit's snippet. `hint.context` is the snippet's text, `hint.start` where its
 * match starts in it. Without a usable hint, the first match.
 */
export function pickOccurrence<T extends { start: number }>(text: string, occ: readonly T[], hint?: { context: string; start: number }): T | undefined {
  if (occ.length === 0) return undefined;
  if (!hint) return occ[0];
  const at = text.indexOf(hint.context);
  if (at < 0) return occ[0];
  const target = at + hint.start;
  return occ.reduce((best, o) => (Math.abs(o.start - target) < Math.abs(best.start - target) ? o : best));
}

/** The expansion id a row's open state is kept under (the ids the transcript's rows use), or none for a row that does not expand. */
export function expansionIdOf(row: RowItem): string | null {
  switch (row.kind) {
    case "tool":
      return `tool:${row.key}`;
    case "toolrun":
      return `run:${row.key}`;
    case "thinking":
      return `think:${row.key}`;
    default:
      return null;
  }
}

/** Rows whose text is rendered as markdown: a hit in one is shown by flashing the row, not by marking its text. */
export const isMarkdownRow = (row: RowItem): boolean => row.kind === "assistant" || row.kind === "media";
