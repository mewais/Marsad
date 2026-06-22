/**
 * Markdown rendering for assistant transcript messages.
 *
 * Assistant replies are GitHub-flavored Markdown; the transcript must render them
 * as formatted HTML (headings, bold, code, lists, links) rather than raw text.
 */
import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { Markdown } from "../src/components/Markdown";

describe("Markdown", () => {
  it("renders inline formatting (bold + inline code)", () => {
    const { container } = render(
      <Markdown text="This is **bold** and `code`." />
    );
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    expect(container.querySelector("code")?.textContent).toBe("code");
  });

  it("renders headings, lists, and fenced code blocks", () => {
    const md = ["# Title", "", "- one", "- two", "", "```", "let x = 1;", "```"].join(
      "\n"
    );
    const { container } = render(<Markdown text={md} />);
    expect(container.querySelector("h1")?.textContent).toBe("Title");
    expect(container.querySelectorAll("li")).toHaveLength(2);
    expect(container.querySelector("pre code")?.textContent).toContain("let x = 1;");
  });

  it("renders GFM tables (remark-gfm) and external links", () => {
    const md = ["| a | b |", "| - | - |", "| 1 | 2 |"].join("\n");
    const { container } = render(<Markdown text={md} />);
    expect(container.querySelector("table")).toBeTruthy();
    expect(container.querySelectorAll("td")).toHaveLength(2);

    const { container: c2 } = render(
      <Markdown text="[site](https://example.com)" />
    );
    const a = c2.querySelector("a");
    expect(a?.getAttribute("href")).toBe("https://example.com");
    expect(a?.getAttribute("target")).toBe("_blank");
  });

  it("does not render raw HTML (no injection from model output)", () => {
    const { container } = render(
      <Markdown text={"<script>alert(1)</script> safe"} />
    );
    expect(container.querySelector("script")).toBeNull();
  });
});
