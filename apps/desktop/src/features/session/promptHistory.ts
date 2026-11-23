import type { SessionEvent } from "../../bridge/wire";

/**
 * The prompts of a conversation, oldest first: those of its earlier history (the user messages of a resumed conversation, as the
 * agent's own records hold them) and then the session's own as the daemon journaled them (its `input` events of kind prompt, the first
 * prompt included). Of the session's own only prompts the agent accepted (sent or queued) count; an attachment-only prompt has no
 * text to bring back, and the same prompt twice in a row is one entry.
 */
export function promptsOf(events: readonly SessionEvent[], earlier: readonly SessionEvent[] = []): string[] {
  const out: string[] = [];
  const add = (text: string) => {
    if (text.trim() && out[out.length - 1] !== text) out.push(text);
  };
  for (const e of earlier) if (e.kind === "agent" && e.event?.type === "history" && e.event.event.type === "user_message") add(e.event.event.text);
  for (const e of events) {
    if (e.kind !== "input" || e.command.command !== "prompt") continue;
    if (e.status.status !== "sent" && e.status.status !== "queued") continue;
    add(e.command.text);
  }
  return out;
}

/**
 * Walking through the prompts. `depth` counts back from the newest prompt (1 = the newest), so it keeps
 * its meaning when older prompts are loaded in front; `draft` is what the box held when the walk began.
 */
export interface Browse {
  depth: number;
  draft: string;
}

/** One step back to an older prompt; `"end"` when there is none among `prompts`. */
export function older(prompts: readonly string[], browse: Browse | null, text: string): { browse: Browse; text: string } | "end" {
  const depth = (browse?.depth ?? 0) + 1;
  if (depth > prompts.length) return "end";
  return { browse: { depth, draft: browse ? browse.draft : text }, text: prompts[prompts.length - depth] };
}

/** One step forward: to a newer prompt, or back to the stashed draft (`browse: null`) after the newest. */
export function newer(prompts: readonly string[], browse: Browse): { browse: Browse | null; text: string } {
  if (browse.depth <= 1 || browse.depth - 1 > prompts.length) return { browse: null, text: browse.draft };
  const depth = browse.depth - 1;
  return { browse: { depth, draft: browse.draft }, text: prompts[prompts.length - depth] };
}

type Caret = { value: string; selectionStart: number; selectionEnd: number };

/** The caret is at the very start of the text, nothing selected: Up may leave the box. */
export const caretAtStart = (el: Caret) => el.selectionStart === 0 && el.selectionEnd === 0;
/** The caret is at the very end of the text, nothing selected: Down may leave the box. */
export const caretAtEnd = (el: Caret) => el.selectionStart === el.value.length && el.selectionEnd === el.value.length;

/**
 * A recalled one-line prompt has no other line for Up / Down to move to, so while walking through the
 * history (the text still exactly as recalled) the arrows work wherever the caret is: otherwise Up then
 * Down would need Home / End in between. A recalled prompt of several lines keeps the strict rule.
 */
const walkingOneLine = (el: Caret, browsing: boolean) => browsing && !el.value.includes("\n") && el.selectionStart === el.selectionEnd;

/** Up may step back to an earlier prompt. */
export const mayStepBack = (el: Caret, browsing: boolean) => caretAtStart(el) || walkingOneLine(el, browsing);
/** Down may step forward to a newer prompt (or the stashed draft). */
export const mayStepForward = (el: Caret, browsing: boolean) => caretAtEnd(el) || walkingOneLine(el, browsing);
