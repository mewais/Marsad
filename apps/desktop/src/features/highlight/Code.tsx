import { memo, useEffect, useMemo, useRef, useState } from "react";
import { AUTO_MAX_BYTES, HL_MAX_BYTES, HL_MAX_LINES, languageFromName, languageFromTag } from "./lang";
import { plainLines, type Line } from "./tokens";
import "./highlight.css";

type Engine = typeof import("./engine");
let enginePromise: Promise<Engine> | null = null;
/** The highlighter chunk, loaded on first use. */
export const loadEngine = (): Promise<Engine> => (enginePromise ??= import("./engine"));

/** Small blocks are cached by content so a re-render (or a remount on scroll) never re-highlights. */
const cache = new Map<string, { lang: string | null; lines: Line[] }>();
const CACHE_MAX = 300;
const CACHE_TEXT_MAX = 8 * 1024;
const key = (lang: string | null, text: string) => `${lang ?? ""}\u0000${text}`;

/** Why a block is shown plain. */
export function skipReason(text: string, lang: string | null): "large" | "none" | "auto-large" | null {
  if (lang === "plaintext") return "none";
  if (text.length > HL_MAX_BYTES) return "large";
  let n = 1;
  for (let i = text.indexOf("\n"); i >= 0 && n <= HL_MAX_LINES; i = text.indexOf("\n", i + 1)) n++;
  if (n > HL_MAX_LINES) return "large";
  if (!lang && text.length > AUTO_MAX_BYTES) return "auto-large";
  return null;
}

/** Tokens of `text`: plain at first, highlighted once the engine chunk has run (off the render). */
export function useHighlighted(text: string, lang: string | null): { lines: Line[]; lang: string | null; skipped: "large" | "none" | "auto-large" | null } {
  const skipped = skipReason(text, lang);
  const plain = useMemo(() => plainLines(text), [text]);
  const hit = !skipped ? cache.get(key(lang, text)) : undefined;
  const [state, setState] = useState<{ text: string; lang: string | null; out: { lang: string | null; lines: Line[] } } | null>(null);
  useEffect(() => {
    if (skipped || hit) return;
    let live = true;
    // Defer past the paint of the plain text; the engine itself is a separate chunk.
    const t = setTimeout(() => {
      loadEngine().then((e) => {
        if (!live) return;
        const out = e.highlight(text, lang);
        if (text.length <= CACHE_TEXT_MAX) {
          cache.set(key(lang, text), out);
          while (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value as string);
        }
        setState({ text, lang, out });
      }, () => undefined);
    }, 0);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [text, lang, skipped, hit]);
  if (skipped) return { lines: plain, lang: null, skipped };
  const out = hit ?? (state && state.text === text && state.lang === lang ? state.out : null);
  return { lines: out?.lines ?? plain, lang: out?.lang ?? null, skipped: null };
}

/** The tokens of one line as React nodes (plain text where there is no class). */
export function Tokens({ line }: { line: Line }) {
  return <>{line.map((t, j) => (t[0] ? <span key={j} className={t[0]}>{t[1]}</span> : t[1]))}</>;
}

export interface CodeProps {
  text: string;
  /** Fence tag, or a file name (`name`) to take the language from; neither = auto-detect (small blocks). */
  lang?: string | null;
  name?: string;
  mime?: string | null;
  lineNumbers?: boolean;
  wrap?: boolean;
  /** Scroll this 1-based line into view and mark it. */
  focusLine?: number | null;
  className?: string;
  testId?: string;
}

/** Plain-text-safe, highlighted code with optional line numbers and soft wrap. */
export const Code = memo(function Code({ text, lang, name, mime, lineNumbers = false, wrap = false, focusLine, className, testId }: CodeProps) {
  const resolved = (lang !== undefined && lang !== null ? languageFromTag(lang) : null) ?? (name ? languageFromName(name, mime) : null);
  const { lines, lang: used, skipped } = useHighlighted(text, resolved);
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (focusLine) ref.current?.querySelector(`[data-ln="${focusLine}"]`)?.scrollIntoView({ block: "center" });
  }, [focusLine, text]);
  return (
    <>
      {skipped === "large" ? <div className="faint fs12 hl-note">Large file: syntax colouring is off.</div> : null}
      <pre ref={ref} className={`hl${lineNumbers ? " ln" : ""}${wrap ? " wrap" : ""}${className ? ` ${className}` : ""}`} data-testid={testId} data-lang={used ?? ""}>
        {lines.map((l, i) => (
          <span key={i} className={`hl-l${focusLine === i + 1 ? " focus" : ""}`} data-ln={i + 1}>
            {l.length ? <Tokens line={l} /> : "\n"}
            {l.length ? "\n" : null}
          </span>
        ))}
      </pre>
    </>
  );
});
