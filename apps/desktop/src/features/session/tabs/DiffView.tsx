import { memo, useMemo } from "react";
import type { GitDiff } from "../../../bridge/wire";
import { Icon } from "../../../ui";
import { parseDiff, splitRows, type DiffLine } from "./diff";

const sign = (l?: DiffLine) => (l?.kind === "add" ? "+" : l?.kind === "del" ? "-" : "");
const cls = (l?: DiffLine) => (l?.kind === "add" ? " a" : l?.kind === "del" ? " d" : "");

/** Renders `git.diff` text unified or side by side; says when the host cut it short or it is binary. */
export const DiffView = memo(function DiffView({ diff, split }: { diff: GitDiff; split: boolean }) {
  const hunks = useMemo(() => parseDiff(diff.text), [diff.text]);
  if (diff.binary) return <div className="diff-note"><Icon name="file" size="sm" />Binary file: no text diff to show.</div>;
  if (!hunks.length) return <div className="diff-note"><Icon name="file" size="sm" />No line changes (mode or rename only).</div>;
  return (
    <div className={`diff${split ? " split" : ""}`} role="table" aria-label={`Diff of ${diff.path}`}>
      {hunks.map((h, hi) => (
        <div key={hi} role="rowgroup">
          <div className="hunk" role="row">{h.header}</div>
          {split
            ? splitRows(h).map((r, i) => (
                <div key={i} className="sl" role="row">
                  <span className={`ln${cls(r.left)}`}><span>{r.left?.old ?? ""}</span><span>{sign(r.left)}</span><span>{r.left?.text ?? ""}</span></span>
                  <span className={`ln${cls(r.right)}`}><span>{r.right?.new ?? ""}</span><span>{sign(r.right)}</span><span>{r.right?.text ?? ""}</span></span>
                </div>
              ))
            : h.lines.map((l, i) => (
                <div key={i} className={`ln u${cls(l)}`} role="row"><span>{l.old ?? ""}</span><span>{l.new ?? ""}</span><span>{sign(l)}</span><span>{l.text}</span></div>
              ))}
        </div>
      ))}
      {diff.truncated ? <div className="diff-note warn" role="note"><Icon name="warn" size="sm" />This diff is longer than Marsad shows at once; it was cut at a line boundary. Open the file on the host to see the rest.</div> : null}
    </div>
  );
});
