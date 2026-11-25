import type { ContentPart, Usage } from "../../bridge/wire";
import { elapsed, money } from "../../lib/format";
import type { RowItem } from "./toolRuns";
import { resultText, toolArg, type ShellItem, type ToolItem, type TurnItem } from "./transcript";

export type DiffPart = Extract<ContentPart, { type: "diff" }>;

/** The diff parts of a tool result, in the order the driver reported them. */
export const diffPartsOf = (parts: readonly ContentPart[]): DiffPart[] => parts.filter((p): p is DiffPart => p.type === "diff");

/** A token count with one decimal above a thousand: `380`, `1.2k`, `3.4M`. */
export function tokenCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

/**
 * The turn footer's words: how long the turn ran (or that it was interrupted), then what the turn alone used, when the driver reported
 * it. Cost is left out when the driver does not report it (0), and a non-zero cost too small to show reads as "<$0.01".
 */
export function turnWords(item: TurnItem): string {
  const took = item.duration === undefined ? "" : elapsed(item.duration);
  const words = item.interrupted ? (took ? `Interrupted after ${took}` : "Interrupted") : took ? `Done in ${took}` : "Done";
  return [words, ...usageWords(item.usage)].join(" · ");
}

function usageWords(u: Usage | undefined): string[] {
  if (!u) return [];
  const out = [`${tokenCount(u.input_tokens)} in / ${tokenCount(u.output_tokens)} out`];
  if (u.cost_usd > 0) out.push(u.cost_usd < 0.01 ? "<$0.01" : money(u.cost_usd));
  return out;
}

/** A diff part as the text it stands for: its hunk when the driver gave one, else the `-` old lines then the `+` new lines. */
export function diffPartText(p: DiffPart): string {
  if (p.hunk) return p.hunk;
  const old = p.old_text == null || p.old_text === "" ? [] : p.old_text.split("\n").map((l) => `-${l}`);
  const neu = p.new_text === "" ? [] : p.new_text.split("\n").map((l) => `+${l}`);
  return [...old, ...neu].join("\n");
}

/** What a tool call produced, as text: its result, then each file change it made. */
export function toolOutputText(item: ToolItem): string {
  const diffs = diffPartsOf(item.parts).map(diffPartText);
  return [resultText(item.result), ...diffs].filter((t) => t.length > 0).join("\n");
}

/** A fence longer than any backtick run in `text`, so the text cannot close it. */
function fenceFor(text: string): string {
  const runs = text.match(/`+/g) ?? [];
  return "`".repeat(Math.max(3, ...runs.map((r) => r.length + 1)));
}

function toolMarkdown(item: ToolItem): string {
  const arg = toolArg(item.input);
  const head = `**${item.name}**${arg ? ` ${arg}` : ""}`;
  const out = toolOutputText(item);
  if (!out) return head;
  const f = fenceFor(out);
  return `${head}\n\n${f}\n${out}\n${f}`;
}

/** A shell command as markdown: a fenced block of `$ command` then what it printed, with a note when the output was cut. */
export function shellMarkdown(item: ShellItem): string {
  const out = item.output.replace(/\n$/, "");
  const body = out ? `$ ${item.command}\n${out}` : `$ ${item.command}`;
  const f = fenceFor(body);
  return `${f}\n${body}\n${f}${item.outputTruncated ? "\n\n*Output truncated.*" : ""}`;
}

/** The item as markdown, for pasting elsewhere: prose as it was written, a tool call as its name, argument and output. */
export function markdownOf(row: RowItem): string {
  switch (row.kind) {
    case "user":
    case "assistant":
    case "thinking":
    case "notice":
      return row.text;
    case "media":
      return row.parts
        .map((p) => (p.type === "text" ? p.text : p.type === "diff" ? diffPartText(p) : p.type === "file" ? `[file: ${p.name ?? "file"}]` : `[${p.type}]`))
        .join("\n\n");
    case "tool":
      return toolMarkdown(row);
    case "toolrun":
      return row.calls.map(toolMarkdown).join("\n\n");
    case "turn":
      return turnWords(row);
    case "shell":
      return shellMarkdown(row);
  }
}

/** The text a quote in the reply carries: what was written (prose), or a notice's words. Empty when the item has no prose. */
export function quotableText(row: RowItem): string {
  switch (row.kind) {
    case "user":
    case "assistant":
    case "thinking":
    case "notice":
      return row.text;
    case "media":
      return row.parts.flatMap((p) => (p.type === "text" ? [p.text] : [])).join("\n\n");
    default:
      return "";
  }
}

/** What a row's tool calls produced, as text (empty for a row that is not a tool call). */
export function rowOutputText(row: RowItem): string {
  if (row.kind === "tool") return toolOutputText(row);
  if (row.kind === "toolrun") return row.calls.map(toolOutputText).filter((t) => t.length > 0).join("\n");
  return "";
}

/** Whether a row's tool calls produced anything to copy (a result or a file change), checked without building the text. */
export function rowHasOutput(row: RowItem): boolean {
  const has = (r: ToolItem) => (r.result !== undefined && r.result !== null) || r.parts.some((p) => p.type === "diff");
  if (row.kind === "tool") return has(row);
  if (row.kind === "toolrun") return row.calls.some(has);
  return false;
}
