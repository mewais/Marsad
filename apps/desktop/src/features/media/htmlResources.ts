/**
 * Relative resources of a previewed HTML file. The file is untrusted and runs in a sandboxed
 * frame with a no-network CSP, so it cannot fetch anything itself. Before it is shown, every
 * relative reference (images, `srcset`, media, stylesheets, `url(...)` and `@import` in CSS,
 * fonts, and — only when the user turned scripts on — scripts) is resolved against the file's
 * own directory, read through the session's restricted `session.file.read` (the host enforces
 * the roots, symlinks and size caps) and inlined as a `data:` URL / inline text. Absolute
 * `http(s)` URLs and every other scheme are removed and counted.
 */
import { bytesToB64 } from "./fetch";

export interface Resource {
  bytes: Uint8Array;
  mime: string;
}
/** Read one file by absolute path on the host; null when missing, denied, too large or not a file. */
export type Reader = (absPath: string, maxBytes: number) => Promise<Resource | null>;

export interface HtmlResult {
  html: string;
  /** Remote or non-file references that were removed. */
  blocked: number;
  /** Relative references that could not be loaded (missing, outside the allowed folders, too large). */
  missing: number;
  loaded: number;
}

export const MAX_RESOURCES = 200;
export const MAX_TOTAL_BYTES = 24 << 20;
const CAP = { image: 8 << 20, media: 12 << 20, font: 4 << 20, css: 1 << 20, script: 1 << 20 } as const;
type Cap = keyof typeof CAP;

const MIME: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml", avif: "image/avif", ico: "image/x-icon", bmp: "image/bmp",
  css: "text/css", js: "text/javascript", mjs: "text/javascript", woff: "font/woff", woff2: "font/woff2", ttf: "font/ttf", otf: "font/otf",
  mp4: "video/mp4", webm: "video/webm", ogg: "audio/ogg", mp3: "audio/mpeg", wav: "audio/wav",
};

const extOf = (p: string) => (p.split(/[?#]/)[0].split(".").pop() ?? "").toLowerCase();

// ── paths ───────────────────────────────────────────────────────────────────

/** The directory of a host path (either separator style). */
export function dirnameOf(p: string): string {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i <= 0 ? (i === 0 ? p.slice(0, 1) : "") : p.slice(0, i);
}

export type Ref = { kind: "keep" } | { kind: "remote" } | { kind: "path"; path: string } | { kind: "bad" };

const SCHEME = /^[a-zA-Z][a-zA-Z0-9+.-]*:/;

/**
 * Classify a reference found in the HTML: `keep` (data:, blob:, fragments, empty), `remote`
 * (any scheme, `//host`), `bad` (root-relative, NUL, undecodable) or `path` — an absolute host
 * path made of `baseDir` and the reference (`.`/`..` resolved lexically, never above the root;
 * the host decides whether the result is readable).
 */
export function classifyRef(baseDir: string, ref: string): Ref {
  const r = ref.trim();
  if (!r || r.startsWith("#")) return { kind: "keep" };
  if (/^(data|blob):/i.test(r)) return { kind: "keep" };
  if (r.startsWith("//") || r.startsWith("\\\\") || SCHEME.test(r)) return { kind: "remote" };
  if (r.startsWith("/") || r.startsWith("\\")) return { kind: "bad" };
  let rel = r.split(/[?#]/)[0];
  try {
    rel = decodeURIComponent(rel);
  } catch {
    return { kind: "bad" };
  }
  if (!rel || rel.includes("\0")) return { kind: "bad" };
  const win = /^[a-zA-Z]:[\\/]/.test(baseDir) || baseDir.startsWith("\\\\");
  const sep = win ? "\\" : "/";
  const drive = win ? /^([a-zA-Z]:|\\\\[^\\]+\\[^\\]+)/.exec(baseDir)?.[0] ?? "" : "";
  const segs = baseDir.slice(drive.length).split(/[\\/]/).filter(Boolean);
  for (const s of rel.split(/[\\/]/)) {
    if (s === "" || s === ".") continue;
    if (s === "..") segs.pop();
    else segs.push(s);
  }
  return { kind: "path", path: win ? `${drive}${sep}${segs.join(sep)}` : `/${segs.join("/")}` };
}

// ── loading with budgets ────────────────────────────────────────────────────

class Loader {
  count = 0;
  total = 0;
  blocked = 0;
  missing = 0;
  loaded = 0;
  private memo: Map<string, Promise<Resource | null>>;
  constructor(private read: Reader, memo?: Map<string, Promise<Resource | null>>) {
    this.memo = memo ?? new Map();
  }

  /** Fetch a resolved path once; null (and counted) when it cannot be had. */
  async get(path: string, cap: Cap): Promise<Resource | null> {
    let p = this.memo.get(path);
    if (!p) {
      if (this.count >= MAX_RESOURCES || this.total >= MAX_TOTAL_BYTES) {
        this.missing++;
        return null;
      }
      this.count++;
      p = this.read(path, CAP[cap]).catch(() => null);
      this.memo.set(path, p);
    }
    const r = await p;
    if (!r) {
      this.missing++;
      return null;
    }
    this.total += r.bytes.length;
    this.loaded++;
    return r;
  }

  async dataUrl(baseDir: string, ref: string, cap: Cap): Promise<string | null> {
    const c = classifyRef(baseDir, ref);
    if (c.kind === "keep") return ref;
    if (c.kind === "remote") {
      this.blocked++;
      return null;
    }
    if (c.kind === "bad") {
      this.missing++;
      return null;
    }
    const r = await this.get(c.path, cap);
    if (!r) return null;
    const mime = MIME[extOf(c.path)] ?? (r.mime || "application/octet-stream");
    return `data:${mime};base64,${bytesToB64(r.bytes)}`;
  }

  async text(baseDir: string, ref: string, cap: Cap): Promise<{ text: string; dir: string } | null> {
    const c = classifyRef(baseDir, ref);
    if (c.kind === "remote") {
      this.blocked++;
      return null;
    }
    if (c.kind !== "path") {
      if (c.kind === "bad") this.missing++;
      return null;
    }
    const r = await this.get(c.path, cap);
    return r ? { text: new TextDecoder().decode(r.bytes), dir: dirnameOf(c.path) } : null;
  }
}

// ── CSS ─────────────────────────────────────────────────────────────────────

const CSS_URL = /url\(\s*(?:"([^"]*)"|'([^']*)'|([^)\s'"]*))\s*\)/gi;
const CSS_IMPORT = /@import\s+(?:url\(\s*(?:"([^"]*)"|'([^']*)'|([^)\s'"]*))\s*\)|"([^"]*)"|'([^']*)')\s*([^;]*);/gi;

