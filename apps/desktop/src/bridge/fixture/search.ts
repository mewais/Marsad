import type { SearchHit, SearchPage, SearchRequest, SessionEvent } from "../wire";
import { err } from "./runtime";
import { historyAdmits } from "./work";

/*
 * `session.search` over the fixture's events: a port of the daemon's rules (`wasit-runtime/src/search.rs`). What each event
 * contributes to find (`searchable`) and how a query matches it are the same as the daemon's, so the app behaves the same on the
 * fixture as on a real session. The daemon's page budget and resume token are kept too.
 */

export type Field = "user_text" | "assistant_text" | "thinking" | "tool_input" | "tool_result" | "notice";
type Piece = [Field, string];

/** Most hits one page returns. */
const MAX_HITS = 50;
/** Bytes of records one page may examine (the daemon's budget; the fixture's size is the JSON length of each record). */
const SCAN_BUDGET = 8 << 20;
/** Characters of context on each side of a hit. */
const RADIUS = 40;

type Json = unknown;
type Loose = Record<string, unknown>;

/** The text an event contributes to find, tagged by field, in the order the event holds it (empty texts are not listed). */
export function searchable(event: SessionEvent): Piece[] {
  const out: Piece[] = [];
  const e = event as unknown as Loose;
  if (e.kind === "agent") agent(e.event as Loose | undefined, out);
  else if (e.kind === "input") {
    const c = e.command as Loose;
    if (c?.command === "prompt") push(out, "user_text", c.text);
  } else if (e.kind === "notice") push(out, "notice", e.message);
  return out;
}

function agent(a: Loose | undefined, out: Piece[]) {
  if (!a) return;
  switch (a.type) {
    case "user_message":
      return push(out, "user_text", a.text);
    case "user_content":
      return parts(out, "user_text", a.parts);
    case "assistant_delta":
      return push(out, "assistant_text", a.text);
    case "assistant_content":
      return parts(out, "assistant_text", a.parts);
    case "thinking_delta":
      return push(out, "thinking", a.text);
    case "tool_call_started":
      return strings(out, "tool_input", a.input);
    case "tool_call_updated":
      if (a.input != null) strings(out, "tool_input", a.input);
      for (const block of (a.content as Json[] | undefined) ?? []) strings(out, "tool_result", block);
      return parts(out, "tool_result", a.parts);
    case "tool_call_finished":
      if (a.result != null) strings(out, "tool_result", a.result);
      return parts(out, "tool_result", a.parts);
    case "notice":
      return push(out, "notice", a.message);
    case "history":
    case "scoped":
      return agent(a.event as Loose | undefined, out);
  }
}

function push(out: Piece[], field: Field, text: unknown) {
  if (typeof text === "string" && text !== "") out.push([field, text]);
}

function parts(out: Piece[], field: Field, list: unknown) {
  for (const p of (list as Loose[] | undefined) ?? []) if (p.type === "text") push(out, field, p.text);
}

/** Every string inside `value`, depth first; object keys and string values under a `type` key are structure, not text. */
function strings(out: Piece[], field: Field, value: Json) {
  if (typeof value === "string") push(out, field, value);
  else if (Array.isArray(value)) for (const item of value) strings(out, field, item);
  else if (value && typeof value === "object") {
    for (const [k, item] of Object.entries(value)) {
      if (k === "type" && typeof item === "string") continue;
      strings(out, field, item);
    }
  }
}

/** Case-insensitive, character-wise: each character folds to its one-character lowercase form (else itself). */
const fold = (c: string): string => {
  const l = c.toLowerCase();
  return [...l].length === 1 ? l : c;
};

/** The non-overlapping matches of `query` in `text`, as character (code point) offsets. An empty query matches nothing. */
export function findAll(text: string, query: string): { start: number; end: number }[] {
  const q = [...query].map(fold);
  if (q.length === 0) return [];
  const t = [...text].map(fold);
  const spans: { start: number; end: number }[] = [];
  for (let i = 0; i + q.length <= t.length; ) {
    if (q.every((c, j) => t[i + j] === c)) {
      spans.push({ start: i, end: i + q.length });
      i += q.length;
    } else i++;
  }
  return spans;
}

