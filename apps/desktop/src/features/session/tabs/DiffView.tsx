import { memo, useMemo } from "react";
import type { GitDiff } from "../../../bridge/wire";
import { Icon } from "../../../ui";
import { Tokens, useHighlighted } from "../../highlight/Code";
import { languageFromName } from "../../highlight/lang";
import type { Line } from "../../highlight/tokens";
import { diffSides, parseDiff, splitRows, type DiffLine } from "./diff";

const sign = (l?: DiffLine) => (l?.kind === "add" ? "+" : l?.kind === "del" ? "-" : "");
const cls = (l?: DiffLine) => (l?.kind === "add" ? " a" : l?.kind === "del" ? " d" : "");

/**
 * Renders `git.diff` text unified or side by side, coloured by the file's language (the app's one
 * highlighter: the old and the new side are each highlighted as a whole, so strings and comments
 * that span lines stay right); says when the host cut it short or it is binary.
 */
export const DiffView = memo(function DiffView({ diff, split }: { diff: GitDiff; split: boolean }) {
  const hunks = useMemo(() => parseDiff(diff.text), [diff.text]);
  const sides = useMemo(() => diffSides(hunks), [hunks]);
  const lang = languageFromName(diff.path);
  const o = useHighlighted(sides.oldText, lang);
  const n = useHighlighted(sides.newText, lang);
  /** A line's tokens: deletions from the old side, everything else from the new one. */
  const text = (l?: DiffLine) => {
    if (!l) return "";
    const pos = sides.at.get(l);
    const t: Line | undefined = l.kind === "del" ? o.lines[pos?.o ?? -1] : n.lines[pos?.n ?? -1];
    return t ? <Tokens line={t} /> : l.text;
  };
  if (diff.binary) return <div className="diff-note"><Icon name="file" size="sm" />Binary file: no text diff to show.</div>;
  if (!hunks.length) return <div className="diff-note"><Icon name="file" size="sm" />No line changes (mode or rename only).</div>;
  return (
    <div className={`diff${split ? " split" : ""}`} role="table" aria-label={`Diff of ${diff.path}`} data-lang={n.lang ?? o.lang ?? ""}>
      {hunks.map((h, hi) => (
        <div key={hi} role="rowgroup">
          <div className="hunk" role="row">{h.header}</div>
          {split
            ? splitRows(h).map((r, i) => (
                <div key={i} className="sl" role="row">
                  <span className={`ln${cls(r.left)}`}><span>{r.left?.old ?? ""}</span><span>{sign(r.left)}</span><span>{text(r.left)}</span></span>
                  <span className={`ln${cls(r.right)}`}><span>{r.right?.new ?? ""}</span><span>{sign(r.right)}</span><span>{text(r.right)}</span></span>
                </div>
              ))
            : h.lines.map((l, i) => (
                <div key={i} className={`ln u${cls(l)}`} role="row"><span>{l.old ?? ""}</span><span>{l.new ?? ""}</span><span>{sign(l)}</span><span>{text(l)}</span></div>
              ))}
        </div>
      ))}
      {diff.truncated ? <div className="diff-note warn" role="note"><Icon name="warn" size="sm" />This diff is longer than Marsad shows at once; it was cut at a line boundary. Open the file on the host to see the rest.</div> : null}
    </div>
  );
});
