import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { createApi } from "../../src/bridge/api";
import type { Api } from "../../src/bridge/api";
import type { FileStat, LaunchReply, PromptInput } from "../../src/bridge/wire";
import { promptInput } from "../../src/bridge/fixture/catalog";
import { revalidate } from "../../src/features/session/attachments";
import { applyMedia, MEDIA_SPEC } from "../../src/bridge/fixture/media";
import { fixtureId } from "../../src/bridge/fixture/sessions";
import { Code, skipReason } from "../../src/features/highlight/Code";
import { languageFromName, languageFromTag, HL_MAX_BYTES } from "../../src/features/highlight/lang";
import { parseHighlighted } from "../../src/features/highlight/tokens";
import { classifyRef, dirnameOf, resolveHtmlResources, type Reader } from "../../src/features/media/htmlResources";
import { parsePathRefs, parsePathToken } from "../../src/features/media/pathRefs";
import { openPreview } from "../../src/features/media/preview";
import { clearFileRefs, pendingStats, statAll, STAT_BATCH, STAT_DEBOUNCE_MS } from "../../src/features/media/refs";
import { launchWithAttachments } from "../../src/features/newsession/launch";
import { peekDraft } from "../../src/features/session/draft";
import { Markdown } from "../../src/features/session/Markdown";
import { clearMediaCache } from "../../src/features/media/fetch";
import { quietBackend, renderApp } from "../helpers";

beforeAll(() => {
  applyMedia();
  (globalThis as { IntersectionObserver?: unknown }).IntersectionObserver = undefined;
  let n = 0;
  if (!URL.createObjectURL) URL.createObjectURL = () => `blob:fixture/${n++}`;
  if (!URL.revokeObjectURL) URL.revokeObjectURL = () => undefined;
});
beforeEach(() => {
  clearMediaCache();
  clearFileRefs();
});
afterEach(() => vi.useRealTimers());

const MEDIA = fixtureId(MEDIA_SPEC.n);
const te = new TextEncoder();

describe("highlighting: languages and limits", () => {
  it("detects the language from a fence tag, a file name and a MIME type", () => {
    expect(languageFromTag("ts")).toBe("typescript");
    expect(languageFromTag("language-rust")).toBe("rust");
    expect(languageFromTag("{.py}")).toBe("python");
    expect(languageFromTag("text")).toBe("plaintext");
    expect(languageFromTag("klingon")).toBeNull();
    expect(languageFromTag("")).toBeNull();
    expect(languageFromName("src/App.TSX")).toBe("typescript");
    expect(languageFromName("Dockerfile")).toBe("dockerfile");
    expect(languageFromName("C:\\x\\run.ps1")).toBe("powershell");
    expect(languageFromName("noext", "application/json")).toBe("json");
    expect(languageFromName("noext")).toBeNull();
  });
  it("skips colouring for huge files and for tag-less large blocks", () => {
    expect(skipReason("a\nb", "rust")).toBeNull();
    expect(skipReason("x".repeat(HL_MAX_BYTES + 1), "rust")).toBe("large");
    expect(skipReason("a\n".repeat(7000), "rust")).toBe("large");
    expect(skipReason("x".repeat(20_000), null)).toBe("auto-large");
    expect(skipReason("x", "plaintext")).toBe("none");
  });
  it("never interprets the highlighter's output as markup", () => {
    const lines = parseHighlighted('<span class="hljs-string">&quot;&lt;img src=x onerror=alert(1)&gt;&quot;</span> a &amp; b\n<span class="hljs-comment">/* a\nb */</span>');
    expect(lines[0][0]).toEqual(["hljs-string", '"<img src=x onerror=alert(1)>"']);
    expect(lines[0][1]).toEqual(["", " a & b"]);
    // A multi-line span is split per line and keeps its class.
    expect(lines[1][0]).toEqual(["hljs-comment", "/* a"]);
    expect(lines[2][0]).toEqual(["hljs-comment", "b */"]);
    // A class that is not an hljs word is dropped.
    expect(parseHighlighted('<span class="x onclick=1">t</span>')[0][0][0]).toBe("");
  });
  it("highlights a block lazily by language, plain text stays text, and a huge file stays plain", async () => {
    const { container, rerender } = render(<Code text={'const a = "x"; // hi'} lang="ts" />);
    expect(container.querySelector("pre")!.textContent).toContain("const a");
    await waitFor(() => expect(container.querySelector(".hljs-keyword")).toBeTruthy());
    expect(container.querySelector(".hljs-string")!.textContent).toBe('"x"');
    expect(container.querySelector(".hljs-comment")).toBeTruthy();
    rerender(<Code text={"x".repeat(HL_MAX_BYTES + 5)} lang="ts" />);
    await new Promise((r) => setTimeout(r, 30));
    expect(container.querySelector(".hljs-keyword")).toBeNull();
    expect(screen.getByText(/syntax colouring is off/)).toBeInTheDocument();
  });
  it("auto-detects a tag-less block and leaves prose plain", async () => {
    const { container } = render(<><Code text={'def f(x):\n    return [i for i in range(x)]\n\nprint(f(3))\n'} /><Code testId="prose" text="just some words here" /></>);
    await waitFor(() => expect(container.querySelector('pre[data-lang="python"]')).toBeTruthy());
    expect(screen.getByTestId("prose").getAttribute("data-lang")).toBe("");
  });
});

