/**
 * Markdown — renders assistant transcript text as GitHub-flavored Markdown.
 *
 * Uses react-markdown (no raw HTML by default, so claude output can't inject
 * markup) + remark-gfm (tables, strikethrough, task lists, autolinks).
 * Element styling lives in the `.md` rules in index.html so it tracks the theme.
 */
import { memo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

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
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
