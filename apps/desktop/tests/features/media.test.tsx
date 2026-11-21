import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { Attachment, PromptInput, SessionEvent } from "../../src/bridge/wire";
import { applyMedia, MEDIA_SPEC } from "../../src/bridge/fixture/media";
import { fixtureId } from "../../src/bridge/fixture/sessions";
import { promptInput } from "../../src/bridge/fixture/catalog";
import { clearMediaCache, kindOf } from "../../src/features/media/fetch";
import { clearFileRefs, textPathCandidates, toolFileRefs } from "../../src/features/media/refs";
import { htmlPreviewDoc, previewCsp } from "../../src/features/media/sanitize";
import { parseCsv } from "../../src/features/media/PreviewHost";
import { batchProblem, handling, upload, type PendingFile } from "../../src/features/session/attachments";
import { buildItems } from "../../src/features/session/transcript";
import { createApi } from "../../src/bridge/api";
import { quietBackend, renderApp } from "../helpers";

beforeAll(() => {
  applyMedia();
  // happy-dom never reports intersections: thumbnails load at once without the observer.
  (globalThis as { IntersectionObserver?: unknown }).IntersectionObserver = undefined;
  let n = 0;
  if (!URL.createObjectURL) URL.createObjectURL = () => `blob:fixture/${n++}`;
  if (!URL.revokeObjectURL) URL.revokeObjectURL = () => undefined;
});
beforeEach(() => {
  clearMediaCache();
  clearFileRefs();
});

const MEDIA = fixtureId(MEDIA_SPEC.n);

async function openMedia() {
  const backend = quietBackend();
  const r = await renderApp(`/session/x1/${MEDIA}`, backend);
  await screen.findByRole("heading", { name: MEDIA_SPEC.name });
  return r;
}

const file = (name: string, mime: string, size: number): PendingFile => ({
  key: name, name, mime, size,
  read: async (o, l) => new Uint8Array(Math.max(0, Math.min(l, size - o))).fill(7),
});

describe("attachments: what each driver takes (from its descriptor)", () => {
  const claude = promptInput("path_reference");
  const acp = promptInput("embedded");
  const imagesOnly: PromptInput = { ...claude, files: "none" };
  const nothing: PromptInput = { ...claude, image_mimes: [], files: "none", max_attachments: 0 };

  it("images within limits go as images; other files by path or embedded; refusals explain", () => {
    expect(handling(file("a.png", "image/png", 10), claude)).toMatchObject({ ok: true, how: "image" });
    expect(handling(file("a.png", "image/png", 6 << 20), claude)).toMatchObject({ ok: false, note: expect.stringMatching(/5 MB/) });
    expect(handling(file("r.pdf", "application/pdf", 10), claude)).toMatchObject({ ok: true, how: "path", note: expect.stringMatching(/names its path/) });
    expect(handling(file("n.txt", "text/plain", 10), acp)).toMatchObject({ ok: true, how: "embedded" });
    expect(handling(file("r.pdf", "application/pdf", 10), acp)).toMatchObject({ ok: true, how: "path", note: expect.stringMatching(/link/) });
    expect(handling(file("r.pdf", "application/pdf", 10), imagesOnly)).toMatchObject({ ok: false, note: expect.stringMatching(/images only/) });
    expect(handling(file("a.bmp", "image/bmp", 10), imagesOnly).ok).toBe(false);
    expect(handling(file("a.png", "image/png", 1), nothing)).toMatchObject({ ok: false, note: "This agent takes no attachments." });
  });

  it("a batch is refused for count and for the embedded total", () => {
    const many = Array.from({ length: 11 }, (_, i) => file(`${i}.png`, "image/png", 1));
    expect(batchProblem(many, claude)).toMatch(/At most 10/);
    expect(batchProblem([file("a.png", "image/png", 4 << 20), file("b.png", "image/png", 4 << 20)], claude)).toMatch(/5.5 MB/);
    expect(batchProblem([file("a.png", "image/png", 4 << 20), file("b.pdf", "application/pdf", 20 << 20)], claude)).toBeNull();
  });

  it("uploads in ordered chunks and returns the host's attachment", async () => {
    const backend = quietBackend();
    const api = createApi(backend);
    const a = await upload(api, "x1", MEDIA_ID_OF(backend), file("big.bin", "application/octet-stream", 2_000_000));
    expect(a).toMatchObject({ name: "big.bin", size: 2_000_000 });
    const puts = backend.requests.filter((r) => r.method === "session.attachment.put");
    expect(puts.length).toBe(3);
    expect(puts.map((p) => (p.params as { offset: number }).offset)).toEqual([0, 786432, 1572864]);
    expect((puts[2].params as { last: boolean }).last).toBe(true);
  });
});

