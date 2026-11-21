/**
 * The one highlighter of the app: highlight.js core with a small set of languages. This module
 * is loaded lazily (`import("./engine")`), so none of it is in the main chunk. It turns code
 * into per-line token lists — never HTML — so rendering is plain React elements.
 */
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
import graphql from "highlight.js/lib/languages/graphql";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import less from "highlight.js/lib/languages/less";
import lua from "highlight.js/lib/languages/lua";
import makefile from "highlight.js/lib/languages/makefile";
import markdown from "highlight.js/lib/languages/markdown";
import php from "highlight.js/lib/languages/php";
import powershell from "highlight.js/lib/languages/powershell";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import scss from "highlight.js/lib/languages/scss";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import { parseHighlighted, type Line } from "./tokens";

const LANGS = { bash, c, cpp, csharp, css, diff, dockerfile, go, graphql, ini, java, javascript, json, kotlin, less, lua, makefile, markdown, php, powershell, python, ruby, rust, scss, sql, swift, typescript, xml, yaml };
for (const [name, def] of Object.entries(LANGS)) hljs.registerLanguage(name, def);

/** Languages tried when a block has no tag: the common ones only (cheap, fewer false positives). */
const AUTO = ["bash", "css", "diff", "go", "java", "javascript", "json", "python", "rust", "sql", "typescript", "xml", "yaml", "c", "cpp"];

/** Minimum relevance for an auto-detected language to be used (below it the block stays plain). */
const AUTO_MIN = 4;

export interface Highlighted {
  lang: string | null;
  lines: Line[];
}

/** Highlight `code` as `lang`, or auto-detect when null. Unknown languages come back plain. */
export function highlight(code: string, lang: string | null): Highlighted {
  try {
    if (lang && lang !== "plaintext" && hljs.getLanguage(lang)) {
      const r = hljs.highlight(code, { language: lang, ignoreIllegals: true });
      return { lang, lines: parseHighlighted(r.value) };
    }
    if (!lang) {
      const r = hljs.highlightAuto(code, AUTO);
      if (r.language && r.relevance >= AUTO_MIN) return { lang: r.language, lines: parseHighlighted(r.value) };
    }
  } catch {
    /* fall through to plain */
  }
  return { lang: null, lines: parseHighlighted(escapeText(code)) };
}

function escapeText(s: string) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
