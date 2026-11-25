import type { Api, ExportSaved } from "../../bridge/api";
import type { ExportChunk, ExportEvent, ExportFile, ExportFormat, ExportReadRequest, HostId, SessionExportRequest, SessionId } from "../../bridge/wire";

/**
 * The conversation export as the dialog asks for it (O5 policy): tool outputs are cut at this many bytes
 * (the runtime's default cap, 64 KiB); the dialog states it as fixed text, so it is not a choice.
 */
export const TOOL_OUTPUT_CAP = 64 * 1024;

/** Bytes per `session.export.read` (the runtime's `MAX_EXPORT_READ`). */
export const READ_CHUNK = 1 << 20;

/** What the Format control offers: one file, or both. */
export type FormatChoice = "markdown" | "json" | "both";

export interface ExportOptions {
  format: FormatChoice;
  /** The events of subagents' conversations (default: no). */
  includeSubagents: boolean;
  /** The conversation's earlier history, before the session's own events (default: yes). */
  includeEarlier: boolean;
}

export const DEFAULT_EXPORT_OPTIONS: ExportOptions = { format: "both", includeSubagents: false, includeEarlier: true };

export function formatsOf(choice: FormatChoice): ExportFormat[] {
  return choice === "both" ? ["markdown", "json"] : [choice];
}

/** `session.export.start`'s parameters for `session` and the dialog's options. */
export function exportRequest(session: SessionId, o: ExportOptions): SessionExportRequest {
  return {
    session,
    formats: formatsOf(o.format),
    include_subagents: o.includeSubagents,
    include_earlier: o.includeEarlier,
    tool_output_cap: TOOL_OUTPUT_CAP,
  };
}

/**
 * The dialog's export, from the first request to the end. `starting`: the start is pending; `running`: the
 * export is on the host (`events` and `bytes` as its progress reports say; `cancelling` once Cancel was
 * asked); `done`: its files and the count of what was masked; `failed`: why it did not finish.
 */
export type ExportRun =
  | { s: "idle" }
  | { s: "starting" }
  | { s: "running"; id: string; events: number; bytes: number; cancelling: boolean }
  | { s: "done"; id: string; files: ExportFile[]; masked: number }
  | { s: "failed"; reason: string };

export const startedRun = (id: string): ExportRun => ({ s: "running", id, events: 0, bytes: 0, cancelling: false });

/** Apply one event of the export's topic. Only a running export changes: a finished one is what it is. */
export function applyEvent(run: ExportRun, e: ExportEvent): ExportRun {
  if (run.s !== "running") return run;
  switch (e.event) {
    case "progress":
      return { ...run, events: e.events_done, bytes: e.bytes };
    case "done":
      return { s: "done", id: run.id, files: e.files, masked: Object.values(e.redactions).reduce((a, n) => a + n, 0) };
    case "failed":
      return { s: "failed", reason: e.reason };
  }
}

/** The topic ended: a running export that never said how it ended has failed (an ended topic with an error says why). */
export function endedRun(run: ExportRun, end: { message: string } | null): ExportRun {
  if (run.s !== "running") return run;
  return { s: "failed", reason: end?.message ?? "the export ended without a result" };
}

/** Secrets masked, as the count the words say ("1 secret", "3 secrets"). */
export function maskedWords(n: number): string {
  return `${n} ${n === 1 ? "secret" : "secrets"} masked (best effort)`;
}

/**
 * The bytes a base64 string stands for, without decoding it (standard alphabet, padded). A string that is
 * not base64 gives a meaningless count, so the caller checks it against the offsets the runtime reports.
 */
export function decodedLength(b64: string): number {
  if (!b64) return 0;
  const pad = b64.endsWith("==") ? 2 : b64.endsWith("=") ? 1 : 0;
  return (b64.length / 4) * 3 - pad;
}

/** The three calls a save makes: read a chunk of the export, ask where to save, append a chunk to that file. */
export interface ExportIO {
  read(r: ExportReadRequest): Promise<ExportChunk>;
  chooseSave(name: string): Promise<ExportSaved>;
  writeChunk(path: string, data: string): Promise<unknown>;
}

/** The export's calls on a host, through the bridge. */
export function exportIO(api: Api, host: HostId): ExportIO {
  return {
    read: (r) => api.sessionExport.read(host, r),
    chooseSave: (name) => api.desktop.chooseSave(name),
    writeChunk: (path, data) => api.desktop.downloadChunk(path, data),
  };
}

/** A file the export wrote whose bytes did not all arrive: the saved file would be wrong, so the save stops. */
export class ExportSaveError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ExportSaveError";
  }
}

const baseName = (path: string): string => path.split(/[\\/]/).pop() || "conversation";

/**
 * Save one finished export file where the user chooses. Asks for the place first (the Save dialog, suggesting
 * the file's own name); cancelling there reads nothing and answers `null`. Otherwise the file is read from
 * the host in order, `READ_CHUNK` bytes at a time, and each chunk is appended to the chosen file as it arrives.
 * The reads must follow each other exactly: every reply starts where the last one ended, and the bytes end
 * at the total the runtime reports. Anything else is an error, and the partly written file is left as it is.
 */
export async function saveExportFile(io: ExportIO, exportId: string, file: Pick<ExportFile, "format" | "path">): Promise<string | null> {
  const picked = await io.chooseSave(baseName(file.path));
  if (!("path" in picked)) return null;
  let offset = 0;
  let total: number;
  for (;;) {
    const c = await io.read({ export_id: exportId, format: file.format, offset, len: READ_CHUNK });
    if (c.offset !== offset) throw new ExportSaveError(`the export answered from byte ${c.offset}, not from ${offset}`);
    const n = decodedLength(c.data);
    if (n === 0 && !c.eof) throw new ExportSaveError(`the export stopped at byte ${offset} of ${c.total}`);
    if (n > 0) await io.writeChunk(picked.path, c.data);
    offset += n;
    total = c.total;
    if (c.eof) break;
  }
  if (offset !== total) throw new ExportSaveError(`the saved file would be ${offset} bytes, the export is ${total}`);
  return picked.path;
}

/** Save each of the export's files in turn (one Save dialog each). Answers the paths saved; a cancelled dialog skips its file. */
export async function saveExportFiles(io: ExportIO, exportId: string, files: readonly ExportFile[]): Promise<string[]> {
  const saved: string[] = [];
  for (const f of files) {
    const p = await saveExportFile(io, exportId, f);
    if (p !== null) saved.push(p);
  }
  return saved;
}
