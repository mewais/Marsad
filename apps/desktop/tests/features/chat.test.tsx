import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Api } from "../../src/bridge/api";
import { Markdown } from "../../src/features/session/Markdown";
import { activeLink, SafeLink } from "../../src/features/session/SafeLink";
import { TranscriptItem } from "../../src/features/session/TranscriptView";
import type { Item } from "../../src/features/session/transcript";
import { AppProvider, type AppContextValue } from "../../src/state";
import { toastStore } from "../../src/ui";
import { quietBackend, renderApp, sessionByName } from "../helpers";

const PROMPT = ["Please do these:", "- first", "- second `code`", "", "```ts", "const a = 1;", "```", "line one", "line two", "fix the <Button> component"].join("\n");

describe("markdown: the user's own prompt", () => {
  it("renders lists, code blocks and code spans, keeps single line breaks and shows <Tag> as text", async () => {
    const { container } = render(<Markdown text={PROMPT} variant="user" />);
    expect(Array.from(container.querySelectorAll("ul > li")).map((li) => li.textContent)).toEqual(["first", "second code"]);
    expect(container.querySelector("li code.md-ic")!.textContent).toBe("code");
    const block = container.querySelector('[data-testid="md-code"]')!;
    expect(block.textContent).toContain("const a = 1;");
    await waitFor(() => expect(block.querySelector(".hljs-keyword")).toBeTruthy());
    // a single newline stays a visible line break
    const p = Array.from(container.querySelectorAll("p")).find((x) => x.textContent?.startsWith("line one"))!;
    expect(p.innerHTML).toMatch(/line one<br>\s*line two<br>\s*fix the &lt;Button&gt; component/);
    // raw HTML is literal text, not an element and not deleted
    expect(p.textContent!.replace(/\n/g, "")).toBe("line oneline twofix the <Button> component");
    expect(p.querySelector("button")).toBeNull();
  });

  it("keeps the breaks of a plain multi-line prompt, including between a sentence and a list", () => {
    const { container } = render(<Markdown text={"a\nb\nc"} variant="user" />);
    expect(container.querySelectorAll("p br").length).toBe(2);
    expect(container.querySelector("p")!.textContent!.replace(/\n/g, "")).toBe("abc");
  });

  it("block-level HTML in a prompt is shown as text, line breaks included", () => {
    const { container } = render(<Markdown text={"<div>\n<p>hi</p>\n</div>"} variant="user" />);
    expect(container.querySelector("div.md > div")).toBeNull();
    expect(container.textContent!.replace(/\n/g, "")).toBe("<div><p>hi</p></div>");
    expect(container.querySelectorAll("br").length).toBe(2);
  });

  it("the assistant variant is unchanged except that literal HTML is now visible as text (and still inert)", () => {
    const { container } = render(<Markdown text={"Use the <Button> and <script>alert(1)</script> here.\n\nnext\nline"} />);
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("p button")).toBeNull();
    expect(container.textContent).toContain("Use the <Button> and <script>alert(1)</script> here.");
    expect(container.querySelectorAll("br").length).toBe(0); // prose is still reflowed
  });

  it("file previews keep dropping raw HTML", () => {
    const { container } = render(<Markdown text={"x <Button> y\n\n<script>alert(1)</script>"} variant="file" />);
    expect(container.textContent).not.toContain("<Button>");
    expect(container.textContent).not.toContain("alert(1)");
  });
});