describe("markdown", () => {
  const md = ["# Title", "", "```ts", "const n: number = 1", "```", "", "- a", "  - b", "", "  ```bash", "  echo hi", "  ```", "", "- [x] done", "", "| h1 | h2 |", "|---|---|", "| 1 | 2 |", "", "<script>alert(1)</script>", "", "![x](https://example.com/p.png) [l](https://example.com)"].join("\n");
  it("renders headings, tables, nested lists, task lists and code in lists; highlights fences; drops raw HTML", async () => {
    const { container } = render(<Markdown text={md} />);
    expect(container.querySelector("h1")!.textContent).toBe("Title");
    expect(container.querySelector("table")).toBeTruthy();
    expect(container.querySelectorAll("ul ul").length).toBe(1);
    expect(container.querySelector('input[type="checkbox"]')).toBeTruthy();
    expect(container.querySelectorAll('[data-testid="md-code"]').length).toBe(2);
    expect(container.querySelector("script")).toBeNull();
    await waitFor(() => expect(container.querySelector(".hljs-keyword")).toBeTruthy());
    expect(container.querySelector('a[href="https://example.com"]')!.getAttribute("rel")).toContain("noopener");
  });
  it("file mode: links are inert and images never load", () => {
    const { container } = render(<Markdown text={md} variant="file" />);
    expect(container.querySelector("a")).toBeNull();
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("[image: x]");
  });
});

