import { pickOccurrence, textMatches } from "../../state/find";

/*
 * In-text marks of the find: the matches of the query in one row's text, drawn with the CSS Custom Highlight API. Ranges are only
 * registered, never inserted into the row, so React's own text nodes are left as they are. Without the API (an older webview) nothing
 * is marked and the caller flashes the row instead.
 */

/** The highlight of every match, and the one of the match the find is on. */
export const MATCH_HIGHLIGHT = "find";
export const CURRENT_HIGHLIGHT = "find-current";

interface HighlightRegistry {
  set(name: string, value: unknown): void;
  delete(name: string): void;
}

function registry(): HighlightRegistry | null {
  const css = (globalThis as { CSS?: { highlights?: HighlightRegistry } }).CSS;
  const HighlightCtor = (globalThis as { Highlight?: unknown }).Highlight;
  return css?.highlights && typeof HighlightCtor === "function" ? css.highlights : null;
}

/** Whether the webview can mark text at all. */
export const canMarkText = (): boolean => registry() !== null;

/** Removes the marks of the find. */
export function clearMarks() {
  const reg = registry();
  reg?.delete(MATCH_HIGHLIGHT);
  reg?.delete(CURRENT_HIGHLIGHT);
}

/**
 * Marks the matches of `query` inside `root`'s text, and the current one (see `pickOccurrence` for `hint`). True when there is at
 * least one match to show.
 */
export function markText(root: Element, query: string, hint?: { context: string; start: number }): boolean {
  const reg = registry();
  if (!reg) return false;
  // The text of the row as one string, with the text node each stretch of it came from.
  const nodes: { node: Text; start: number }[] = [];
  let full = "";
  const walk = (n: Node) => {
    if (n.nodeType === 3) {
      nodes.push({ node: n as Text, start: full.length });
      full += (n as Text).data;
    } else n.childNodes.forEach(walk);
  };
  walk(root);
  const occ = textMatches(full, query);
  if (occ.length === 0) {
    clearMarks();
    return false;
  }
  // The node holding offset `at`: the one where it starts, or (for the end of a match) where it ends.
  const locate = (at: number, end: boolean): [Text, number] => {
    for (let i = nodes.length - 1; i >= 0; i--) {
      const { node, start } = nodes[i];
      if (start <= at && (end ? at > start : true)) return [node, Math.min(at - start, node.data.length)];
    }
    return [nodes[0].node, 0];
  };
  const range = (m: { start: number; end: number }) => {
    const r = document.createRange();
    const [sn, so] = locate(m.start, false);
    const [en, eo] = locate(m.end, true);
    r.setStart(sn, so);
    r.setEnd(en, eo);
    return r;
  };
  const Hl = (globalThis as unknown as { Highlight: new (...r: Range[]) => unknown }).Highlight;
  const current = pickOccurrence(full, occ, hint) ?? occ[0];
  reg.set(MATCH_HIGHLIGHT, new Hl(...occ.map(range)));
  reg.set(CURRENT_HIGHLIGHT, new Hl(range(current)));
  return true;
}
