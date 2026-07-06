/**
 * Markdown — renders assistant transcript text as GitHub-flavored Markdown.
 *
 * Uses react-markdown (no raw HTML by default, so claude output can't inject
 * markup) + remark-gfm (tables, strikethrough, task lists, autolinks).
 * Element styling lives in the `.md` rules in index.html so it tracks the theme.
 */
import { memo, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { CopyButton } from "./CopyButton";

// Recursively collect the raw text of a rendered node so the copy button copies
// the fenced block's exact contents (react-markdown hands us React children, not
// the source string).
function nodeText(node: ReactNode): string {
  if (node == null || node === false || node === true) return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join("");
  if (typeof node === "object" && "props" in node) {
    return nodeText((node as { props: { children?: ReactNode } }).props.children);
  }
  return "";
}

// Memoized by `text`: re-parsing Markdown is the expensive part of a transcript
// re-render, so unchanged messages must not re-parse when a new event arrives.
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          // Open links externally rather than navigating the app webview.
          a: ({ node: _node, ...props }) => (
            <a {...props} target="_blank" rel="noopener noreferrer" />
          ),
          // Fenced code blocks get a hover copy button (inline `code` does not —
          // only multi-line `<pre>` blocks route through this renderer).
          pre: ({ node: _node, children, ...props }) => (
            <div className="copy-wrap">
              <pre {...props}>{children}</pre>
              <CopyButton text={nodeText(children).replace(/\n$/, "")} />
            </div>
          ),
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