describe("html: relative resources", () => {
  const dir = "/home/you/src/reports/site";
  const files: Record<string, { bytes: Uint8Array; mime: string }> = {
    [`${dir}/img/a.png`]: { bytes: new Uint8Array([1, 2, 3]), mime: "image/png" },
    [`${dir}/x.css`]: { bytes: te.encode('@import "more.css";\n@import url(print.css) print;\nbody{background:url(img/a.png)}@font-face{src:url(f.woff2)}'), mime: "text/css" },
    [`${dir}/more.css`]: { bytes: te.encode("h1{color:red}"), mime: "text/css" },
    [`${dir}/print.css`]: { bytes: te.encode("p{color:#000}"), mime: "text/css" },
    [`${dir}/f.woff2`]: { bytes: new Uint8Array([9]), mime: "font/woff2" },
    [`${dir}/app.js`]: { bytes: te.encode("var s='</script>';"), mime: "text/javascript" },
    "/home/you/src/reports/up.png": { bytes: new Uint8Array([4]), mime: "image/png" },
  };
  const calls: string[] = [];
  const read: Reader = async (p) => {
    calls.push(p);
    return files[p] ?? null;
  };
  beforeEach(() => (calls.length = 0));
  const run = (html: string, scripts = false) => resolveHtmlResources(html, { baseDir: dir, scripts, read, csp: "default-src 'none'" });

  it("classifies references: keep, remote, bad, and paths resolved lexically", () => {
    expect(classifyRef(dir, "img/a.png")).toEqual({ kind: "path", path: `${dir}/img/a.png` });
    expect(classifyRef(dir, "./img/../a%20b.png?v=1#x")).toEqual({ kind: "path", path: `${dir}/a b.png` });
    expect(classifyRef(dir, "../up.png")).toEqual({ kind: "path", path: "/home/you/src/reports/up.png" });
    expect(classifyRef(dir, "../../../../../../../../etc/passwd")).toEqual({ kind: "path", path: "/etc/passwd" });
    for (const r of ["https://x.io/a.png", "http://x", "//cdn.x/a.js", "file:///etc/passwd", "ftp://x", "C:/x.png", "javascript:alert(1)"]) expect(classifyRef(dir, r).kind, r).toBe("remote");
    expect(classifyRef(dir, "/etc/passwd").kind).toBe("bad");
    expect(classifyRef(dir, "%00x").kind).toBe("bad");
    expect(classifyRef(dir, "%zz").kind).toBe("bad");
    expect(classifyRef(dir, "data:image/png;base64,AAAA").kind).toBe("keep");
    expect(classifyRef(dir, "#top").kind).toBe("keep");
    expect(classifyRef("C:\\Users\\me\\site", "..\\img\\a.png")).toEqual({ kind: "path", path: "C:\\Users\\me\\img\\a.png" });
    expect(classifyRef("\\\\wsl.localhost\\U\\home\\site", "a.png")).toEqual({ kind: "path", path: "\\\\wsl.localhost\\U\\home\\site\\a.png" });
    expect(dirnameOf("/a/b/c.html")).toBe("/a/b");
    expect(dirnameOf("C:\\a\\b.html")).toBe("C:\\a");
  });

  it("inlines images, srcset, css (imports, url, fonts), counts blocked remote content, and never fetches remote or traversal targets", async () => {
    const r = await run(
      `<html><head><link rel="stylesheet" href="x.css"><link rel="stylesheet" href="https://cdn.x/y.css"><style>.a{background:url('img/a.png')}</style></head>
       <body><img src="img/a.png"><img srcset="img/a.png 1x, ../up.png 2x"><img src="https://x.io/t.png"><img src="//x.io/u.png"><img src="../../../../../etc/passwd">
       <img src="missing.png"><div style="background:url(img/a.png)"></div><iframe src="https://x.io"></iframe><a href="https://x.io">l</a><meta http-equiv="refresh" content="0;url=https://x.io"><script src="app.js"></script></body></html>`,
    );
    expect(r.html).toContain("data:image/png;base64,AQID");
    expect(r.html).not.toContain("https://x.io");
    expect(r.html).not.toContain("//x.io");
    expect(r.html).not.toContain("<iframe");
    expect(r.html).not.toContain('http-equiv="refresh"');
    expect(r.html).toContain("Content-Security-Policy");
    expect(r.html).toContain("h1{color:red}");
    expect(r.html).toContain("@media print{p{color:#000}}");
    expect(r.html).toContain("data:font/woff2;base64,CQ==");
    // Scripts are off: nothing of app.js is read or kept.
    expect(r.html).not.toContain("var s=");
    expect(calls).not.toContain(`${dir}/app.js`);
    // The remote ones are blocked and counted: cdn css, two images, the iframe.
    expect(r.blocked).toBe(4);
    expect(r.missing).toBe(2);
    // The traversal attempt reaches the host reader only as a plain path the host will refuse.
    expect(calls).toContain("/etc/passwd");
    expect(calls.every((c) => !c.includes("http") && !c.includes(".."))).toBe(true);
    // Links are inert.
    expect(r.html).not.toMatch(/<a [^>]*href=/);
  });

  it("loads relative scripts only when scripts are on", async () => {
    const r = await run('<script src="app.js"></script><script src="https://x.io/e.js"></script>', true);
    expect(r.html).toContain("var s='<\\/script>';");
    expect(r.blocked).toBe(1);
  });

  it("stops at the resource budget", async () => {
    const many = Array.from({ length: 260 }, (_, i) => `<img src="img/n${i}.png">`).join("");
    const r = await run(many);
    expect(calls.length).toBe(200);
    expect(r.missing).toBe(260);
  });
});

