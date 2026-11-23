import type { ExportSource, ExportSourceState, ExportStatus } from "../../bridge/wire";
import type { HostView } from "../../state";
import { sizeWords, type Tone } from "./words";

/** The windows an export can cover, newest first is the default. */
export const WINDOWS = [
  { id: "24h", label: "Last 24 hours", ms: 24 * 3_600_000 },
  { id: "3d", label: "Last 3 days", ms: 3 * 24 * 3_600_000 },
  { id: "7d", label: "Last 7 days", ms: 7 * 24 * 3_600_000 },
] as const;
export type WindowId = (typeof WINDOWS)[number]["id"];

/** What one source of an export is called: the app, this computer, or a host by its name (its id when the fleet has forgotten it). */
export function sourceName(s: ExportSource, hosts: readonly HostView[]): string {
  if (s.source === "app") return "This app";
  if (!s.host) return s.source;
  const h = hosts.find((x) => x.id === s.host);
  return h ? (h.local ? "This computer" : h.name) : s.host;
}

const STATE: Record<ExportSourceState, { word: string; tone: Tone }> = {
  waiting: { word: "Waiting", tone: "plain" },
  reading: { word: "Reading", tone: "info" },
  not_connected: { word: "Not connected", tone: "warn" },
  done: { word: "Done", tone: "ok" },
  partial: { word: "Only partly read", tone: "warn" },
  failed: { word: "Failed", tone: "bad" },
};

/** A source's state in words; the bytes read are shown once there are any. */
export function sourceState(s: ExportSource): { word: string; tone: Tone; bytes: string; reason: string | null } {
  const st = STATE[s.state] ?? { word: String(s.state), tone: "plain" as Tone };
  const quiet = s.state === "waiting" || s.state === "not_connected";
  return { ...st, bytes: quiet && !s.bytes ? "" : sizeWords(s.bytes), reason: s.reason || null };
}

const FINISHED: ExportSourceState[] = ["not_connected", "done", "partial", "failed"];

/** How many of an export's sources have ended (read, or given up on). */
export const finishedSources = (x: ExportStatus): number => x.sources.filter((s) => FINISHED.includes(s.state)).length;

/** The result line of a finished export: its size and whether anything planned is missing. */
export function completeness(x: ExportStatus): { size: string; complete: boolean | null; words: string } {
  const size = typeof x.bytes === "number" ? sizeWords(x.bytes) : "";
  const complete = x.complete ?? null;
  const words =
    complete === true ? "Everything planned is in the file."
    : complete === false ? "Some parts were left out. The file lists what is missing and why."
    : "The hub did not say whether the file is complete.";
  return { size, complete, words };
}
