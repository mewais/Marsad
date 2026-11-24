import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { TranscriptItem } from "../../src/features/session/TranscriptView";
import type { ThinkingItem } from "../../src/features/session/transcript";

const thinking = (text: string): ThinkingItem => ({ kind: "thinking", key: "th1", text, span: { from: 1, to: 1 } });

afterEach(() => {
  delete (window as { __pwned?: unknown }).__pwned;
});

describe("thinking is shown as markdown, muted", () => {
  it("renders the reasoning's markdown: emphasis is <strong>, in the muted thinking style", async () => {
    const user = userEvent.setup();
    const { container } = render(<TranscriptItem item={thinking("Weigh **the two** options:\n\n- first\n- second")} />);
    await user.click(screen.getByRole("button", { name: /Thinking/ }));
    const body = container.querySelector(".think-body .md-muted") as HTMLElement;
    expect(body).not.toBeNull();
    expect(body.querySelector("strong")).toHaveTextContent("the two");
    expect(body.querySelectorAll("li")).toHaveLength(2);
  });

  it("raw HTML in the reasoning never runs: a <script> is text, and a <strong> is shown as text, not an element", async () => {
    const user = userEvent.setup();
    const { container } = render(<TranscriptItem item={thinking('Check <script>window.__pwned = 1</script> and <img src=x onerror="window.__pwned=2"> plus <strong>tag</strong>.')} />);
    await user.click(screen.getByRole("button", { name: /Thinking/ }));
    const body = container.querySelector(".think-body") as HTMLElement;
    expect(body.querySelector("script")).toBeNull();
    expect(body.querySelector("img")).toBeNull();
    expect(body.querySelector("strong")).toBeNull();
    expect(body).toHaveTextContent("<script>window.__pwned = 1</script>");
    expect(body).toHaveTextContent("<strong>tag</strong>");
    expect((window as { __pwned?: unknown }).__pwned).toBeUndefined();
  });

  it("the reasoning keeps its own line breaks inside a paragraph, as a markdown block would", async () => {
    const user = userEvent.setup();
    const { container } = render(<TranscriptItem item={thinking("first paragraph\n\nsecond paragraph")} />);
    await user.click(screen.getByRole("button", { name: /Thinking/ }));
    expect(container.querySelectorAll(".think-body p")).toHaveLength(2);
  });
});
