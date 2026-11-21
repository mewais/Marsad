import { useEffect, useMemo, useState } from "react";
import type { Api } from "../../bridge/api";
import type { FileStat, HostId, SessionId } from "../../bridge/wire";
import { useApi } from "../../state";
import { baseName } from "./fetch";
import { FileChip, useMediaScope } from "./Parts";

/** Input keys that name the file a tool writes or edits. */
const FILE_KEYS = ["file_path", "filePath", "notebook_path", "path"];
/** Tools whose input path is a file they create or change (claude, ACP kinds, codex). */
const WRITES = /^(write|edit|multiedit|notebookedit|create|apply_patch|str_replace_based_edit_tool)$/i;

/** Files a tool call writes or edits, from its input (codex `apply_patch` lists changes). */
export function toolFileRefs(name: string, input: unknown): string[] {
  if (!input || typeof input !== "object") return [];
  const o = input as Record<string, unknown>;
  const out: string[] = [];
  if (WRITES.test(name.trim())) {
    for (const k of FILE_KEYS) if (typeof o[k] === "string" && o[k]) out.push(o[k] as string);
  }
  if (Array.isArray(input)) {
    for (const c of input) if (c && typeof c === "object" && typeof (c as { path?: unknown }).path === "string") out.push((c as { path: string }).path);
  }
  return [...new Set(out)];
}

const PATH_RE = /(?:^|[\s`'"(\[])((?:~?\/|\.{1,2}\/)?(?:[\w@+-][\w.@+-]*\/)*[\w@+-][\w.@+-]*\.[A-Za-z0-9]{1,8})(?=$|[\s`'"),:;\]!?]|\.(?:\s|$))/g;

/** Path-like tokens in prose (`src/app.ts`, `./out/chart.svg`, `/abs/x.pdf`), at most 20. */
export function textPathCandidates(text: string): string[] {
  const out = new Set<string>();
  for (const m of text.matchAll(PATH_RE)) {
    const p = m[1];
    if (p.length > 300 || /^\d+(\.\d+)+$/.test(p) || p.includes("://")) continue;
    // A bare name needs a known-looking extension; anything with a slash is a candidate.
    if (!p.includes("/") && !/\.(svg|png|jpe?g|gif|webp|pdf|html?|md|json|csv|tsv|txt|py|ts|tsx|js|rs|go|sh|toml|ya?ml|mp3|mp4|wav)$/i.test(p)) continue;
    out.add(p);
    if (out.size >= 20) break;
  }
  return [...out];
}

/** Per-session answers of `session.files.stat` (null: missing or outside the session). */
const known = new Map<string, Map<string, FileStat | null>>();
const inflight = new Map<string, Promise<void>>();

async function statAll(api: Api, host: HostId, session: SessionId, paths: string[]) {
  const k = `${host}/${session}`;
  const m = known.get(k) ?? new Map<string, FileStat | null>();
  known.set(k, m);
  const ask = paths.filter((p) => !m.has(p));
  if (!ask.length) return;
  const key = `${k}:${ask.join("\0")}`;
  const running = inflight.get(key);
  if (running) return running;
  const p = api.media
    .stat(host, session, ask)
    .then((r) => {
      for (const p of ask) m.set(p, null);
      for (const f of r.files) m.set(f.path, f);
    })
    .catch(() => {
      // Hosts without the method (older helpers) simply link nothing.
      for (const p of ask) m.set(p, null);
    })
    .finally(() => inflight.delete(key));
  inflight.set(key, p);
  return p;
}

/** The subset of `paths` that exist inside the session's folder, as stats. */
export function useExistingFiles(paths: string[]): FileStat[] {
  const api = useApi();
  const scope = useMediaScope();
  const sig = paths.join("\0");
  const [stats, setStats] = useState<FileStat[]>([]);
  useEffect(() => {
    if (!scope || !paths.length) return;
    let live = true;
    void statAll(api, scope.host, scope.session, paths).then(() => {
      const m = known.get(`${scope.host}/${scope.session}`);
      if (live && m) setStats(paths.map((p) => m.get(p)).filter((x): x is FileStat => !!x && !x.is_dir));
    });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sig, scope?.host, scope?.session]);
  return stats;
}

/** Test hook. */
export function clearFileRefs() {
  known.clear();
}

/** Chips for the files among `paths` that exist in the session's folder. */
export function FileRefs({ paths, label = "Files" }: { paths: string[]; label?: string }) {
  const uniq = useMemo(() => [...new Set(paths)], [paths]);
  const files = useExistingFiles(uniq);
  if (!files.length) return null;
  return (
    <div className="parts frefs" aria-label={label} data-testid="file-refs">
      {files.map((f) => (
        <FileChip key={f.resolved} target={{ path: f.resolved }} name={baseName(f.path)} mime={f.mime} size={f.size} testId="file-ref" />
      ))}
    </div>
  );
}
