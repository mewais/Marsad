import { describe, expect, it } from "vitest";
import type { ExportChunk, ExportEvent, ExportReadRequest } from "../../src/bridge/wire";
import {
  applyEvent,
  decodedLength,
  endedRun,
  ExportSaveError,
  exportRequest,
  formatsOf,
  maskedWords,
  READ_CHUNK,
  saveExportFile,
  saveExportFiles,
  startedRun,
  TOOL_OUTPUT_CAP,
  type ExportIO,
} from "../../src/features/session/conversationExport";

const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));
const utf8 = (s: string) => new TextEncoder().encode(s);
const fromB64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

/**
 * A fake export on the host that answers reads in pieces of `piece` bytes (fewer than asked, as a host may),
 * and a Save dialog that answers `path` (or cancels with `null`). Records every call.
 */
function fakeIO(content: Uint8Array, opts: { piece?: number; path?: string | null } = {}) {
  const reads: ExportReadRequest[] = [];
  const written: { path: string; data: string }[] = [];
  const asked: string[] = [];
  const piece = opts.piece ?? 3;
  const io: ExportIO = {
    async read(r) {
      reads.push(r);
      const offset = Math.min(r.offset, content.length);
      const end = Math.min(offset + Math.min(r.len, piece), content.length);
      return { offset, total: content.length, eof: end >= content.length, data: b64(content.subarray(offset, end)) } satisfies ExportChunk;
    },
    async chooseSave(name) {
      asked.push(name);
      return opts.path === null ? { saved: false } : { path: opts.path ?? `/home/me/Downloads/${name}` };
    },
    async writeChunk(path, data) {
      written.push({ path, data });
      return null;
    },
  };
  return { io, reads, written, asked };
}