describe("path chips: detection", () => {
  it("finds absolute, home, relative, outside-cwd, line-suffixed and Windows paths", () => {
    const p = (t: string, w = false) => parsePathToken(t, w);
    expect(p("/etc/hosts")).toEqual({ path: "/etc/hosts" });
    expect(p("~/.ssh/config")).toEqual({ path: "~/.ssh/config" });
    expect(p("~")).toEqual({ path: "~" });
    expect(p("../other/file.rs")).toEqual({ path: "../other/file.rs" });
    expect(p("./src/a.ts")).toEqual({ path: "./src/a.ts" });
    expect(p("src/app.ts:12")).toEqual({ path: "src/app.ts", line: 12, col: undefined });
    expect(p("src/app.ts:12:5")).toEqual({ path: "src/app.ts", line: 12, col: 5 });
    expect(p("src/app.ts#L40")).toEqual({ path: "src/app.ts", line: 40, col: undefined });
    expect(p("src/app.ts.")).toEqual({ path: "src/app.ts" });
    expect(p("README.md,")).toEqual({ path: "README.md" });
    expect(p("docs/")).toEqual({ path: "docs/" });
    expect(p("C:\\Users\\me\\a.txt", true)).toEqual({ path: "C:\\Users\\me\\a.txt" });
    expect(p("C:\\Users\\me\\a.txt:3", true)).toMatchObject({ path: "C:\\Users\\me\\a.txt", line: 3 });
    expect(p("C:/x/y.txt", true)).toEqual({ path: "C:/x/y.txt" });
    expect(p("\\\\srv\\share\\a.txt", true)).toBeTruthy();
    // Not paths.
    for (const t of ["C:\\Users\\me\\a.txt", "https://x.io/a.png", "and/or", "1/2", "v1.2.3", "word", "/", "//x", "a@b.com", "x:12", "mailto:a@b.c"]) expect(p(t), t).toBeNull();
  });
  it("collects distinct refs from prose, with lines, at most the bound", () => {
    const refs = parsePathRefs("See `src/app.ts:2` and (../notes/todo.md), then ~/.ssh/config; also src/app.ts:2 again, /abs/dir/ and https://x.io/a.png.");
    expect(refs.map((r) => r.path)).toEqual(["src/app.ts", "../notes/todo.md", "~/.ssh/config", "/abs/dir/"]);
    expect(refs[0].line).toBe(2);
    const long = Array.from({ length: 80 }, (_, i) => `d${i}/f${i}.txt`).join(" ");
    expect(parsePathRefs(long).length).toBe(30);
  });
  // (no secret-location handling: nothing outside the session's folder is ever opened)
});

const stat = (p: Partial<FileStat>): FileStat => ({ path: "x", resolved: "/home/me/x", size: 1, mime: "text/plain", is_dir: false, modified_ms: null, ...p });
const mockApi = (over: Partial<Record<string, unknown>> = {}) =>
  ({
    media: {
      stat: vi.fn(async (_h: string, _s: string, paths: string[]) => ({ files: paths.map((p) => stat({ path: p, resolved: `/r/${p}` })) })),
      ...over,
    },
  }) as unknown as Api;

describe("path chips: stat batching", () => {
  it("asks the host once for many paths, debounced, in bounded batches, cached", async () => {
    vi.useFakeTimers();
    const api = mockApi();
    const a = statAll(api, "x1", "s1", ["a.ts", "b.ts"]);
    const b = statAll(api, "x1", "s1", ["b.ts", "c.ts"]);
    expect(pendingStats()).toBe(1);
    expect((api.media.stat as ReturnType<typeof vi.fn>).mock.calls.length).toBe(0);
    await vi.advanceTimersByTimeAsync(STAT_DEBOUNCE_MS + 1);
    await Promise.all([a, b]);
    const calls = (api.media.stat as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.length).toBe(1);
    expect(calls[0][2]).toEqual(["a.ts", "b.ts", "c.ts"]);
    expect(calls[0].length).toBe(3);
    await statAll(api, "x1", "s1", ["a.ts"]);
    expect(calls.length).toBe(1);
    const many = Array.from({ length: STAT_BATCH * 2 + 3 }, (_, i) => `p${i}.ts`);
    const c = statAll(api, "x1", "s1", many);
    await vi.advanceTimersByTimeAsync(STAT_DEBOUNCE_MS + 1);
    await c;
    expect(calls.length).toBe(4);
    expect(calls.slice(1).every((x) => x[2].length <= STAT_BATCH)).toBe(true);
  });
});

