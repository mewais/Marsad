import { Children, isValidElement, memo, useContext, type ReactElement, type ReactNode } from "react";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import remarkGfm from "remark-gfm";
import { copyName, useCopy } from "../../ui/CopyButton";
import { Code } from "../highlight/Code";
import { remarkHtmlAsText, remarkSoftBreaks } from "./markdownPlugins";
import { InLink, SafeLink } from "./SafeLink";
import "./markdown.css";

/** Text of a React subtree (the code of a fenced block arrives as a string child). */
function textOf(n: ReactNode): string {
  if (n == null || typeof n === "boolean") return "";
  if (typeof n === "string" || typeof n === "number") return String(n);
  if (Array.isArray(n)) return n.map(textOf).join("");
  if (isValidElement(n)) return textOf((n as ReactElement<{ children?: ReactNode }>).props.children);
  return "";
}

/** A fenced block: highlighted by its tag (auto-detected when there is none), with a copy button in its corner. */
function FencedCode({ children }: { children?: ReactNode }) {
  const el = Children.toArray(children)[0] as ReactElement<{ className?: string; children?: ReactNode }> | undefined;
  const cls = isValidElement(el) ? el.props.className ?? "" : "";
  const tag = /language-(\S+)/.exec(cls)?.[1] ?? null;
  const text = textOf(isValidElement(el) ? el.props.children : children).replace(/\n$/, "");
  return (
    <div className="md-code" data-testid="md-code">
      {tag ? <span className="md-lang">{tag}</span> : null}
      <Code text={text} lang={tag} copyable />
    </div>
  );
}

/**
 * Inline code: click (or Enter / Space) copies it. Quiet until pointed at: the only cue is the
 * copy cursor and a border, and a small note above it says Copied or Copy failed. Inside a link
 * the link wins and the code stays plain.
 */
function InlineCode({ children, className }: { children?: ReactNode; className?: string }) {
  const text = textOf(children);
  const inLink = useContext(InLink);
  const { state, copy } = useCopy(text);
  const cls = className ? `${className} md-ic` : "md-ic";
  if (inLink) return <code className={cls}>{children}</code>;
  return (
    <code
      className={cls}
      role="button"
      tabIndex={0}
      title="Click to copy"
      aria-label={copyName(state, `Copy ${text}`)}
      data-state={state}
      onClick={copy}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          copy();
        }
      }}
    >
      {children}
      {state === "idle" ? null : <span className="tip" aria-hidden="true">{copyName(state, "")}</span>}
    </code>
  );
}

const COMMON: Components = {
  pre: FencedCode,
  // Inline code (fenced code never reaches here: `pre` takes it).
  code: InlineCode,
  table: ({ children }) => <div className="md-table"><table>{children}</table></div>,
};

const CHAT: Components = {
  ...COMMON,
  a: SafeLink,
};

/** A file from disk: links are inert, images never load (no remote content), raw HTML is dropped. */
const FILE: Components = {
  ...COMMON,
  img: ({ alt }) => <span className="faint">[image{alt ? `: ${alt}` : ""}]</span>,
  a: ({ children, href }) => <span className="pv-link" title={href}>{children}</span>,
};

const GFM: Options["remarkPlugins"] = [remarkGfm];
/** Chat text: raw HTML is shown as text. */
const CHAT_PLUGINS: Options["remarkPlugins"] = [remarkGfm, remarkHtmlAsText];
/** The user's own prompt: as chat text, and every line break they typed stays one. */
const USER_PLUGINS: Options["remarkPlugins"] = [remarkGfm, remarkHtmlAsText, remarkSoftBreaks];

/**
 * Markdown prose. `chat` is an agent's message and `user` the user's own prompt (which also keeps
 * its line breaks): both show raw HTML as literal text, never as elements; links go through
 * SafeLink. `thinking` is the agent's reasoning: chat rules, in a muted colour. `file` (previews of untrusted files) makes links inert,
 * drops images and raw HTML. Memoised per text, so a stream of unrelated events does not re-render (or re-highlight)
 * finished messages.
 */
export const Markdown = memo(function Markdown({ text, variant = "chat" }: { text: string; variant?: "chat" | "user" | "thinking" | "file" }) {
  const file = variant === "file";
  const muted = variant === "thinking";
  return (
    <div className={`md${file ? " pv-md" : ""}${muted ? " md-muted" : ""}`} data-testid={file ? "preview-markdown" : undefined}>
      <ReactMarkdown remarkPlugins={file ? GFM : variant === "user" ? USER_PLUGINS : CHAT_PLUGINS} skipHtml={file} components={file ? FILE : CHAT}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