const concat = (chunks: { data: string }[]) => {
  const parts = chunks.map((c) => fromB64(c.data));
  const out = new Uint8Array(parts.reduce((a, p) => a + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
};

describe("export request", () => {
  it("sends the session, the formats, the two switches and the fixed tool-output cap", () => {
    expect(exportRequest("s1", { format: "both", includeSubagents: false, includeEarlier: true })).toEqual({
      session: "s1",
      formats: ["markdown", "json"],
      include_subagents: false,
      include_earlier: true,
      tool_output_cap: 65536,
    });
    expect(exportRequest("s2", { format: "json", includeSubagents: true, includeEarlier: false }).formats).toEqual(["json"]);
    expect(TOOL_OUTPUT_CAP).toBe(64 * 1024);
  });

  it("offers one format or both, in the order the files are listed", () => {
    expect(formatsOf("markdown")).toEqual(["markdown"]);
    expect(formatsOf("json")).toEqual(["json"]);
    expect(formatsOf("both")).toEqual(["markdown", "json"]);
  });
});

describe("export progress", () => {
  it("shows the latest progress report while running, and keeps cancelling apart from it", () => {
    let run = startedRun("sx1");
    run = applyEvent(run, { event: "progress", events_done: 40, bytes: 5000 });
    run = applyEvent(run, { event: "progress", events_done: 120, bytes: 18000 });
    expect(run).toEqual({ s: "running", id: "sx1", events: 120, bytes: 18000, cancelling: false });
    expect({ ...run, cancelling: true }).toMatchObject({ events: 120 });
  });

  it("ends with the files and the masked total, summed over the kinds", () => {
    const done: ExportEvent = {
      event: "done",
      files: [{ format: "markdown", path: "/x/a.md", bytes: 10 }],
      redactions: { api_key: 2, password: 1 },
    };
    expect(applyEvent(startedRun("sx1"), done)).toEqual({ s: "done", id: "sx1", files: [{ format: "markdown", path: "/x/a.md", bytes: 10 }], masked: 3 });
    expect(maskedWords(3)).toBe("3 secrets masked (best effort)");
    expect(maskedWords(1)).toBe("1 secret masked (best effort)");
    expect(maskedWords(0)).toBe("0 secrets masked (best effort)");
  });

  it("ends as failed with the runtime's reason, and an ended topic without a result is a failure too", () => {
    expect(applyEvent(startedRun("sx1"), { event: "failed", reason: "disk full" })).toEqual({ s: "failed", reason: "disk full" });
    expect(endedRun(startedRun("sx1"), null)).toEqual({ s: "failed", reason: "the export ended without a result" });
    expect(endedRun(startedRun("sx1"), { message: "no such export: sx1" })).toEqual({ s: "failed", reason: "no such export: sx1" });
  });

  it("ignores events once the export is finished", () => {
    const done = applyEvent(startedRun("sx1"), { event: "failed", reason: "cancelled" });
    expect(applyEvent(done, { event: "progress", events_done: 9, bytes: 9 })).toBe(done);
    expect(endedRun(done, null)).toBe(done);
  });
});

describe("saving an export file", () => {
  it("reads the file in order and appends every chunk to the chosen file, so the bytes are exactly the export's", async () => {
    const content = utf8("# Conversation\n\nHello, wörld: tool output and a secret-free reply.\n");
    const f = fakeIO(content, { piece: 7 });
    const path = await saveExportFile(f.io, "sx9", { format: "markdown", path: "~/.local/share/marsad/sessions/s/exports/conversation-sx9.md" });
    expect(path).toBe("/home/me/Downloads/conversation-sx9.md");
    expect(f.asked).toEqual(["conversation-sx9.md"]);
    // Each read asks for a chunk at the end of the last one, in the chosen format.
    expect(f.reads.map((r) => r.offset)).toEqual(Array.from({ length: f.reads.length }, (_, i) => i * 7));
    expect(f.reads.every((r) => r.export_id === "sx9" && r.format === "markdown" && r.len === READ_CHUNK)).toBe(true);
    // The chunks handed to the save bridge, joined, are the file.
    expect(f.written.every((w) => w.path === path)).toBe(true);
    expect(new TextDecoder().decode(concat(f.written))).toBe(new TextDecoder().decode(content));
  });

  it("reads nothing when the Save dialog is cancelled", async () => {
    const f = fakeIO(utf8("abc"), { path: null });
    expect(await saveExportFile(f.io, "sx1", { format: "json", path: "/x/c.json" })).toBeNull();
    expect(f.reads).toEqual([]);
    expect(f.written).toEqual([]);
  });

  it("saves an empty file as an empty file", async () => {
    const f = fakeIO(new Uint8Array(0));
    expect(await saveExportFile(f.io, "sx1", { format: "json", path: "/x/e.json" })).toBe("/home/me/Downloads/e.json");
    expect(f.written).toEqual([]);
  });

  it("saves each file of the export with its own Save dialog, and skips the ones cancelled", async () => {
    const md = fakeIO(utf8("md"));
    const saved = await saveExportFiles(md.io, "sx2", [
      { format: "markdown", path: "/x/a.md", bytes: 2 },
      { format: "json", path: "/x/a.json", bytes: 2 },
    ]);
    expect(saved).toEqual(["/home/me/Downloads/a.md", "/home/me/Downloads/a.json"]);
    expect(md.asked).toEqual(["a.md", "a.json"]);
    const cancelled = fakeIO(utf8("md"), { path: null });
    expect(await saveExportFiles(cancelled.io, "sx2", [{ format: "markdown", path: "/x/a.md", bytes: 2 }])).toEqual([]);
  });

  it("stops with an error when a reply starts somewhere other than where the last one ended", async () => {
    const io: ExportIO = {
      read: async () => ({ offset: 5, total: 9, eof: false, data: b64(utf8("abc")) }),
      chooseSave: async () => ({ path: "/x/o.md" }),
      writeChunk: async () => null,
    };
    await expect(saveExportFile(io, "sx1", { format: "markdown", path: "/x/o.md" })).rejects.toBeInstanceOf(ExportSaveError);
  });

  it("stops with an error when the export gives no bytes before its end", async () => {
    const io: ExportIO = {
      read: async () => ({ offset: 0, total: 9, eof: false, data: "" }),
      chooseSave: async () => ({ path: "/x/o.md" }),
      writeChunk: async () => null,
    };
    await expect(saveExportFile(io, "sx1", { format: "markdown", path: "/x/o.md" })).rejects.toThrow("the export stopped at byte 0 of 9");
  });

  it("refuses to call a file saved when its bytes do not add up to the total", async () => {
    const io: ExportIO = {
      read: async () => ({ offset: 0, total: 9, eof: true, data: b64(utf8("abc")) }),
      chooseSave: async () => ({ path: "/x/o.md" }),
      writeChunk: async () => null,
    };
    await expect(saveExportFile(io, "sx1", { format: "markdown", path: "/x/o.md" })).rejects.toThrow("the saved file would be 3 bytes, the export is 9");
  });
});

describe("base64 length", () => {
  it("counts the decoded bytes from the text, padding included", () => {
    for (const s of ["", "a", "ab", "abc", "abcd", "hello world!!"]) {
      expect(decodedLength(btoa(s))).toBe(s.length);
    }
  });
});
