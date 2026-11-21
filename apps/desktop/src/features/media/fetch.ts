import type { Api } from "../../bridge/api";
import type { HostId, MediaSource, SessionId } from "../../bridge/wire";

/** What to read on the session's host: a blob of its store, or a path (relative to its cwd). */
export type Target = { blob: string } | { path: string; outside?: boolean; grant?: string | null };

export interface Loaded {
  bytes: Uint8Array;
  mime: string;
  /** Size of the whole file (may exceed `bytes.length` when `truncated`). */
  size: number;
  /** Canonical path on the host. */
  path: string;
  truncated: boolean;
}

/** Largest chunk asked per `session.file.read` (the host caps it at 4 MiB). */
const CHUNK = 2 * 1024 * 1024;

export function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function bytesToB64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

/** The read target of a media source, when its bytes live on the session's host. */
export function targetOf(src: MediaSource): Target | null {
  switch (src.source) {
    case "blob":
      return { blob: src.id };
    case "path":
      return { path: src.path };
    default:
      return null;
  }
}

export const targetKey = (host: HostId, session: SessionId, t: Target) => `${host}/${session}/${"blob" in t ? `b:${t.blob}` : `p:${t.path}`}`;

/** Read up to `max` bytes of a target in ranges (the host enforces its roots and limits). */
export async function readTarget(api: Api, host: HostId, session: SessionId, t: Target, max: number, onProgress?: (done: number, total: number) => void): Promise<Loaded> {
  const parts: Uint8Array[] = [];
  let offset = 0;
  let size = 0;
  let mime = "application/octet-stream";
  let path = "";
  for (;;) {
    const c = await api.media.read(host, {
      id: session,
      blob: "blob" in t ? t.blob : null,
      path: "path" in t ? t.path : null,
      offset,
      length: Math.min(CHUNK, Math.max(1, max - offset)),
      outside: "path" in t ? !!t.outside : false,
      grant: "path" in t ? t.grant ?? null : null,
    });
    const b = b64ToBytes(c.data);
    parts.push(b);
    offset += b.length;
    size = c.size;
    mime = c.mime;
    path = c.path;
    onProgress?.(offset, size);
    if (c.eof || b.length === 0 || offset >= max) break;
  }
  const bytes = new Uint8Array(offset);
  let at = 0;
  for (const p of parts) {
    bytes.set(p, at);
    at += p.length;
  }
  return { bytes, mime, size, path, truncated: offset < size };
}

/** A small cache of loaded media (thumbnails are fetched once per target). */
const cache = new Map<string, Promise<Loaded>>();
const CACHE_MAX = 60;

export function loadCached(api: Api, host: HostId, session: SessionId, t: Target, max: number): Promise<Loaded> {
  const k = `${targetKey(host, session, t)}#${max}`;
  const hit = cache.get(k);
  if (hit) {
    cache.delete(k);
    cache.set(k, hit);
    return hit;
  }
  const p = readTarget(api, host, session, t, max);
  cache.set(k, p);
  p.catch(() => cache.delete(k));
  while (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value as string);
  return p;
}

/** Test hook. */
export function clearMediaCache() {
  cache.clear();
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function baseName(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

/** A MIME type from a file name, for files the browser does not type (picked paths). */
export function mimeFromName(name: string): string {
  const ext = name.toLowerCase().split(".").pop() ?? "";
  const m: Record<string, string> = {
    png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml",
    pdf: "application/pdf", html: "text/html", htm: "text/html", md: "text/markdown", markdown: "text/markdown",
    json: "application/json", csv: "text/csv", tsv: "text/tab-separated-values", txt: "text/plain", log: "text/plain",
    mp3: "audio/mpeg", wav: "audio/wav", ogg: "audio/ogg", mp4: "video/mp4", webm: "video/webm", xml: "application/xml",
  };
  if (m[ext]) return m[ext];
  if (CODE_EXT.has(ext)) return "text/plain";
  return "application/octet-stream";
}

export const CODE_EXT = new Set(["ts", "tsx", "js", "jsx", "mjs", "cjs", "rs", "py", "go", "java", "c", "h", "cpp", "hpp", "cs", "rb", "php", "sh", "bash", "zsh", "toml", "yaml", "yml", "ini", "css", "scss", "sql", "kt", "swift", "lua", "txt", "log", "env", "gitignore", "dockerfile", "makefile"]);

export type Kind = "image" | "svg" | "pdf" | "html" | "markdown" | "json" | "csv" | "text" | "audio" | "video" | "other";

/** How a file is previewed, from its (host-sniffed) MIME type and name. */
export function kindOf(mime: string | null | undefined, name: string): Kind {
  const m = (mime ?? "").toLowerCase();
  const ext = name.toLowerCase().split(".").pop() ?? "";
  if (m === "image/svg+xml") return "svg";
  if (m.startsWith("image/")) return ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(m) ? "image" : "other";
  if (m === "application/pdf") return "pdf";
  if (m === "text/html") return "html";
  if (m === "text/markdown" || ext === "md" || ext === "markdown") return "markdown";
  if (m === "application/json" || ext === "json") return "json";
  if (m === "text/csv" || m === "text/tab-separated-values" || ext === "csv" || ext === "tsv") return "csv";
  if (m.startsWith("audio/")) return "audio";
  if (m.startsWith("video/")) return "video";
  if (m.startsWith("text/") || m === "application/xml" || m === "application/javascript" || CODE_EXT.has(ext)) return "text";
  return "other";
}
