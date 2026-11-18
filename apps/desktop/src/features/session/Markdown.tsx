import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** Assistant prose. Raw HTML in the text is not rendered (react-markdown default). */
export function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: ({ children, href }) => <a href={href} target="_blank" rel="noreferrer noopener">{children}</a> }}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
