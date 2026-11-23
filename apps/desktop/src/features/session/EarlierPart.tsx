import type { EarlierHistory } from "../../bridge/wire";
import type { DriverDescriptor } from "../../ui";
import { Button, Icon } from "../../ui";
import { earlierNotes } from "./earlierWords";
import { TranscriptItem } from "./TranscriptView";
import type { RowItem } from "./toolRuns";

/** The line between the conversation's earlier history and what this session did. */
export const EARLIER_DIVIDER = "Earlier in this conversation — before this session";

/**
 * The conversation's earlier history of a resumed session, above the session's own transcript: what it leaves out at its top,
 * its items (the same rows as the session's own, a little quieter), what is still to come or cannot come where it ends, and the
 * divider. The words come from what the daemon reported (`earlier`); an empty history says why it is empty.
 */
export function EarlierPart({ earlier, rows, driver, error, retrying, onRetry }: {
  earlier: EarlierHistory;
  rows: readonly RowItem[];
  driver?: DriverDescriptor;
  /** Fetching the items failed (the daemon's or the link's words). */
  error?: string;
  retrying: boolean;
  onRetry: () => void;
}) {
  const notes = earlierNotes(earlier);
  return (
    <>
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
        {rows.map((it) => <TranscriptItem key={it.key} item={it} driver={driver} />)}
        {notes.joint ? (
          <div className="hist-note" role="status" data-testid="earlier-joint" data-tone={notes.joint.tone}>
            <Icon name={notes.joint.tone === "loading" ? "working" : "warn"} size="xs" spin={notes.joint.tone === "loading"} />
            <div>{notes.joint.text}</div>
          </div>
        ) : null}
      </div>
      <div className="hist-sep" role="separator" data-testid="earlier-divider"><span>{EARLIER_DIVIDER}</span></div>
    </>
  );
}
