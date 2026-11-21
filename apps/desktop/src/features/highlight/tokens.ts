/** One styled piece of a line: a highlighter class (empty = plain) and its text. */
export type Token = readonly [cls: string, text: string];
export type Line = readonly Token[];

const ENT: Record<string, string> = { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": '"', "&#x27;": "'", "&#39;": "'" };
const unescape = (s: string) => s.replace(/&(?:amp|lt|gt|quot|#x27|#39);/g, (m) => ENT[m]);

/**
 * Parse the highlighter's output (escaped text and `<span class="hljs-…">` only) into tokens per
 * line, splitting multi-line spans at newlines. Nothing is ever interpreted as markup: text is
 * unescaped into strings and rendered by React, and classes are restricted to `hljs-…` words.
 */
export function parseHighlighted(html: string): Line[] {
  const lines: Token[][] = [[]];
  const stack: string[] = [];
  const re = /<span class="([^"]*)">|<\/span>|([^<]+)/g;
  let m: RegExpExecArray | null;
  const cls = () => stack[stack.length - 1] ?? "";
  while ((m = re.exec(html))) {
    if (m[2] !== undefined) {
      const text = unescape(m[2]);
      const parts = text.split("\n");
      parts.forEach((p, i) => {
        if (i > 0) lines.push([]);
        if (p) lines[lines.length - 1].push([cls(), p]);
      });
    } else if (m[1] !== undefined) {
      const ok = m[1].split(/\s+/).filter((c) => /^hljs-[a-z_-]+$/.test(c));
      // Nested classes (a substitution inside a string) keep the innermost one.
      stack.push(ok[0] ?? cls());
    } else stack.pop();
  }
  return lines;
}

/** Plain lines (no highlighting). */
export function plainLines(code: string): Line[] {
  return code.split("\n").map((l) => (l ? [["", l] as Token] : []));
}