describe("first prompt with attachments: launch flow", () => {
  const reply: LaunchReply = { host: "x1", id: "s9", unsupported: [] } as unknown as LaunchReply;
  const file = (name: string) => ({ key: name, name, size: 3, mime: "image/png", read: async () => new Uint8Array([1, 2, 3]) });
  const api = (fail?: "create" | "upload" | "send") => {
    const log: string[] = [];
    const a = {
      launch: vi.fn(async (r: { first_prompt: string | null }) => {
        log.push(`launch:${r.first_prompt}`);
        if (fail === "create") throw new Error("no host");
        return reply;
      }),
      media: {
        put: vi.fn(async (_h: string, r: { name: string; size: number; offset: number; last: boolean }) => {
          log.push(`put:${r.name}`);
          if (fail === "upload") throw new Error("link dropped");
          return { received: r.size, attachment: r.last ? { name: r.name, mime: "image/png", size: r.size, path: `/a/${r.name}` } : null };
        }),
      },
      command: vi.fn(async (_h: string, _i: string, c: { text: string; attachments: unknown[] }) => {
        log.push(`prompt:${c.text}:${c.attachments.length}`);
        if (fail === "send") throw new Error("busy");
        return {};
      }),
    };
    return { a: a as unknown as Api, log };
  };
  const req = { first_prompt: "hello" } as never;

  it("creates the session, uploads in order, then sends the prompt with the attachments; reports progress", async () => {
    const { a, log } = api();
    const steps: string[] = [];
    const out = await launchWithAttachments(a, req, "hello", [file("a.png"), file("b.png")], (s) => steps.push(s.step === "uploading" ? `up${s.index}/${s.total}` : s.step));
    expect(out).toMatchObject({ ok: true });
    expect(log).toEqual(["launch:null", "put:a.png", "put:b.png", "prompt:hello:2"]);
    expect(steps[0]).toBe("creating");
    expect(steps).toContain("up2/2");
    expect(steps[steps.length - 1]).toBe("sending");
  });
  it("a failed create changes nothing; a failed upload or send reports the created session", async () => {
    let r = api("create");
    expect(await launchWithAttachments(r.a, req, "hello", [file("a.png")], () => undefined)).toMatchObject({ ok: false, stage: "create" });
    expect(r.log).toEqual(["launch:null"]);
    r = api("upload");
    expect(await launchWithAttachments(r.a, req, "hello", [file("a.png"), file("b.png")], () => undefined)).toMatchObject({ ok: false, stage: "upload", reply });
    expect(r.log).toEqual(["launch:null", "put:a.png"]);
    r = api("send");
    expect(await launchWithAttachments(r.a, req, "hello", [file("a.png")], () => undefined)).toMatchObject({ ok: false, stage: "send", reply });
  });
});

