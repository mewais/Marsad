import type { EarlierGaps, EarlierHistory, EarlierUnavailable } from "../../bridge/wire";
import { formatBytes } from "../media/fetch";

/**
 * The words for what a resumed conversation's earlier history is and what it leaves out. Everything here is a pure function of
 * what the daemon reported (`snapshot.earlier`); nothing is inferred, and nothing the daemon did not say is worded.
 */

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
const was = (n: number) => (n === 1 ? "was" : "were");
const is = (n: number) => (n === 1 ? "is" : "are");

/** Why the earlier conversation cannot be shown, as a sentence. A reason this build does not know is said generically. */
export function unavailableWords(r: EarlierUnavailable): string {
  const detail = (d: unknown) => (typeof d === "string" && d ? ` (${d})` : "");
  switch (r.kind) {
    case "not_offered":
      return "This agent does not give out its earlier conversation, so it cannot be shown here.";
    case "source_missing":
      return "The agent's file for this conversation could not be found, so the earlier conversation cannot be shown.";
    case "source_unreadable":
      return `The agent's file for this conversation could not be read${detail(r.detail)}, so the earlier conversation cannot be shown.`;
    case "too_large":
      return `The agent's file for this conversation is too large to read${typeof r.bytes === "number" && typeof r.limit === "number" ? ` (${formatBytes(r.bytes)}; the limit is ${formatBytes(r.limit)})` : ""}, so the earlier conversation cannot be shown.`;
    case "format_not_understood":
      return `Marsad does not understand the agent's file for this conversation${typeof r.record === "number" ? ` (the first line it could not read is number ${r.record})` : ""}, so the earlier conversation cannot be shown.`;
    case "failed":
      return `Loading the earlier conversation failed${detail(r.detail)}.`;
    default:
      return "The earlier conversation could not be loaded.";
  }
}

/** What the loaded history leaves out, one sentence per kind of gap (empty when nothing is). The process ending early is not here: it is about the newest part (see `earlierNotes`). */
export function gapWords(g: EarlierGaps): string[] {
  const out: string[] = [];
  if (g.omitted > 0) out.push(`${plural(g.omitted, "older message")} ${g.omitted === 1 ? "exists" : "exist"} in the agent's own history and ${is(g.omitted)} not shown.`);
  if (g.shortened > 0) out.push(`${plural(g.shortened, "item")} ${was(g.shortened)} too large to keep in full and ${is(g.shortened)} shortened, or ${g.shortened === 1 ? "has" : "have"} images or files left out.`);
  if (g.unreadable > 0) out.push(`${plural(g.unreadable, "line")} of the agent's file could not be read and ${is(g.unreadable)} missing.`);
  if (g.unrecognised > 0) {
    const kinds = g.unrecognised_kinds.length ? ` (${g.unrecognised_kinds.join(", ")})` : "";
    out.push(`${plural(g.unrecognised, "record")} ${was(g.unrecognised)} not understood${kinds} and ${is(g.unrecognised)} missing.`);
  }
  if (g.missing_files > 0) out.push(`${plural(g.missing_files, "file")} of the conversation (a subagent's own transcript) could not be read, so ${g.missing_files === 1 ? "that part is" : "those parts are"} missing.`);
  if (g.truncated_tail) out.push("The last line of the agent's file was cut off while the agent was writing it, so the very last message may be missing.");
  return out;
}

/** The notice that sits where the missing part is (just above the divider): the history is arriving, cannot arrive, or stopped arriving. */
export type JointNote = { tone: "loading" | "unavailable" | "interrupted"; text: string };

export interface EarlierNotes {
  /** What the history leaves out, at the top of it. */
  top: string[];
  /** What is wrong with or still to come of its newest part, at its lower end. */
  joint: JointNote | null;
}

/** The notices of an earlier history: nothing for `none` and for one that arrived complete. */
export function earlierNotes(e: EarlierHistory): EarlierNotes {
  const top = gapWords(e.gaps);
  switch (e.state.kind) {
    case "none":
      return { top: [], joint: null };
    case "loading":
      return { top, joint: { tone: "loading", text: e.items > 0 ? `Loading the earlier conversation… ${plural(e.items, "item")} so far` : "Loading the earlier conversation…" } };
    case "loaded":
      return { top, joint: e.gaps.interrupted ? { tone: "interrupted", text: "The agent's process ended before all of the earlier conversation arrived, so its newest part is missing here. It carries on the next time the session starts." } : null };
    case "unavailable":
      return { top, joint: { tone: "unavailable", text: `${unavailableWords(e.state.reason)}${e.items > 0 ? ` ${plural(e.items, "item")} arrived before that and ${is(e.items)} shown.` : ""}` } };
    default:
      return { top, joint: { tone: "unavailable", text: "The earlier conversation is in a state this version does not know." } };
  }
}

/** Whether the transcript has an earlier part at all: the history is coming, is here, or could not come. */
export const hasEarlier = (e: EarlierHistory | undefined): e is EarlierHistory => !!e && e.state.kind !== "none";
