import { Children, isValidElement, memo, type ReactElement, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Code } from "../highlight/Code";
import "./markdown.css";

/** Text of a React subtree (the code of a fenced block arrives as a string child). */
function textOf(n: ReactNode): string {
  if (n == null || typeof n === "boolean") return "";
  if (typeof n === "string" || typeof n === "number") return String(n);
  if (Array.isArray(n)) return n.map(textOf).join("");
  if (isValidElement(n)) return textOf((n as ReactElement<{ children?: ReactNode }>).props.children);
  return "";
}

/** A fenced block: highlighted by its tag (auto-detected when there is none). */
function FencedCode({ children }: { children?: ReactNode }) {
  const el = Children.toArray(children)[0] as ReactElement<{ className?: string; children?: ReactNode }> | undefined;
  const cls = isValidElement(el) ? el.props.className ?? "" : "";
  const tag = /language-(\S+)/.exec(cls)?.[1] ?? null;
  const text = textOf(isValidElement(el) ? el.props.children : children).replace(/\n$/, "");
  return (
    <div className="md-code" data-testid="md-code">
      {tag ? <span className="md-lang">{tag}</span> : null}
      <Code text={text} lang={tag} />
    </div>
  );
}

const COMMON: Components = {
  pre: FencedCode,
  // Inline code (fenced code never reaches here: `pre` takes it).
  code: ({ children, className }) => <code className={className ? `${className} md-ic` : "md-ic"}>{children}</code>,
  table: ({ children }) => <div className="md-table"><table>{children}</table></div>,
};

const CHAT: Components = {
  ...COMMON,
  a: ({ children, href }) => <a href={href} target="_blank" rel="noreferrer noopener">{children}</a>,
};

/** A file from disk: links are inert, images never load (no remote content), raw HTML is dropped. */
const FILE: Components = {
  ...COMMON,
  img: ({ alt }) => <span className="faint">[image{alt ? `: ${alt}` : ""}]</span>,
  a: ({ children, href }) => <span className="pv-link" title={href}>{children}</span>,
};

/**
 * Markdown prose. Raw HTML is never rendered. `file` mode (previews of untrusted files) also
 * makes links inert and drops images. Memoised per text, so a stream of unrelated events does
 * not re-render (or re-highlight) finished messages.
 */
export const Markdown = memo(function Markdown({ text, variant = "chat" }: { text: string; variant?: "chat" | "file" }) {
  return (
    <div className={`md${variant === "file" ? " pv-md" : ""}`} data-testid={variant === "file" ? "preview-markdown" : undefined}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml components={variant === "file" ? FILE : CHAT}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
