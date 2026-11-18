/** Unified diff text (as `git.diff` returns it) → rows for unified and side-by-side views. */

export type LineKind = "ctx" | "add" | "del";
export interface DiffLine {
  kind: LineKind;
  old?: number;
  new?: number;
  text: string;
}
export interface Hunk {
  header: string;
  lines: DiffLine[];
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/;

export function parseDiff(text: string): Hunk[] {
  const hunks: Hunk[] = [];
  let cur: Hunk | null = null;
  let o = 0;
  let n = 0;
  for (const raw of text.split("\n")) {
    const m = HUNK.exec(raw);
    if (m) {
      cur = { header: raw, lines: [] };
      hunks.push(cur);
      o = Number(m[1]);
      n = Number(m[2]);
      continue;
    }
    if (!cur || raw.startsWith("\\")) continue;
    if (raw.startsWith("+")) cur.lines.push({ kind: "add", new: n++, text: raw.slice(1) });
    else if (raw.startsWith("-")) cur.lines.push({ kind: "del", old: o++, text: raw.slice(1) });
    else if (raw.startsWith(" ") || raw === "") {
      if (raw === "" && cur.lines.length && text.endsWith("\n") && false) continue;
      cur.lines.push({ kind: "ctx", old: o++, new: n++, text: raw.slice(1) });
    }
  }
  // A trailing empty line from the final "\n" is not a context line.
  for (const h of hunks) {
    const last = h.lines[h.lines.length - 1];
    if (last && last.kind === "ctx" && last.text === "" && h === hunks[hunks.length - 1] && text.endsWith("\n")) h.lines.pop();
  }
  return hunks;
}

export interface SplitRow {
  left?: DiffLine;
  right?: DiffLine;
}

/** Side by side: context on both sides; runs of deletions and additions paired line by line. */
export function splitRows(h: Hunk): SplitRow[] {
  const rows: SplitRow[] = [];
  let dels: DiffLine[] = [];
  let adds: DiffLine[] = [];
  const flush = () => {
    for (let i = 0; i < Math.max(dels.length, adds.length); i++) rows.push({ left: dels[i], right: adds[i] });
    dels = [];
    adds = [];
  };
  for (const l of h.lines) {
    if (l.kind === "del") {
      if (adds.length) flush();
      dels.push(l);
    } else if (l.kind === "add") adds.push(l);
    else {
      flush();
      rows.push({ left: l, right: l });
    }
  }
  flush();
  return rows;
}

export function countLines(hunks: Hunk[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const h of hunks) for (const l of h.lines) l.kind === "add" ? added++ : l.kind === "del" ? removed++ : 0;
  return { added, removed };
}
