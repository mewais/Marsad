import { fieldStr, type DiagRecord } from "../../lib/diagRecord";
import { errorWords } from "./words";

/**
 * The connection trail of a host (design §5.3) grouped by connect attempt. The records come from
 * one source, the hub's log, so their order is exact. The ones that carry the attempt's number `n`
 * (`link.attempt_*`, `transport.*`, `boot.*`) go to that attempt. Those that do not (`link.state`,
 * `auth.*`, `hostenv.apply`, `fleet.feed`…) go to the attempt that was open when they were
 * written, or, between attempts, to a group of their own.
 */
export interface Attempt {
  /** The attempt's number; `null` for what happened between attempts. */
  n: number | null;
  /** When it began (the hub's clock), when its `link.attempt_begin` is among the records. */
  begin?: number;
  /** `link.attempt_end`'s outcome (`connected`, `backoff`, `park`, `provision`); absent while it is under way (or its end is not among the records). */
  outcome?: string;
  /** How long it took, from `link.attempt_end`. */
  ms?: number;
  /** Why it ended, in the words of `link.attempt_end`'s error. */
  error?: string;
  /** What `boot.failed` recorded: the stage, the exit code, the scrubbed error and what the host's shell wrote. */
  failure?: { stage?: string; kind?: string; exit?: number; stderr?: string; error?: string };
  records: DiagRecord[];
}

const numOf = (r: DiagRecord): number | undefined => (typeof r.fields.n === "number" ? r.fields.n : undefined);

export function connectionAttempts(records: readonly DiagRecord[]): Attempt[] {
  const out: Attempt[] = [];
  let open: Attempt | null = null;
  const attemptFor = (n: number): Attempt => {
    for (let i = out.length - 1; i >= 0; i--) if (out[i].n === n) return out[i];
    const a: Attempt = { n, records: [] };
    out.push(a);
    return a;
  };
  for (const r of records) {
    const n = numOf(r);
    if (r.ev === "link.attempt_begin" && n !== undefined) {
      open = { n, begin: r.t, records: [r] };
      out.push(open);
      continue;
    }
    if (n !== undefined) {
      const a = attemptFor(n);
      a.records.push(r);
      if (r.ev === "link.attempt_end") {
        a.outcome = fieldStr(r, "outcome");
        if (typeof r.fields.ms === "number") a.ms = r.fields.ms;
        const e = r.fields.error;
        if (e !== undefined && e !== null) a.error = errorWords(e);
        if (open === a) open = null;
      } else if (r.ev === "boot.failed") {
        const f = r.fields;
        a.failure = {
          stage: fieldStr(r, "stage"),
          kind: fieldStr(r, "kind"),
          exit: typeof f.exit === "number" ? f.exit : undefined,
          stderr: fieldStr(r, "stderr"),
          error: f.error !== undefined && f.error !== null ? errorWords(f.error) : undefined,
        };
      }
      continue;
    }
    if (open) {
      open.records.push(r);
      continue;
    }
    const last = out[out.length - 1];
    if (last && last.n === null) last.records.push(r);
    else out.push({ n: null, records: [r] });
  }
  return out;
}
