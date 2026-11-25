import { useEffect, useMemo, useState } from "react";
import type { Api } from "../../bridge/api";
import type { ContentPart, FileStat, HostId, SessionId } from "../../bridge/wire";
import { useApi } from "../../state";
import { useMediaScope } from "./Parts";
import { parsePathRefs } from "./pathRefs";
import { PathChip } from "./PathChip";

/** Input keys that name the file a tool writes or edits. */
const FILE_KEYS = ["file_path", "filePath", "notebook_path", "path"];
/** Tools whose input path is a file they create or change (claude, ACP kinds, codex). */
const WRITES = /^(write|edit|multiedit|notebookedit|create|apply_patch|str_replace_based_edit_tool)$/i;

/**
 * Files a tool call writes or edits: the paths of its diff parts (the driver reports each change it made), then, from its input,
 * the file of a write-like tool (codex `apply_patch` lists changes).
 */
export function toolFileRefs(name: string, input: unknown, parts: readonly ContentPart[] = []): string[] {
  const out: string[] = [];
  for (const p of parts) if (p.type === "diff") out.push(p.path);
  if (input && typeof input === "object") {
    const o = input as Record<string, unknown>;
    if (WRITES.test(name.trim())) {
      for (const k of FILE_KEYS) if (typeof o[k] === "string" && o[k]) out.push(o[k] as string);
    }
    if (Array.isArray(input)) {
      for (const c of input) if (c && typeof c === "object" && typeof (c as { path?: unknown }).path === "string") out.push((c as { path: string }).path);
    }
  }
  return [...new Set(out)];
}

/** Path-like tokens in prose (`src/app.ts`, `~/x`, `../y/z.rs:12`, `/abs/x.pdf`), at most 30. */
export function textPathCandidates(text: string, windows = false): string[] {
  return [...new Set(parsePathRefs(text, windows).map((r) => r.path))];
}

/** Per-session answers of `session.files.stat` (null: missing). */
const known = new Map<string, Map<string, FileStat | null>>();
const waiting = new Map<string, { api: Api; host: HostId; session: SessionId; paths: Set<string>; done: Array<() => void>; timer: ReturnType<typeof setTimeout> }>();
/** Paths are asked together after a short pause (a transcript renders many messages at once). */
export const STAT_DEBOUNCE_MS = 120;
export const STAT_BATCH = 50;

/** Ask the host about `paths` not yet known: debounced, batched, cached; resolves when answered. */
export function statAll(api: Api, host: HostId, session: SessionId, paths: string[]): Promise<void> {
  const k = `${host}/${session}`;
  const m = known.get(k) ?? new Map<string, FileStat | null>();
  known.set(k, m);
  const ask = paths.filter((p) => !m.has(p));
  if (!ask.length) return Promise.resolve();
  let w = waiting.get(k);
  if (!w) {
    w = { api, host, session, paths: new Set(), done: [], timer: setTimeout(() => flush(k), STAT_DEBOUNCE_MS) };
    waiting.set(k, w);
  }
  for (const p of ask) w.paths.add(p);
  return new Promise((res) => w!.done.push(res));
}

async function flush(k: string) {
  const w = waiting.get(k);
  if (!w) return;
  waiting.delete(k);
  const m = known.get(k) ?? new Map<string, FileStat | null>();
  known.set(k, m);
  const all = [...w.paths].filter((p) => !m.has(p));
  for (let i = 0; i < all.length; i += STAT_BATCH) {
    const batch = all.slice(i, i + STAT_BATCH);
    await w.api.media.stat(w.host, w.session, batch).then(
      (r) => {
        for (const p of batch) m.set(p, null);
        for (const f of r.files) m.set(f.path, f);
      },
      // Hosts without the method (older helpers) simply link nothing.
      () => batch.forEach((p) => m.set(p, null)),
    );
  }
  w.done.forEach((f) => f());
}

/** Number of stat requests waiting for their debounce (tests). */
export const pendingStats = () => waiting.size;

/** The subset of `paths` that exist on the session's host (files), as stats. */
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
  for (const w of waiting.values()) clearTimeout(w.timer);
  waiting.clear();
}

/** Chips for the paths that exist on the session's host; `lines` maps a path to the line a mention names. */
export function FileRefs({ paths, label = "Files", lines }: { paths: string[]; label?: string; lines?: Record<string, number> }) {
  const uniq = useMemo(() => [...new Set(paths)], [paths]);
  const files = useExistingFiles(uniq);
  if (!files.length) return null;
  return (
    <div className="parts frefs" aria-label={label} data-testid="file-refs">
      {files.map((f) => (
        <PathChip key={f.resolved} stat={f} line={lines?.[f.path]} />
      ))}
    </div>
  );
}
