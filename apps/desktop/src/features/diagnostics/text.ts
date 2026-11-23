import type { TrailItem } from "../../bridge/wire";
import type { DiagRecord } from "../../lib/diagRecord";
import { stepTime, type PromptRow } from "./prompts";
import { clockMs, recordWords, trailWords } from "./words";

export interface PanelText {
  facts: { k: string; v: string | null }[];
  prompts: readonly PromptRow[];
  /** The trail as loaded; `null` when it has not been. */
  trail: readonly TrailItem[] | null;
  /** Older trail entries exist that were not loaded. */
  trailHasMore: boolean;
  /** The host's log for the session as loaded; `null` when it was not opened (or has not arrived). */
  hostLog: readonly DiagRecord[] | null;
  /** When the text was made. */
  at: number;
}

/**
 * The Diagnostics panel of a session as plain text, for a report. Made of the same sentences the
 * panel shows, so it holds what they hold: ids, sizes, timings, states and reasons, and no content.
 * Says what it does not hold (a trail not loaded, older entries, a log not opened) instead of
 * leaving it out silently.
 */
export function panelText(p: PanelText): string {
  const out: string[] = [`Marsad session diagnostics, ${new Date(p.at).toISOString()}`, ""];
  for (const f of p.facts) out.push(`${f.k}: ${f.v ?? "not reported"}`);
  out.push("", "Prompts");
  if (!p.prompts.length) out.push("  none seen");
  for (const r of p.prompts) {
    out.push(`  ${r.input !== null ? `#${r.input}` : "not recorded by the host"} ${r.kind}, ${r.chars} characters${r.attachments ? `, ${r.attachments} attachments, ${r.bytes} bytes` : ""}${r.trace ? `, trace ${r.trace}` : ", no trace id"}`);
    out.push(`    ${r.steps.map((s) => `${s.label} ${stepTime(s)}${s.state === "seen" && s.source ? ` (${s.source})` : ""}${s.detail && s.state === "seen" ? ` [${s.detail}]` : ""}`).join(" > ")}`);
  }
  out.push("", "Timeline (the host's clock)");
  if (p.trail === null) out.push("  not loaded");
  else {
    if (p.trailHasMore) out.push("  (older entries exist and are not included)");
    for (const it of p.trail) out.push(`  ${clockMs(it.t)}  ${trailWords(it)}`);
    if (!p.trail.length) out.push("  no entries");
  }
  out.push("", "Host log for this session");
  if (p.hostLog === null) out.push("  not opened");
  else for (const r of p.hostLog) out.push(`  ${clockMs(r.t)}  ${recordWords(r)}`);
  if (p.hostLog?.length === 0) out.push("  no records");
  return out.join("\n") + "\n";
}