describe("links: one SafeLink", () => {
  afterEach(() => {
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    toastStore.set([]);
  });
  const withApi = (open: (t: string) => Promise<null>, ui: React.ReactNode) =>
    render(<AppProvider value={{ api: { desktop: { open } } as unknown as Api } as AppContextValue}>{ui}</AppProvider>);

  it("only http, https and mailto are links", () => {
    expect(activeLink("https://example.com/a?b=1")).toMatchObject({ href: "https://example.com/a?b=1" });
    expect(activeLink("  http://example.com ")).toMatchObject({ href: "http://example.com" });
    expect(activeLink("mailto:me@example.com")).not.toBeNull();
    for (const bad of ["javascript:alert(1)", "JaVaScRiPt:alert(1)", "file:///etc/passwd", "data:text/html,<b>x</b>", "vbscript:x", "ftp://example.com", "tauri://localhost", "//example.com", "/etc/passwd", "docs/readme.md", "#top", "", undefined]) {
      expect(activeLink(bad), String(bad)).toBeNull();
    }
  });

  it("a refused scheme renders its text and no anchor, in assistant and user text", () => {
    const md = "[a](file:///etc/passwd) [b](ftp://x.example) [c](docs/readme.md) [d](https://ok.example)";
    for (const variant of ["chat", "user"] as const) {
      const { container, unmount } = render(<Markdown text={md} variant={variant} />);
      expect(Array.from(container.querySelectorAll("a")).map((a) => a.getAttribute("href"))).toEqual(["https://ok.example"]);
      expect(container.textContent).toBe("a b c d");
      unmount();
    }
    const { container } = render(<SafeLink href="javascript:alert(1)">click</SafeLink>);
    expect(container.querySelector("a")).toBeNull();
    expect(container.textContent).toBe("click");
  });

  it("in the shell a click goes through the native opener (normalised), never the webview", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const open = vi.fn().mockResolvedValue(null);
    const winOpen = vi.spyOn(window, "open");
    withApi(open, <Markdown text="[docs](HTTPS://Example.com/x) and <mailto:me@example.com>" />);
    const a = screen.getByRole("link", { name: "docs" });
    expect(a).toHaveAttribute("rel", expect.stringContaining("noopener"));
    expect(fireEvent.click(a)).toBe(false); // default prevented: the webview does not navigate
    expect(open).toHaveBeenCalledWith("https://example.com/x");
    fireEvent.click(screen.getByRole("link", { name: "mailto:me@example.com" }));
    expect(open).toHaveBeenLastCalledWith("mailto:me@example.com");
    expect(winOpen).not.toHaveBeenCalled();
    winOpen.mockRestore();
  });

  it("a failed native open is reported, not swallowed", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    withApi(() => Promise.reject(new Error("no default browser")), <SafeLink href="https://example.com">go</SafeLink>);
    fireEvent.click(screen.getByRole("link", { name: "go" }));
    await waitFor(() => expect(toastStore.get()[0]).toMatchObject({ tone: "err", title: "Could not open the link", detail: "no default browser" }));
  });

  it("in a browser tab it opens a detached window", () => {
    const winOpen = vi.spyOn(window, "open").mockReturnValue(null);
    render(<SafeLink href="https://example.com">go</SafeLink>);
    fireEvent.click(screen.getByRole("link", { name: "go" }));
    expect(winOpen).toHaveBeenCalledWith("https://example.com/", "_blank", "noopener,noreferrer");
    winOpen.mockRestore();
  });

  it("inline code inside a link stays plain: the link is the one control", () => {
    render(<Markdown text="[`foo`](https://example.com)" />);
    expect(screen.getByRole("link")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });
});