describe("first prompt with attachments: the dialog", () => {
  const png = () => new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "shot.png", { type: "image/png" });
  async function openDialog() {
    const user = userEvent.setup();
    const r = await renderApp();
    await user.keyboard("n");
    const dlg = await screen.findByRole("dialog", { name: "New session" });
    await waitFor(() => expect(within(dlg).getByRole("radio", { name: /Claude Code/ })).toHaveAttribute("aria-checked", "true"));
    await waitFor(() => expect(within(dlg).getByLabelText("Project folder")).toHaveValue("~/src/ledgerline"));
    await waitFor(() => expect(within(dlg).getByRole("button", { name: "Attach files" })).toBeEnabled());
    return { user, dlg, ...r };
  }

  it("paste adds a chip; start creates the session, uploads, sends the first prompt with the file", async () => {
    const { user, dlg, backend, history } = await openDialog();
    const box = within(dlg).getByLabelText(/First message/);
    await user.type(box, "look at this");
    fireEvent.paste(box, { clipboardData: { files: [png()], types: ["Files"] } });
    expect(await within(dlg).findAllByTestId("pending-attachment")).toHaveLength(1);
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    await waitFor(() => expect(history.store.get()).toMatch(/^\/session\/x1\//));
    const methods = backend.requests.map((r) => r.method).filter((m) => ["sessions.launch", "session.attachment.put", "session.command"].includes(m));
    expect(methods.slice(0, 3)).toEqual(["sessions.launch", "session.attachment.put", "session.command"]);
    const launch = backend.requests.find((r) => r.method === "sessions.launch")!;
    expect((launch.params as { first_prompt: unknown }).first_prompt).toBeNull();
    const cmd = backend.requests.find((r) => r.method === "session.command" && (r.params as { command: { command: string } }).command.command === "prompt")!;
    expect((cmd.params as { command: { text: string; attachments: unknown[] } }).command).toMatchObject({ text: "look at this", attachments: [{ name: "shot.png" }] });
  });

  it("when the upload fails the session opens with the message and files still in its composer", async () => {
    const { user, dlg, backend, history } = await openDialog();
    const orig = backend.request.bind(backend);
    backend.request = (async (m: string, p: unknown, o: unknown) => {
      if (m === "session.attachment.put") throw new Error("link dropped");
      return (orig as (m: string, p: unknown, o: unknown) => Promise<unknown>)(m, p, o);
    }) as never;
    const box = within(dlg).getByLabelText(/First message/);
    await user.type(box, "keep me");
    fireEvent.paste(box, { clipboardData: { files: [png()], types: ["Files"] } });
    await within(dlg).findAllByTestId("pending-attachment");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    await waitFor(() => expect(history.store.get()).toMatch(/^\/session\/x1\//));
    const comp = await screen.findByLabelText("Message");
    await waitFor(() => expect(comp).toHaveValue("keep me"));
    expect(await screen.findAllByTestId("pending-attachment")).toHaveLength(1);
    expect(peekDraft(history.store.get().replace("/session/", ""))).toBeUndefined();
  });

  it("re-validates against another agent's limits: what it cannot take is dropped and explained", () => {
    const f = (name: string, mime: string, size = 3) => ({ name, mime, size });
    const claude = promptInput("path_reference");
    const imagesOnly: PromptInput = { ...claude, files: "none" };
    const none: PromptInput = { ...claude, files: "none", image_mimes: [], max_attachments: 0 };
    const set = [f("a.png", "image/png"), f("spec.pdf", "application/pdf"), f("b.png", "image/png")];
    expect(revalidate(set, claude)).toMatchObject({ keep: set, dropped: [] });
    const r = revalidate(set, imagesOnly);
    expect(r.keep.map((x) => x.name)).toEqual(["a.png", "b.png"]);
    expect(r.dropped).toEqual([expect.stringMatching(/^spec\.pdf: .*images only/)]);
    expect(revalidate(set, none).keep).toEqual([]);
    expect(revalidate(set, { ...claude, max_attachments: 1 }).dropped).toEqual([expect.stringMatching(/spec\.pdf: this agent takes at most 1/), expect.stringMatching(/b\.png: this agent takes at most 1/)]);
  });
});

describe("files in a session: WSL hosts count as local; relative resources; highlighted previews", () => {
  async function openMedia() {
    const r = await renderApp(`/session/x1/${MEDIA}`, quietBackend());
    await screen.findByRole("heading", { name: MEDIA_SPEC.name });
    return r;
  }
  const pv = (path: string, extra: Record<string, unknown> = {}) => openPreview({ host: "x1", session: MEDIA, target: { path }, name: path.split("/").pop()!, localFiles: false, ...extra });

  it("a host with local files gets Open / Reveal and no download; the shell is told the distro", async () => {
    const { backend } = await openMedia();
    act(() => pv("README.md", { localFiles: true, distro: "Ubuntu-24.04" }));
    await screen.findByTestId("preview-markdown");
    expect(screen.queryByRole("button", { name: /Download/ })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /Open with system app/ }));
    await waitFor(() => expect(backend.requests.some((r) => r.method === "desktop.open")).toBe(true));
    expect(backend.requests.find((r) => r.method === "desktop.open")!.params).toMatchObject({ distro: "Ubuntu-24.04", target: expect.stringContaining("README.md") });
    await userEvent.click(screen.getByRole("button", { name: /Reveal in folder/ }));
    expect(backend.requests.find((r) => r.method === "desktop.reveal")!.params).toMatchObject({ distro: "Ubuntu-24.04" });
  });

  it("html: relative image, css and font are inlined into the sandboxed frame; remote content is counted", async () => {
    await openMedia();
    act(() => pv("site/index.html"));
    await waitFor(() => expect(screen.getByTestId("html-blocked")).toHaveTextContent("Remote content blocked: 2"));
    const frame = screen.getByTestId("preview-html") as HTMLIFrameElement;
    expect(frame.getAttribute("sandbox")).toBe("");
    const doc = frame.getAttribute("srcdoc")!;
    expect(doc).toContain("data:image/png;base64,");
    expect(doc).toContain("font-family:sans-serif");
    expect(doc).not.toContain("example.com");
    expect(doc).toContain("Content-Security-Policy");
    expect(screen.getByTestId("html-missing")).toHaveTextContent("1 local resource");
  });

  it("code files are coloured with line numbers and a wrap toggle; markdown has Rendered / Source", async () => {
    const user = userEvent.setup();
    await openMedia();
    act(() => pv("src/app.ts"));
    const code = await screen.findByTestId("preview-text");
    await waitFor(() => expect(code.querySelector(".hljs-keyword")).toBeTruthy());
    expect(code.getAttribute("data-lang")).toBe("typescript");
    expect(code.querySelectorAll("[data-ln]").length).toBeGreaterThan(5);
    expect(code.className).not.toContain("wrap");
    await user.click(screen.getByLabelText("Wrap lines"));
    expect(screen.getByTestId("preview-text").className).toContain("wrap");
    await user.keyboard("{Escape}");
    act(() => pv("docs/guide.md"));
    const md = await screen.findByTestId("preview-markdown");
    expect(md.querySelector("h1")!.textContent).toBe("Guide");
    expect(md.querySelector("script")).toBeNull();
    expect(md.querySelector("img")).toBeNull();
    await waitFor(() => expect(md.querySelector(".hljs-built_in, .hljs-title, .hljs-literal, .hljs-keyword, .hljs-string, .hljs-attr")).toBeTruthy());
    await user.click(screen.getByRole("button", { name: "Source" }));
    expect((await screen.findByTestId("preview-text")).getAttribute("data-lang")).toBe("markdown");
    await user.keyboard("{Escape}");
    act(() => pv("config.json"));
    await waitFor(() => expect(screen.getByTestId("preview-text").getAttribute("data-lang")).toBe("json"));
  });

  it("a path:line chip opens the preview scrolled to that line", async () => {
    await openMedia();
    const scroll = vi.fn();
    (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView = scroll;
    act(() => pv("src/app.ts", { line: 4 }));
    const code = await screen.findByTestId("preview-text");
    await waitFor(() => expect(code.querySelector('[data-ln="4"]')!.className).toContain("focus"));
    expect(scroll).toHaveBeenCalled();
  });

  it("the transcript shows chips for paths inside the folder (with :line) and keeps outside paths as plain text; reads outside are refused", async () => {
    const user = userEvent.setup();
    const { backend } = await openMedia();
    const chips = await screen.findAllByTestId("file-ref", {}, { timeout: 4000 });
    const names = chips.map((c) => c.textContent ?? "");
    expect(names.some((n) => n.startsWith("app.ts:2"))).toBe(true);
    expect(names.some((n) => n.startsWith("index.html"))).toBe(true);
    // Outside the session's folder: never a chip, only text.
    for (const outside of ["todo.md", "config", "notes"]) expect(names.some((n) => n.startsWith(outside)), outside).toBe(false);
    expect(document.body.textContent).toContain("../notes/todo.md");
    expect(document.body.textContent).toContain("~/.ssh/config");
    // The host refuses to read them, whatever the client asks.
    const api = createApi(backend);
    for (const path of ["../notes/todo.md", "/home/you/.ssh/config", "../../../etc/passwd"]) {
      await expect(api.media.read("x1", { id: MEDIA, blob: null, path, offset: 0, length: null, outside: false })).rejects.toThrow(/outside|no such/);
    }
    expect((await api.media.stat("x1", MEDIA, ["../notes/todo.md", "/home/you/.ssh/config", "src/app.ts"])).files.map((f) => f.path)).toEqual(["src/app.ts"]);
    // A coloured fenced block in the reply; a chip with a line number opens at that line.
    await waitFor(() => expect(document.querySelector('[data-testid="md-code"] .hljs-keyword')).toBeTruthy());
    await user.click(within(chips.find((c) => c.textContent?.startsWith("app.ts:2"))!).getByRole("button"));
    const code = await screen.findByTestId("preview-text");
    await waitFor(() => expect(code.querySelector('[data-ln="2"]')!.className).toContain("focus"));
  });
});
