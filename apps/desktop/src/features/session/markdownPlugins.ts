/** The few mdast fields these transforms touch (the full types are not a direct dependency). */
interface MdNode {
  type: string;
  value?: string;
  children?: MdNode[];
}

/** Parents whose children are blocks: HTML directly under them is a block of its own. */
const FLOW = new Set(["root", "blockquote", "listItem"]);

/**
 * Raw HTML in chat text is text. Without this it would either be deleted (`skipHtml`: "fix the
 * <Button> component" loses a word) or travel on as `raw` nodes; as plain text nodes it is shown
 * literally and can never become an element. Block-level HTML gets its own paragraph.
 */
export function remarkHtmlAsText() {
  const walk = (node: MdNode) => {
    const kids = node.children;
    if (!kids) return;
    kids.forEach((k, i) => {
      if (k.type !== "html") return walk(k);
      const text: MdNode = { type: "text", value: k.value ?? "" };
      kids[i] = FLOW.has(node.type) ? { type: "paragraph", children: [text] } : text;
    });
  };
  return (tree: MdNode) => walk(tree);
}

/**
 * Keep the author's line breaks: markdown folds a single newline into a space, which is right for
 * prose a model wraps but wrong for a prompt somebody typed line by line. Every newline inside a
 * text run becomes a hard break (`<br>`); code spans and fences are not text nodes and keep theirs.
 */
export function remarkSoftBreaks() {
  const walk = (node: MdNode) => {
    if (!node.children) return;
    const out: MdNode[] = [];
    for (const k of node.children) {
      if (k.type === "text" && k.value?.includes("\n")) {
        k.value.split("\n").forEach((part, i) => {
          if (i) out.push({ type: "break" });
          if (part) out.push({ type: "text", value: part });
        });
      } else {
        walk(k);
        out.push(k);
      }
    }
    node.children = out;
  };
  return (tree: MdNode) => walk(tree);
}