function MEDIA_ID_OF(b: ReturnType<typeof quietBackend>) {
  return b.sessionIds("x1")[0];
}

describe("transcript model", () => {
  it("keeps attachments, assistant media and tool parts", () => {
    const att: Attachment = { name: "a.png", mime: "image/png", size: 3, path: "/up/a.png" };
    const ev = (cursor: number, rest: object) => ({ cursor, t: cursor, ...rest }) as SessionEvent;
    const items = buildItems([
      ev(1, { kind: "input", command: { command: "prompt", text: "see", attachments: [att] }, status: { status: "sent" } }),
      ev(2, { kind: "agent", event: { type: "tool_call_started", name: "Read", call_id: "c", input: {}, is_subagent: false } }),
      ev(3, { kind: "agent", event: { type: "tool_call_finished", name: "Read", call_id: "c", ok: true, result: [{ type: "image", source: { data: "[1 bytes omitted]" } }], parts: [{ type: "image", mime: "image/png", source: { source: "blob", id: "b".repeat(64), size: 3 } }] } }),
      ev(4, { kind: "agent", event: { type: "assistant_content", parts: [{ type: "file", source: { source: "path", path: "x.pdf" }, name: "x.pdf" }] } }),
    ]);
    expect(items[0]).toMatchObject({ kind: "user", attachments: [att] });
    expect(items[1]).toMatchObject({ kind: "tool", done: true, parts: [{ type: "image" }] });
    expect(items[2]).toMatchObject({ kind: "media", role: "assistant" });
  });

  it("finds written files and plain paths in prose", () => {
    expect(toolFileRefs("Write", { file_path: "out/a.svg" })).toEqual(["out/a.svg"]);
    expect(toolFileRefs("Read", { file_path: "out/a.svg" })).toEqual([]);
    expect(toolFileRefs("edit", { filePath: "/w/x.ts" })).toEqual(["/w/x.ts"]);
    expect(textPathCandidates("See `out/chart.svg` and report.pdf, not v1.2.3 or https://x.io/a.png.")).toEqual(["out/chart.svg", "report.pdf"]);
  });

  it("previews by host-sniffed type", () => {
    expect(kindOf("image/svg+xml", "x")).toBe("svg");
    expect(kindOf("application/pdf", "x.bin")).toBe("pdf");
    expect(kindOf("text/plain", "main.rs")).toBe("text");
    expect(kindOf("application/octet-stream", "notes.bin")).toBe("other");
    expect(parseCsv('a,b\n"x, y",2\n', ",")).toEqual([["a", "b"], ["x, y", "2"]]);
  });
});

describe("sanitisation", () => {
  it("HTML previews carry a no-network CSP; scripts only when asked", () => {
    expect(previewCsp(false)).toContain("default-src 'none'");
    expect(previewCsp(false)).toContain("script-src 'none'");
    expect(previewCsp(true)).toContain("script-src 'unsafe-inline'");
    expect(previewCsp(true)).toContain("connect-src 'none'");
    const doc = htmlPreviewDoc("<script>x()</script>", false);
    expect(doc.indexOf("Content-Security-Policy")).toBeLessThan(doc.indexOf("<script>"));
  });
});

