import type { DiagTailParams, HostId } from "../bridge/wire";

/**
 * One record of a daemon's diagnostics log, read from the NDJSON line the `diag.tail` topic
 * delivers: the header every record has (`b, seq, t, m, src, lvl, ev`) and the event's own
 * fields. The daemons write only ids, sizes, timings, states and scrubbed reasons into a record,
 * so nothing here needs redacting; and nothing here may add content (no joining with transcripts).
 */
export interface DiagRecord {
  /** `<boot>.<seq>`: the record's place in its process's log, and the cursor to resume from. */
  id: string;
  /** The writing process's boot id. */
  b: string;
  /** Per-process, strictly increasing: the exact order of the records of one process. */
  seq: number;
  /** Wall clock of the machine that wrote it (ms since the Unix epoch). */
  t: number;
  /** The writing process's monotonic clock (ms since it started). */
  m: number;
  /** Which process wrote it (`hub`, `daemon`, `bridge`, `shim`, `app`…). */
  src: string;
  lvl: string;
  /** The dotted event name (`link.attempt_end`). */
  ev: string;
  /** The event's own fields. */
  fields: Record<string, unknown>;
}

const HEADER = new Set(["b", "seq", "t", "m", "src", "lvl", "ev"]);

/** A record from its NDJSON line; `null` when the line is not one (never throws). */
export function parseRecord(line: string): DiagRecord | null {
  let v: unknown;
  try {
    v = JSON.parse(line);
  } catch {
    return null;
  }
  if (!v || typeof v !== "object" || Array.isArray(v)) return null;
  const o = v as Record<string, unknown>;
  if (typeof o.ev !== "string" || typeof o.seq !== "number" || typeof o.t !== "number") return null;
  const b = typeof o.b === "string" ? o.b : "";
  const fields: Record<string, unknown> = {};
  for (const [k, x] of Object.entries(o)) if (!HEADER.has(k)) fields[k] = x;
  return {
    id: `${b}.${o.seq}`,
    b,
    seq: o.seq,
    t: o.t,
    m: typeof o.m === "number" ? o.m : 0,
    src: typeof o.src === "string" ? o.src : "",
    lvl: typeof o.lvl === "string" ? o.lvl : "info",
    ev: o.ev,
    fields,
  };
}

/** The string value of a field, when it is one. */
export const fieldStr = (r: DiagRecord, k: string): string | undefined => (typeof r.fields[k] === "string" ? (r.fields[k] as string) : undefined);

/** The label the hub gives a host's link records (`link` of `link.*`, `transport.*`). */
export const hostLink = (host: HostId) => `hosts:${host}`;

/**
 * Whether `r` matches `f` the way the daemon's `diag.tail` filter does: every filter given must
 * match, and a filter on a key excludes the records that have none. The app checks every record
 * it receives itself, so what a panel shows is what it asked for whether or not the transport
 * carried the filter to the daemon.
 */
export function matchesFilter(r: DiagRecord, f: DiagTailParams): boolean {
  if (f.host != null && fieldStr(r, "host") !== f.host) return false;
  if (f.session != null && fieldStr(r, "session") !== f.session) return false;
  if (f.traces?.length) {
    const t = fieldStr(r, "trace");
    if (t === undefined || !f.traces.includes(t)) return false;
  }
  if (f.ev_prefix != null && !r.ev.startsWith(f.ev_prefix)) return false;
  return true;
}

/**
 * What a panel keeps of the records it is given, beyond the daemon's filter (which cannot say
 * these): the records about one host (its `host` field, or the `hosts:<id>` label its link
 * records carry), and only some kinds of event (a name, or a prefix ending in `.`).
 */
export interface TailKeep {
  about?: HostId;
  events?: readonly string[];
}

export function keeps(r: DiagRecord, k: TailKeep | undefined): boolean {
  if (!k) return true;
  if (k.about !== undefined && fieldStr(r, "host") !== k.about && fieldStr(r, "link") !== hostLink(k.about)) return false;
  if (k.events && !k.events.some((e) => (e.endsWith(".") ? r.ev.startsWith(e) : r.ev === e))) return false;
  return true;
}