describe("copy buttons", () => {
  const writeText = vi.fn();
  const execCommand = Object.getOwnPropertyDescriptor(document, "execCommand");
  beforeEach(() => {
    writeText.mockReset().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    toastStore.set([]);
  });
  afterEach(() => {
    if (execCommand) Object.defineProperty(document, "execCommand", execCommand);
    else delete (document as { execCommand?: unknown }).execCommand;
  });
  const user: Item = { kind: "user", key: "1", text: "Do **this**:\n- a\n- b\n\n`x`", delivery: { state: "sent" }, attachments: [] };
  const ai: Item = { kind: "assistant", key: "2", text: "Here is `cfg.name` and\n\n```sh\nnpm test -- --run\n```\n\n- one\n- two" };

  it("copies the user's prompt as typed (the markdown source, not the rendered text)", async () => {
    render(<TranscriptItem item={user} />);
    await userEvent.click(screen.getByRole("button", { name: "Copy prompt" }));
    expect(writeText).toHaveBeenCalledWith("Do **this**:\n- a\n- b\n\n`x`");
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("copies an assistant response's markdown source", async () => {
    render(<TranscriptItem item={ai} />);
    await userEvent.click(screen.getByRole("button", { name: "Copy response" }));
    expect(writeText).toHaveBeenCalledWith(ai.kind === "assistant" ? ai.text : "");
  });

  it("copies a code block exactly, without the fence or the trailing newline", async () => {
    render(<TranscriptItem item={ai} />);
    const block = screen.getByTestId("md-code");
    await userEvent.click(within(block).getByRole("button", { name: "Copy code" }));
    expect(writeText).toHaveBeenCalledWith("npm test -- --run");
  });

  it("copies an inline code span on click and on Enter, and says Copied only afterwards", async () => {
    // (userEvent.setup() would replace the clipboard stub this test installs)
    const u = userEvent;
    let done!: () => void;
    writeText.mockReturnValueOnce(new Promise<void>((r) => (done = r)));
    render(<TranscriptItem item={ai} />);
    const ic = screen.getByRole("button", { name: "Copy cfg.name" });
    expect(ic.tagName).toBe("CODE");
    await u.click(ic);
    expect(writeText).toHaveBeenCalledWith("cfg.name");
    expect(screen.queryByText("Copied")).toBeNull();
    await act(async () => done());
    expect(await screen.findByRole("button", { name: "Copied" })).toHaveTextContent("Copied");
    writeText.mockClear();
    screen.getByRole("button", { name: "Copied" }).focus();
    await u.keyboard("{Enter}");
    expect(writeText).toHaveBeenCalledWith("cfg.name");
  });

  it("a prompt's inline code span copies just that span", async () => {
    render(<TranscriptItem item={user} />);
    await userEvent.click(screen.getByRole("button", { name: "Copy x" }));
    expect(writeText).toHaveBeenCalledWith("x");
  });

  it("failure is shown as failure: no Copied, a warning state and an error toast", async () => {
    writeText.mockRejectedValue(new Error("clipboard denied"));
    Object.defineProperty(document, "execCommand", { configurable: true, value: () => false });
    render(<TranscriptItem item={user} />);
    await userEvent.click(screen.getByRole("button", { name: "Copy prompt" }));
    expect(await screen.findByRole("button", { name: "Copy failed" })).toHaveAttribute("data-state", "failed");
    expect(screen.queryByRole("button", { name: "Copied" })).toBeNull();
    expect(toastStore.get()[0]).toMatchObject({ tone: "err", title: "Could not copy" });
  });

  it("a prompt with only attachments has nothing to copy", () => {
    render(<TranscriptItem item={{ ...user, text: "" }} />);
    expect(screen.queryByRole("button", { name: "Copy prompt" })).toBeNull();
  });

  it("tool code (a shell command) is copyable too", async () => {
    const tool: Item = { kind: "tool", key: "3", callId: "c", name: "Bash", input: { command: "ls -la\necho hi" }, subagent: false, done: true, ok: true, result: "x", parts: [] };
    render(<TranscriptItem item={tool} />);
    await userEvent.click(screen.getByRole("button", { name: /Bash/ }));
    await userEvent.click(within(screen.getByTestId("tool-command").parentElement!).getByRole("button", { name: "Copy code" }));
    expect(writeText).toHaveBeenCalledWith("ls -la\necho hi");
  });
});

describe("the transcript shows what was sent", () => {
  it("a multi-line prompt with a list sent from the composer is rendered as markdown with its line breaks", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const s = sessionByName(backend, "docs-search-index");
    await renderApp(`/session/${s.host}/${s.id}`, backend);
    await screen.findByRole("heading", { name: "docs-search-index" });
    const box = screen.getByRole("textbox", { name: "Message" });
    await user.type(box, "Do this:{Shift>}{Enter}{/Shift}- one{Shift>}{Enter}{/Shift}- two{Shift>}{Enter}{/Shift}then <Tag>{Enter}");
    await waitFor(() => expect(backend.requests.some((r) => (r.params as { command?: { text?: string } })?.command?.text === "Do this:\n- one\n- two\nthen <Tag>")).toBe(true));
    const bubble = await waitFor(() => {
      const el = Array.from(document.querySelectorAll(".msg-user")).find((e) => e.textContent?.includes("Do this:"));
      if (!el) throw new Error("no bubble yet");
      return el as HTMLElement;
    });
    expect(Array.from(bubble.querySelectorAll("li")).map((li) => li.textContent)).toEqual(["one", "two\nthen <Tag>"]);
    expect(within(bubble).getByRole("button", { name: "Copy prompt" })).toBeInTheDocument();
  });
});