describe("session view with media", () => {
  it("renders inline images, attachments and file chips; the lightbox and previews open", async () => {
    const user = userEvent.setup();
    await openMedia();
    const imgs = await screen.findAllByTestId("media-image");
    expect(imgs).toHaveLength(2);
    await waitFor(() => expect(imgs[1].querySelector("img")).not.toBeNull());
    expect(imgs[1].querySelector("img")).toHaveAttribute("alt", "image");
    expect(screen.getByTestId("sent-attachments")).toHaveTextContent("");
    // Files the agent wrote (tool chips), mentioned (prose) and showed (resource link).
    await waitFor(() => expect(screen.getAllByTestId("file-ref").map((c) => c.textContent)).toEqual(expect.arrayContaining([expect.stringContaining("chart.svg"), expect.stringContaining("report.pdf"), expect.stringContaining("README.md")])));
    expect(screen.getByTestId("file-chip")).toHaveTextContent("preview.html");

    // Lightbox.
    await user.click(imgs[1]);
    const pv = await screen.findByTestId("preview");
    await waitFor(() => expect(pv).toHaveAttribute("data-kind", "image"));
    expect(await screen.findByTestId("preview-image")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByTestId("preview")).toBeNull());

    // HTML: sandboxed, scripts off, then on.
    await user.click(screen.getByTestId("file-chip").querySelector("button")!);
    const frame = await screen.findByTestId("preview-html");
    expect(frame).toHaveAttribute("sandbox", "");
    expect(frame.getAttribute("srcdoc")).toContain("script-src 'none'");
    await user.click(screen.getByLabelText("Run scripts"));
    expect(screen.getByTestId("preview-html")).toHaveAttribute("sandbox", "allow-scripts");
    expect(screen.getByTestId("preview-html").getAttribute("sandbox")).not.toContain("same-origin");
    await user.keyboard("{Escape}");

    const chip = (name: string) => screen.getAllByTestId("file-ref").find((c) => c.textContent?.includes(name))!.querySelector("button")!;
    // Markdown: no raw HTML, no remote images.
    await user.click(chip("README.md"));
    const md = await screen.findByTestId("preview-markdown");
    expect(md).toHaveTextContent("The chart is in");
    expect(md.querySelector("img, script")).toBeNull();
    await user.keyboard("{Escape}");
    // SVG through <img>, never inline.
    await user.click(chip("chart.svg"));
    await waitFor(() => expect(screen.getByTestId("preview")).toHaveAttribute("data-kind", "svg"));
    expect(await screen.findByTestId("preview-image")).toBeInTheDocument();
    expect(screen.getByTestId("preview").querySelector("svg script")).toBeNull();
    await user.keyboard("{Escape}");
    // The local host offers the system app and the folder.
    await user.click(chip("report.pdf"));
    await waitFor(() => expect(screen.getByTestId("preview")).toHaveAttribute("data-kind", "pdf"));
    expect(screen.getByRole("button", { name: /Open with system app/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Reveal in folder/ })).toBeInTheDocument();
  });

  it("previews csv as a table, json pretty, and unknown types as metadata", async () => {
    const user = userEvent.setup();
    const { backend } = await openMedia();
    const api = createApi(backend);
    // Paths outside the session's folder are refused by the host.
    await expect(api.media.read("x1", { id: MEDIA, blob: null, path: "../../etc/passwd", offset: 0, length: null, outside: false })).rejects.toThrow(/outside/);
    const { openPreview } = await import("../../src/features/media/preview");
    for (const [path, testId] of [["data.csv", "preview-table"], ["config.json", "preview-text"], ["notes.bin", "preview-other"]] as const) {
      openPreview({ host: "x1", session: MEDIA, target: { path }, name: path, localFiles: false });
      expect(await screen.findByTestId(testId)).toBeInTheDocument();
      if (testId === "preview-table") expect(within(screen.getByTestId(testId)).getByText("Q3, est")).toBeInTheDocument();
      if (testId === "preview-text") expect(screen.getByTestId(testId)).toHaveTextContent('"type": "bar"');
      // A remote host offers download instead of the system app.
      expect(screen.getByRole("button", { name: "Download" })).toBeInTheDocument();
      await user.keyboard("{Escape}");
    }
  });

  it("composer: paste and drop add chips, remove works, unsupported files explain, send uploads", async () => {
    const user = userEvent.setup();
    const { backend } = await openMedia();
    const box = screen.getByLabelText("Message");
    const png = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "shot.png", { type: "image/png" });
    fireEvent.paste(box, { clipboardData: { files: [png], types: ["Files"] } });
    expect(await screen.findAllByTestId("pending-attachment")).toHaveLength(1);
    const pdf = new File([new Uint8Array(10)], "spec.pdf", { type: "application/pdf" });
    fireEvent.drop(box.closest(".composer")!, { dataTransfer: { files: [pdf], types: ["Files"] } });
    const chips = await screen.findAllByTestId("pending-attachment");
    expect(chips).toHaveLength(2);
    expect(chips[1].getAttribute("title")).toMatch(/names its path/);
    await user.click(screen.getByRole("button", { name: "Remove spec.pdf" }));
    expect(screen.getAllByTestId("pending-attachment")).toHaveLength(1);
    await user.type(box, "what is this");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.queryAllByTestId("pending-attachment")).toHaveLength(0));
    const cmd = backend.requests.find((r) => r.method === "session.command" && (r.params as { command: { command: string } }).command.command === "prompt");
    expect((cmd!.params as { command: { attachments: Attachment[] } }).command.attachments).toMatchObject([{ name: "shot.png", mime: "image/png", size: 4 }]);
  });
});
