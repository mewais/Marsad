/**
 * Which language a piece of code is in: from a fence tag, a file name or a MIME type. Pure and
 * tiny (the highlighter itself is a lazy chunk). The ids are the highlighter's language names.
 */
const ALIAS: Record<string, string> = {
  js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript", javascript: "javascript", node: "javascript",
  ts: "typescript", tsx: "typescript", typescript: "typescript", mts: "typescript", cts: "typescript",
  json: "json", jsonc: "json", json5: "json", jsonl: "json", ndjson: "json",
  py: "python", python: "python", python3: "python", py3: "python",
  rs: "rust", rust: "rust",
  go: "go", golang: "go",
  sh: "bash", bash: "bash", zsh: "bash", shell: "bash", console: "bash", ksh: "bash", fish: "bash",
  ps1: "powershell", powershell: "powershell", pwsh: "powershell",
  yml: "yaml", yaml: "yaml",
  toml: "ini", ini: "ini", cfg: "ini", conf: "ini", properties: "ini", env: "ini",
  css: "css", scss: "scss", less: "less",
  html: "xml", htm: "xml", xml: "xml", svg: "xml", xhtml: "xml", vue: "xml", plist: "xml",
  md: "markdown", markdown: "markdown", mdx: "markdown",
  sql: "sql", psql: "sql", mysql: "sql", sqlite: "sql",
  java: "java", kt: "kotlin", kts: "kotlin", kotlin: "kotlin", swift: "swift", scala: "scala",
  c: "c", h: "c", cc: "cpp", cpp: "cpp", cxx: "cpp", hpp: "cpp", hh: "cpp", "c++": "cpp",
  cs: "csharp", csharp: "csharp",
  rb: "ruby", ruby: "ruby", php: "php", lua: "lua", pl: "perl", perl: "perl", r: "r", dart: "dart",
  diff: "diff", patch: "diff", udiff: "diff",
  dockerfile: "dockerfile", docker: "dockerfile", makefile: "makefile", make: "makefile", mk: "makefile",
  graphql: "graphql", gql: "graphql", proto: "protobuf", protobuf: "protobuf", nginx: "nginx",
  ex: "elixir", exs: "elixir", elixir: "elixir", hs: "haskell", haskell: "haskell", zig: "plaintext",
};

/** Exact file names (lowercase) that name a language without an extension. */
const BY_NAME: Record<string, string> = {
  dockerfile: "dockerfile", makefile: "makefile", gnumakefile: "makefile", ".env": "ini", ".gitignore": "plaintext",
  ".bashrc": "bash", ".zshrc": "bash", ".profile": "bash", "cargo.lock": "ini", "cmakelists.txt": "plaintext",
};

const BY_MIME: Record<string, string> = {
  "application/json": "json", "text/javascript": "javascript", "application/javascript": "javascript", "application/xml": "xml",
  "text/xml": "xml", "text/html": "xml", "text/css": "css", "text/markdown": "markdown", "text/x-python": "python",
  "application/x-sh": "bash", "text/x-shellscript": "bash", "application/yaml": "yaml", "text/yaml": "yaml", "application/toml": "ini",
};

/** The highlighter language for a fence tag (`ts`, `language-rust`, `{.py}`), or null when unknown. */
export function languageFromTag(tag: string | null | undefined): string | null {
  if (!tag) return null;
  const t = tag.trim().toLowerCase().replace(/^language-/, "").replace(/^\{?\.?/, "").split(/[\s,{}:]/)[0];
  if (!t || t === "text" || t === "txt" || t === "plain" || t === "plaintext" || t === "none") return t ? "plaintext" : null;
  return ALIAS[t] ?? null;
}

/** The language of a file by name, then MIME type; null when neither says (auto-detect or plain). */
export function languageFromName(name: string, mime?: string | null): string | null {
  const base = (name.split(/[\\/]/).pop() ?? name).toLowerCase();
  if (BY_NAME[base]) return BY_NAME[base];
  const dot = base.lastIndexOf(".");
  const ext = dot > 0 ? base.slice(dot + 1) : dot === 0 ? base.slice(1) : "";
  if (ext && ALIAS[ext]) return ALIAS[ext];
  const m = (mime ?? "").toLowerCase().split(";")[0];
  return BY_MIME[m] ?? null;
}

/** Highlighting is skipped above these sizes so huge files stay responsive. */
export const HL_MAX_BYTES = 256 * 1024;
export const HL_MAX_LINES = 6000;
/** Auto-detection (no tag, no extension) only for small blocks: it tries every language. */
export const AUTO_MAX_BYTES = 16 * 1024;