/** Replace control characters with spaces (one for one). */
const clean = (chars: string[]) => chars.map((c) => (/\p{Cc}/u.test(c) ? " " : c)).join("");

/** The text around one match: `RADIUS` characters each side, control characters shown as spaces (one for one). */
function snippetOf(chars: string[], span: { start: number; end: number }): { text: string; start: number; end: number } {
  const from = Math.max(0, span.start - RADIUS);
  const to = Math.min(chars.length, span.end + RADIUS);
  return { text: clean(chars.slice(from, to)), start: span.start - from, end: span.end - from };
}

/** The hits of one event, in the order of its pieces; an empty query (with `kinds`) lists every matching piece once. */
function hitsIn(e: SessionEvent, query: string, kinds: readonly string[] | undefined, part: SearchHit["part"]): SearchHit[] {
  const out: SearchHit[] = [];
  for (const [field, text] of searchable(e)) {
    if (kinds && !kinds.includes(field)) continue;
    const chars = [...text];
    if (!query) {
      out.push({ cursor: e.cursor, part, field, snippet: clean(chars.slice(0, RADIUS * 2)), start: 0, len: 0, at: e.t });
      continue;
    }
    for (const span of findAll(text, query)) {
      const s = snippetOf(chars, span);
      out.push({ cursor: e.cursor, part, field, snippet: s.text, start: s.start, len: s.end - s.start, at: e.t });
    }
  }
  return out;
}

/**
 * One page of `session.search` over `events` (one log, in cursor order): the same rules as the daemon's. Backward walks from the
 * anchor (exclusive) towards the start, forward from the anchor towards the end. A resume token continues a page: its record is
 * examined again, its first `skip` hits already returned.
 */
export function searchFixture(events: readonly SessionEvent[], r: SearchRequest): SearchPage {
  if (r.kinds && r.kinds.length === 0) throw err("invalid_params", "kinds names no field", { marsad_error: "invalid_request", message: "kinds names no field" });
  if (!r.query && !r.kinds) throw err("invalid_params", "the query is empty", { marsad_error: "invalid_request", message: "the query is empty" });
  if (r.resume && r.anchor != null) throw err("invalid_params", "give either an anchor or a resume token, not both", { marsad_error: "invalid_request", message: "give either an anchor or a resume token, not both" });
  const token = r.resume ? parseToken(r.resume) : null;
  const limit = Math.min(MAX_HITS, Math.max(1, r.limit));
  const scope = r.scope ?? { kind: "all" };
  const backward = r.direction !== "forward";
  const list = backward
    ? events.filter((e) => (token ? e.cursor <= token.cursor : r.anchor == null || e.cursor < r.anchor)).reverse()
    : events.filter((e) => (token ? e.cursor >= token.cursor : r.anchor == null || e.cursor > r.anchor));

  const hits: SearchHit[] = [];
  let scanned = 0;
  let last: { cursor: number; skip: number } | null = null;
  for (const e of list) {
    if (hits.length >= limit || scanned >= SCAN_BUDGET) return { hits, next: last ? encode(last) : undefined, scanned_bytes: scanned };
    const bytes = JSON.stringify(e).length;
    scanned += bytes;
    const found = historyAdmits(scope, e) ? hitsIn(e, r.query, r.kinds ?? undefined, r.part) : [];
    const skip = token && token.cursor === e.cursor ? token.skip : 0;
    for (let i = skip; i < found.length; i++) {
      if (hits.length >= limit) return { hits, next: encode({ cursor: e.cursor, skip: i }), scanned_bytes: scanned };
      hits.push(found[i]);
    }
    last = { cursor: e.cursor, skip: found.length };
  }
  return { hits, scanned_bytes: scanned };
}

function encode(t: { cursor: number; skip: number }): string {
  return `${t.cursor}.${t.skip}`;
}

function parseToken(token: string): { cursor: number; skip: number } {
  const [c, s] = token.split(".");
  const cursor = Number(c);
  const skip = Number(s);
  if (!Number.isInteger(cursor) || !Number.isInteger(skip)) throw err("invalid_params", `not a resume token: ${token}`, { marsad_error: "invalid_request", message: `not a resume token: ${token}` });
  return { cursor, skip };
}