async function replaceAsync(s: string, re: RegExp, f: (m: RegExpExecArray) => Promise<string>): Promise<string> {
  const parts: Promise<string>[] = [];
  const gaps: string[] = [];
  let last = 0;
  re.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s))) {
    gaps.push(s.slice(last, m.index));
    parts.push(f(m));
    last = m.index + m[0].length;
  }
  gaps.push(s.slice(last));
  const out = await Promise.all(parts);
  return gaps.reduce((a, g, i) => a + g + (out[i] ?? ""), "");
}

async function processCss(css: string, dir: string, l: Loader, depth: number): Promise<string> {
  // `@import` first: the imported sheet is inlined (and processed against its own directory).
  const withImports = await replaceAsync(css, CSS_IMPORT, async (m) => {
    const ref = m[1] ?? m[2] ?? m[3] ?? m[4] ?? m[5] ?? "";
    if (depth >= 3) return "";
    const t = await l.text(dir, ref, "css");
    if (!t) return "";
    const inner = await processCss(t.text, t.dir, l, depth + 1);
    const media = (m[6] ?? "").trim();
    return media ? `@media ${media}{${inner}}` : inner;
  });
  return replaceAsync(withImports, CSS_URL, async (m) => {
    const ref = m[1] ?? m[2] ?? m[3] ?? "";
    const font = /\.(woff2?|ttf|otf)(?:[?#]|$)/i.test(ref);
    const d = await l.dataUrl(dir, ref, font ? "font" : "image");
    return d ? `url("${d}")` : "url()";
  });
}

// ── HTML ────────────────────────────────────────────────────────────────────

async function srcset(value: string, dir: string, l: Loader): Promise<string> {
  // Candidates are `url descriptor` separated by commas (a data: URL's own commas stay in it).
  const cands = value.split(/,\s+|,(?=[^\s,]+\s+[\d.]+[xw]\b)/).map((c) => c.trim()).filter(Boolean);
  const out: string[] = [];
  for (const c of cands) {
    const [url, ...desc] = c.split(/\s+/);
    const d = await l.dataUrl(dir, url, "image");
    if (d) out.push([d, ...desc].join(" "));
  }
  return out.join(", ");
}

/** Run `f` over items with at most `n` in flight. */
async function pool<T>(items: T[], n: number, f: (t: T) => Promise<void>): Promise<void> {
  let i = 0;
  await Promise.all(Array.from({ length: Math.min(n, items.length) }, async () => {
    while (i < items.length) await f(items[i++]);
  }));
}

/**
 * Resolve `html`'s relative resources and return the document to put in the sandboxed frame
 * (the caller adds the CSP). `memo` lets a re-run (the scripts toggle) reuse what was read.
 */
export async function resolveHtmlResources(
  html: string,
  o: { baseDir: string; scripts: boolean; read: Reader; csp: string; memo?: Map<string, Promise<Resource | null>> },
): Promise<HtmlResult> {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const l = new Loader(o.read, o.memo);
  const dir = o.baseDir;
  const jobs: Array<() => Promise<void>> = [];
  const q = (sel: string) => Array.from(doc.querySelectorAll(sel));

  // Never navigable or embeddable: the frame has no network and no way out.
  for (const el of q("iframe,frame,frameset,object,embed,applet,base,meta[http-equiv]")) {
    const ref = el.getAttribute("src") ?? el.getAttribute("data") ?? el.getAttribute("href") ?? "";
    if (/^(https?:)?\/\//i.test(ref.trim())) l.blocked++;
    el.remove();
  }
  for (const el of q("a[href],area[href]")) if (!(el.getAttribute("href") ?? "").startsWith("#")) el.removeAttribute("href");
  for (const el of q("form")) el.removeAttribute("action");

  const attr = (el: Element, name: string, cap: Cap) =>
    jobs.push(async () => {
      const v = el.getAttribute(name);
      if (v == null) return;
      const d = await l.dataUrl(dir, v, cap);
      if (d) el.setAttribute(name, d);
      else el.removeAttribute(name);
    });
  for (const el of q("img,input[type=image]")) attr(el, "src", "image");
  for (const el of q("video")) {
    attr(el, "src", "media");
    attr(el, "poster", "image");
  }
  for (const el of q("audio,source")) attr(el, "src", el.closest("picture") ? "image" : "media");
  for (const el of q("svg image,svg feImage")) {
    for (const n of ["href", "xlink:href"]) if (el.hasAttribute(n)) attr(el, n, "image");
  }
  for (const el of q("img[srcset],source[srcset]")) {
    jobs.push(async () => {
      const v = await srcset(el.getAttribute("srcset") ?? "", dir, l);
      if (v) el.setAttribute("srcset", v);
      else el.removeAttribute("srcset");
    });
  }
  for (const el of q("track")) el.remove();
  for (const el of q("link")) {
    const rel = (el.getAttribute("rel") ?? "").toLowerCase();
    const href = el.getAttribute("href") ?? "";
    if (!/\bstylesheet\b/.test(rel)) {
      if (/^(https?:)?\/\//i.test(href.trim())) l.blocked++;
      el.remove();
      continue;
    }
    jobs.push(async () => {
      const t = await l.text(dir, href, "css");
      if (!t) return el.remove();
      const style = doc.createElement("style");
      style.textContent = await processCss(t.text, t.dir, l, 0);
      const media = el.getAttribute("media");
      if (media) style.setAttribute("media", media);
      el.replaceWith(style);
    });
  }
  for (const el of q("style")) {
    jobs.push(async () => {
      el.textContent = await processCss(el.textContent ?? "", dir, l, 0);
    });
  }
  for (const el of q("[style]")) {
    jobs.push(async () => {
      el.setAttribute("style", await processCss(el.getAttribute("style") ?? "", dir, l, 0));
    });
  }
  for (const el of q("script[src]")) {
    const src = el.getAttribute("src") ?? "";
    if (!o.scripts) {
      // Scripts are off: nothing to load; remote ones still count as blocked.
      if (/^(https?:)?\/\//i.test(src.trim())) l.blocked++;
      el.remove();
      continue;
    }
    jobs.push(async () => {
      const t = await l.text(dir, src, "script");
      if (!t) return el.remove();
      el.removeAttribute("src");
      // Keep the script text from ending its own element.
      el.textContent = t.text.replace(/<\/(script)/gi, "<\\/$1");
    });
  }

  await pool(jobs, 4, (j) => j());

  const meta = doc.createElement("meta");
  meta.setAttribute("http-equiv", "Content-Security-Policy");
  meta.setAttribute("content", o.csp);
  const ref = doc.createElement("meta");
  ref.setAttribute("name", "referrer");
  ref.setAttribute("content", "no-referrer");
  doc.head.prepend(meta, ref);
  return { html: `<!doctype html>${doc.documentElement.outerHTML}`, blocked: l.blocked, missing: l.missing, loaded: l.loaded };
}
