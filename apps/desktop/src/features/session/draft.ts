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
