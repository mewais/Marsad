import type { PendingFile } from "./attachments";

/** A message and attachments that did not get sent (a launch that created the session first). */
export interface Draft {
  text: string;
  files: PendingFile[];
}

const drafts = new Map<string, Draft>();

/** Keep a draft for a session's composer to pick up when it opens. */
export function setDraft(sessionKey: string, d: Draft) {
  drafts.set(sessionKey, d);
}

/** The draft of a session, if any (reading does not remove it: a render may run twice). */
export function peekDraft(sessionKey: string): Draft | undefined {
  return drafts.get(sessionKey);
}

/** Forget a draft once the composer has taken it over. */
export function clearDraft(sessionKey: string) {
  drafts.delete(sessionKey);
}

/** The composers that are open, by session: each takes a quote into its own text while it is open. */
const quoteListeners = new Map<string, Set<(quote: string) => void>>();

/** Call `cb` with each quote sent to `sessionKey`'s reply while its composer is open; returns the unsubscribe. */
export function onQuote(sessionKey: string, cb: (quote: string) => void): () => void {
  const set = quoteListeners.get(sessionKey) ?? new Set<(quote: string) => void>();
  quoteListeners.set(sessionKey, set);
  set.add(cb);
  return () => {
    set.delete(cb);
    if (!set.size) quoteListeners.delete(sessionKey);
  };
}

/** `text` as a markdown quote: every line behind `> `, blank lines kept as a bare `>`. */
export function quoteLines(text: string): string {
  return text
    .replace(/\r\n?/g, "\n")
    .trimEnd()
    .split("\n")
    .map((l) => (l.trim() ? `> ${l}` : ">"))
    .join("\n");
}

/** `reply` with `quote` after it (a blank line between), ready for the reader to write under it. */
export function appendQuote(reply: string, quote: string): string {
  const base = reply.trim() ? `${reply.replace(/\s+$/, "")}\n\n` : "";
  return `${base}${quoteLines(quote)}\n`;
}

/**
 * Put `quote` into the session's reply. An open composer takes it into its own text (what it holds, and its attachments, stay as they
 * are). Without one, the quote is kept in the session's draft, which the composer takes over when it opens, with the draft's attachments
 * unchanged. Returns whether an open composer took it.
 */
export function quoteInReply(sessionKey: string, quote: string): boolean {
  const open = quoteListeners.get(sessionKey);
  if (open?.size) {
    open.forEach((cb) => cb(quote));
    return true;
  }
  const cur = peekDraft(sessionKey);
  setDraft(sessionKey, { text: appendQuote(cur?.text ?? "", quote), files: cur?.files ?? [] });
  return false;
}
