import { useContext, useState } from "react";
import { errorMessage } from "../../bridge/api";
import { copyText } from "../../lib/clipboard";
import { IconButton, Menu, toast, type MenuItem } from "../../ui";
import type { TranscriptScope } from "../../state/scope";
import { quotableText, markdownOf, rowHasOutput, rowOutputText } from "./itemText";
import { quoteInReply } from "./draft";
import { RawEventDrawer } from "./RawEventDrawer";
import { TranscriptCtx } from "./transcriptContext";
import type { Part, Span } from "./transcript";
import type { RowItem } from "./toolRuns";
import "./MessageMenu.css";

/** The cursor span of a row: a tool run's is the span of its entries together. */
function rowSpan(row: RowItem): Span | undefined {
  if (row.kind !== "toolrun") return "span" in row ? row.span : undefined;
  const spans = row.entries.map((e) => e.span);
  return spans.length ? { from: Math.min(...spans.map((s) => s.from)), to: Math.max(...spans.map((s) => s.to)) } : undefined;
}

/** The composer's message box: where a quote lands and takes the focus. */
const COMPOSER_SELECTOR = 'textarea[aria-label="Message"]';

/**
 * The ⋯ menu on a transcript row: copy the item as markdown, copy what its tool calls produced, quote it in the reply, or open the
 * raw events behind it. It is reached by hover or by keyboard focus (Tab to the button, then the arrow keys in the menu).
 * Quote and raw events need the session the transcript belongs to (`TranscriptCtx`); without one they are not offered.
 */
export function MessageMenu({ row, part, scope }: { row: RowItem; part: Part; scope: TranscriptScope }) {
  const ctx = useContext(TranscriptCtx);
  const [raw, setRaw] = useState(false);
  const quotable = quotableText(row);
  const span = rowSpan(row);
  // Text is built when an action is chosen, not per render: a long tool output is not turned into text for every row on screen.
  const copy = (what: string, build: () => string) => {
    let text: string;
    try {
      text = build();
    } catch (e) {
      toast({ tone: "err", title: `Could not copy ${what}`, detail: errorMessage(e), ttl: 6000 });
      return;
    }
    copyText(text).then(
      () => toast({ tone: "ok", title: `${what} copied`, ttl: 2000 }),
      (e: unknown) => toast({ tone: "err", title: "Could not copy", detail: errorMessage(e), ttl: 6000 }),
    );
  };

  const items: MenuItem[] = [{ id: "markdown", label: "Copy as markdown", icon: "copy", onSelect: () => copy("Markdown", () => markdownOf(row)) }];
  if (rowHasOutput(row)) items.push({ id: "output", label: "Copy tool output", icon: "copy", onSelect: () => copy("Tool output", () => rowOutputText(row)) });
  if (ctx && quotable) {
    items.push({
      id: "quote",
      label: "Quote in reply",
      icon: "edit",
      onSelect: () => {
        // An open composer takes the quote into its text; with none open the draft is kept for when it opens, so the composer is focused then.
        if (quoteInReply(ctx.key, quotable)) document.querySelector<HTMLTextAreaElement>(COMPOSER_SELECTOR)?.focus();
      },
    });
  }
  if (ctx && span) items.push({ id: "raw", label: "Open raw event", icon: "file", onSelect: () => setRaw(true) });

  return (
    <>
      <Menu
        label="Message actions"
        align="right"
        items={items}
        trigger={({ props }) => <IconButton icon="dots" label="Message actions" className="row-menu-btn" {...props} />}
      />
      {raw && ctx && span ? <RawEventDrawer sessionKey={ctx.key} host={ctx.host} id={ctx.id} scope={scope} part={part} span={span} onClose={() => setRaw(false)} /> : null}
    </>
  );
}
