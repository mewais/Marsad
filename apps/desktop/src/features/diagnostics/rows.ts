import type { TrailItem } from "../../bridge/wire";
import type { DiagRecord } from "../../lib/diagRecord";
import type { DiagRow } from "./DiagList";
import { recordFields, recordTone, recordWords, trailFields, trailTone, trailWords } from "./words";

/** A session's trail as list rows, in the host's order. */
export function trailRows(items: readonly TrailItem[]): DiagRow[] {
  return items.map((it) => ({ key: String(it.cursor), t: it.t, tone: trailTone(it), words: trailWords(it), fields: () => trailFields(it) }));
}

/** A daemon's log records as list rows, in the daemon's order; `source` names the writing process when the list should say. */
export function recordRows(records: readonly DiagRecord[], source = false): DiagRow[] {
  return records.map((r) => ({ key: r.id, t: r.t, tone: recordTone(r), words: recordWords(r), source: source ? r.src : undefined, fields: () => recordFields(r) }));
}
