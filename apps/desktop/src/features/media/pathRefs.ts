/**
 * Paths mentioned in prose, and which of them are sensitive. Pure: existence is checked on the
 * session's host (`session.files.stat`), never guessed here.
 */
export interface PathRef {
  /** The path as written (without a `:line` suffix). */
  path: string;
  line?: number;
  col?: number;
}

const KNOWN_EXT = /\.(svg|png|jpe?g|gif|webp|pdf|html?|md|json|csv|tsv|txt|log|py|ts|tsx|js|jsx|mjs|rs|go|sh|toml|ya?ml|mp3|mp4|wav|css|java|c|h|cpp|rb|php|sql|lock|xml|ini|cfg|conf|env)$/i;
const SEGMENT = /^[\w@+%~.#=,-]+$/;
const WIN_ABS = /^(?:[A-Za-z]:[\\/]|\\\\[^\\/\s]+[\\/][^\\/\s]+)/;

/** Trailing `:12`, `:12:5` or `#L12` (also `#L12-L20`). */
const LOC = /^(.*?)(?::(\d{1,7})(?::(\d{1,5}))?|#L(\d{1,7})(?:-L?\d+)?)$/;

/**
 * A path in one whitespace-free token, or null: absolute (`/x`, `~/x`, `C:\x` on Windows
 * hosts), relative (`./x`, `../x`, `src/x`) or a bare file name with a known extension; with an
 * optional `:line[:col]` / `#L<line>`.
 */
export function parsePathToken(token: string, windows = false): PathRef | null {
  let t = token.replace(/[.,:;!?]+$/, "");
  if (!t || t.length > 400 || t.includes("://") || /^(mailto|data|javascript):/i.test(t)) return null;
  let line: number | undefined;
  let col: number | undefined;
  const m = LOC.exec(t);
  if (m && m[1] && !/^[A-Za-z]$/.test(m[1])) {
    t = m[1];
    line = Number(m[2] ?? m[4]);
    col = m[3] ? Number(m[3]) : undefined;
    if (!line) return null;
  }
  if (WIN_ABS.test(t)) return windows ? { path: t, line, col } : null;
  if (/^\d+(\.\d+)+$/.test(t) || /^[\d./]+$/.test(t)) return null;
  const abs = t.startsWith("/") || t === "~" || t.startsWith("~/");
  const rel = t.startsWith("./") || t.startsWith("../");
  const parts = t.split("/").filter((p) => p !== "");
  if (!parts.every((p) => p === "." || p === ".." || p === "~" || SEGMENT.test(p))) return null;
  if (abs) {
    if (t === "/" || t.startsWith("//") || (t.length < 3 && t !== "~")) return null;
    return { path: t, line, col };
  }
  if (rel) return parts.length > 1 ? { path: t, line, col } : null;
  if (parts.length === 1 && t.endsWith("/") && parts[0].length >= 3) return { path: t, line, col };
  if (parts.length > 1) {
    // `and/or`, `w/o`: a relative path needs an extension, a trailing slash or some depth.
    const last = parts[parts.length - 1];
    if (/\.\w{1,8}$/.test(last) || t.endsWith("/") || parts.length > 2 || t.length >= 8) return { path: t, line, col };
    return null;
  }
  return KNOWN_EXT.test(t) ? { path: t, line, col } : null;
}

/** Path references of a text, in order of appearance, distinct, at most `max` (default 30). */
export function parsePathRefs(text: string, windows = false, max = 30): PathRef[] {
  const out = new Map<string, PathRef>();
  for (const m of text.matchAll(/[^\s`'"()<>[\]{},;|*]+/g)) {
    const r = parsePathToken(m[0], windows);
    if (!r) continue;
    const k = `${r.path}:${r.line ?? ""}`;
    if (!out.has(k)) out.set(k, r);
    if (out.size >= max) break;
  }
  return [...out.values()];
}
