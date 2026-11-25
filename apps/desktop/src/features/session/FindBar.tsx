import { useEffect, useRef } from "react";
import { Button, IconButton, useEscape } from "../../ui";
import "./find.css";

/**
 * The find bar of a transcript (Ctrl+F). The hits stream in, newest first; the count reads "3 of 17", with "searching…" while the
 * search runs. Enter steps to the next hit, Shift+Enter to the previous one, Escape closes the bar.
 */
export function FindBar({ query, onQuery, count, active, searching, error, onStep, onClose }: {
  query: string;
  onQuery: (q: string) => void;
  count: number;
  /** The index of the hit the find is on, or -1 when none is. */
  active: number;
  searching: boolean;
  error?: string;
  onStep: (dir: 1 | -1) => void;
  onClose: () => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  // The bar is open while it is mounted: Escape closes it first (before anything the page would do with the key).
  useEscape(true, onClose);

  const status = error
    ? error
    : query === ""
      ? ""
      : count === 0
        ? (searching ? "searching…" : "No matches")
        : `${active + 1} of ${count}${searching ? " · searching…" : ""}`;

  return (
    <div className="tr-find" role="search">
      <input
        ref={input}
        className="tr-find-input"
        aria-label="Find in transcript"
        placeholder="Find in transcript"
        value={query}
        onChange={(e) => onQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          e.preventDefault();
          onStep(e.shiftKey ? -1 : 1);
        }}
      />
      <span className="tr-find-status" role="status" aria-label="Find status" aria-live="polite">{status}</span>
      <Button size="sm" variant="ghost" aria-label="Previous match" disabled={count === 0} onClick={() => onStep(-1)}>Previous</Button>
      <Button size="sm" variant="ghost" aria-label="Next match" disabled={count === 0} onClick={() => onStep(1)}>Next</Button>
      <IconButton icon="x" label="Close find" onClick={onClose} />
    </div>
  );
}
