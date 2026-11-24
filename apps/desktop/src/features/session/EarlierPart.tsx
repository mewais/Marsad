import type { EarlierHistory } from "../../bridge/wire";
import { Button, Icon } from "../../ui";
import { earlierNotes } from "./earlierWords";

/** The line between the conversation's earlier history and what this session did. */
export const EARLIER_DIVIDER = "Earlier in this conversation — before this session";

/**
 * The top of a resumed conversation's earlier history: what it leaves out at its top, and the fetch error if the items failed.
 * The words come from what the daemon reported (`earlier`). It sits at the head of the transcript, above the earlier rows.
 */
export function EarlierTop({ earlier, error, retrying, onRetry }: {
  earlier: EarlierHistory;
  /** Fetching the items failed (the daemon's or the link's words). */
  error?: string;
  retrying: boolean;
  onRetry: () => void;
}) {
  const notes = earlierNotes(earlier);
  return (
    <div className="hist-part" data-testid="earlier-part" data-state={earlier.state.kind}>
      {notes.top.length ? (
        <div className="hist-note" role="status" data-testid="earlier-gaps">
          <Icon name="warn" size="xs" />
          <div>{notes.top.map((t) => <div key={t}>{t}</div>)}</div>
        </div>
      ) : null}
      {error ? (
        <div className="hist-note c-failed" role="alert" data-testid="earlier-error">
          <Icon name="warn" size="xs" />
          <div>The earlier conversation could not be loaded: {error}</div>
          <Button size="sm" variant="ghost" pending={retrying} onClick={onRetry}>Try again</Button>
        </div>
      ) : null}
    </div>
  );
}

/**
 * The end of the earlier history, between its last row and the session's own: what it cannot say about its end (still loading,
 * or cut off), and the divider. An empty history says why it is empty, here.
 */
export function EarlierEnd({ earlier }: { earlier: EarlierHistory }) {
  const notes = earlierNotes(earlier);
  return (
    <div className="hist-part">
      {notes.joint ? (
        <div className="hist-note" role="status" data-testid="earlier-joint" data-tone={notes.joint.tone}>
          <Icon name={notes.joint.tone === "loading" ? "working" : "warn"} size="xs" spin={notes.joint.tone === "loading"} />
          <div>{notes.joint.text}</div>
        </div>
      ) : null}
      <div className="hist-sep" role="separator" data-testid="earlier-divider"><span>{EARLIER_DIVIDER}</span></div>
    </div>
  );
}
