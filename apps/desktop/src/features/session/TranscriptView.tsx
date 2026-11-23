import { memo, useMemo, useState, type ReactNode } from "react";
import { Code, Tokens, useHighlighted } from "../highlight/Code";
import { languageFromName } from "../highlight/lang";
import { CopyButton, DriverBadge, Icon, StatusPill, type DriverDescriptor } from "../../ui";
import { Markdown } from "./Markdown";
import { AttachmentsView, PartsView } from "../media/Parts";
import { useMediaScope } from "../media/Parts";
import { parsePathRefs } from "../media/pathRefs";
import { FileRefs, toolFileRefs } from "../media/refs";
import { deliveryNote, resultText, toolArg, type Item, type ToolItem } from "./transcript";
import type { RowItem, ToolRunItem } from "./toolRuns";

const DIFF_CAP = 2000;

/** Old and new text of an edit as `-` / `+` rows, each side coloured by the file's language. */
function EditDiff({ oldText, newText, file }: { oldText: string; newText: string; file: string }) {
  const lang = languageFromName(file);
  const o = useHighlighted(oldText, lang);
  const n = useHighlighted(newText, lang);
  return (
    <div className="diff" data-testid="tool-diff">
      {o.lines.slice(0, DIFF_CAP).map((l, i) => <div key={`d${i}`} className="ln d"><span /><span /><span>-</span><span><Tokens line={l} /></span></div>)}
      {n.lines.slice(0, DIFF_CAP).map((l, i) => <div key={`a${i}`} className="ln a"><span /><span /><span>+</span><span><Tokens line={l} /></span></div>)}
    </div>
  );
}

/** Does a tool result look like a unified diff? */
export function looksLikeDiff(text: string): boolean {
  return /^(diff --git |@@ -\d)/m.test(text) || (/^--- .+\n\+\+\+ /m.test(text) && /^@@ /m.test(text));
}

/** A unified diff as text: additions and deletions coloured, hunk headers dimmed. */
function DiffText({ text }: { text: string }) {
  const lines = useMemo(() => text.split("\n").slice(0, DIFF_CAP), [text]);
  return (
    <div className="diff" data-testid="tool-diff-text">
      {lines.map((l, i) => {
        const head = /^(diff --git|index |--- |\+\+\+ |new file|deleted file|similarity)/.test(l);
        if (l.startsWith("@@") || head) return <div key={i} className="hunk">{l}</div>;
        const k = l.startsWith("+") ? "a" : l.startsWith("-") ? "d" : "";
        return <div key={i} className={`ln${k ? ` ${k}` : ""}`}><span /><span /><span>{k ? l[0] : ""}</span><span>{k ? l.slice(1) : l}</span></div>;
      })}
    </div>
  );
}

function ToolBody({ item }: { item: ToolItem }) {
  const i = item.input as { old_string?: string; new_string?: string; file_path?: string; path?: string; content?: string; command?: string } | undefined;
  const file = i?.file_path ?? i?.path ?? "";
  if (typeof i?.old_string === "string" && typeof i.new_string === "string") return <EditDiff oldText={i.old_string} newText={i.new_string} file={file} />;
  const text = resultText(item.result);
  // A file a tool writes is shown by its extension; a shell command as shell.
  const written = typeof i?.content === "string" && file ? <Code text={i.content} name={file} lineNumbers className="tool-code" testId="tool-write" copyable /> : null;
  const command = typeof i?.command === "string" ? <Code text={i.command} lang="bash" className="tool-code" testId="tool-command" copyable /> : null;
  let result: ReactNode;
  if (!text) result = <div className="tool-b">{item.done ? (item.parts.length ? "" : "(no output)") : "Running…"}</div>;
  else if (looksLikeDiff(text)) result = <DiffText text={text} />;
  else if (/^\s*[[{]/.test(text) && text.length < 200_000 && isJson(text)) result = <Code text={text} lang="json" className="tool-code" testId="tool-json" copyable />;
  else result = <div className="tool-b">{text}</div>;
  return <>{command}{written}{result}</>;
}

const isJson = (t: string) => {
  try {
    JSON.parse(t);
    return true;
  } catch {
    return false;
  }
};

/** What a tool call shows without being opened: the images and files it produced, and the files it wrote. */
function ToolOutputs({ item }: { item: ToolItem }) {
  const files = useMemo(() => (item.done && item.ok !== false ? toolFileRefs(item.name, item.input) : []), [item.done, item.ok, item.name, item.input]);
  return (
    <>
      <PartsView parts={item.parts} />
      {files.length ? <FileRefs paths={files} label="Files this tool wrote" /> : null}
    </>
  );
}

const Tool = memo(function Tool({ item }: { item: ToolItem }) {
  const [open, setOpen] = useState(false);
  const icon = !item.done ? "working" : item.ok === false ? "failed" : "done";
  const tone = !item.done ? "c-working" : item.ok === false ? "c-failed" : "c-done";
  const first = resultText(item.result).split("\n")[0].slice(0, 60) || (item.parts.length ? `${item.parts.length} attachment${item.parts.length > 1 ? "s" : ""}` : "");
  return (
    <div className="tool">
      <button type="button" className="tool-h plainbtn wfull" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Icon name={icon} size="sm" className={tone} spin={!item.done} />
        <span className="nm">{item.name}</span>
        <span className="arg">{toolArg(item.input)}</span>
        <span className="res">{item.done ? (item.ok === false ? "failed · " : "") + first : item.status ?? "running"}</span>
        <Icon name={open ? "chev" : "chevr"} size="xs" className="faint" />
      </button>
      {open ? <ToolBody item={item} /> : null}
      <ToolOutputs item={item} />
    </div>
  );
});

/**
 * Consecutive tool calls as one row: the count and the latest call, expanding to the calls themselves.
 * Its expanded state lives here and the row keeps its key while the run grows, so new calls update the
 * collapsed row in place without closing or opening it.
 */
const ToolRun = memo(function ToolRun({ item }: { item: ToolRunItem }) {
  const [open, setOpen] = useState(false);
  const latest = item.calls[item.calls.length - 1];
  const running = item.calls.some((c) => !c.done);
  const failed = item.calls.filter((c) => c.done && c.ok === false).length;
  const icon = running ? "working" : latest.ok === false ? "failed" : "done";
  const tone = running ? "c-working" : latest.ok === false ? "c-failed" : "c-done";
  return (
    <div className="tool run" data-testid="tool-run">
      <button type="button" className="tool-h plainbtn wfull" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Icon name={icon} size="sm" className={tone} spin={running} />
        <span className="nm">{item.calls.length} tool calls</span>
        <span className="arg" data-testid="tool-run-latest">{latest.name} {toolArg(latest.input)}</span>
        <span className="res">{running ? latest.status ?? "running" : failed ? `${failed} failed` : "done"}</span>
        <Icon name={open ? "chev" : "chevr"} size="xs" className="faint" />
      </button>
      {open ? (
        <div className="run-b">
          {item.entries.map((e) => (e.kind === "tool" ? <Tool key={e.key} item={e} /> : <Thinking key={e.key} text={e.text} />))}
        </div>
      ) : (
        // What the calls produced stays in view; opened, each call shows its own.
        item.calls.map((c) => <ToolOutputs key={c.key} item={c} />)
      )}
    </div>
  );
});

function Subagent({ item }: { item: ToolItem }) {
  const i = item.input as { description?: string; subagent_type?: string };
  return (
    <div className="subag" data-testid="subagent">
      <div className="subag-h">
        <Icon name="sub" size="sm" />
        <b>Subagent · {i.subagent_type ?? item.name}</b>
        <span className="muted">“{i.description ?? toolArg(item.input)}”</span>
        <span className="flex1" />
        <StatusPill status={item.done ? (item.ok === false ? "failed" : "done") : "working"} label={item.done ? (item.ok === false ? "Failed" : "Finished") : "Running"} />
      </div>
      {item.done && typeof item.result === "string" ? <Markdown text={item.result} /> : null}
    </div>
  );
}

function Thinking({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button type="button" className="think plainbtn" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Icon name={open ? "chev" : "chevr"} size="xs" />
        <Icon name="sparkle" size="xs" />
        Thinking
      </button>
      {open ? <div className="faint subopen">{text}</div> : null}
    </div>
  );
}

/** Paths in assistant prose that exist in the session's folder, as clickable chips. */
function MentionedFiles({ text }: { text: string }) {
  const scope = useMediaScope();
  const windows = !!scope?.windows;
  const { paths, lines } = useMemo(() => {
    const refs = parsePathRefs(text, windows);
    const lines: Record<string, number> = {};
    for (const r of refs) if (r.line && !(r.path in lines)) lines[r.path] = r.line;
    return { paths: [...new Set(refs.map((r) => r.path))], lines };
  }, [text, windows]);
  return paths.length ? <FileRefs paths={paths} lines={lines} label="Files mentioned" /> : null;
}

/** Under a user message: only a message that is not simply sent says anything about its delivery. */
function Delivery({ item }: { item: Extract<Item, { kind: "user" }> }) {
  if (item.delivery.state === "queued") return <div className="msg-state faint fs12" data-testid="delivery">Queued</div>;
  const note = deliveryNote(item.delivery);
  if (!note) return null;
  return (
    <div className="msg-state c-failed fs12" role="status" data-testid="delivery">
      <Icon name="warn" size="xs" /> {note.text}{note.detail ? <span className="faint"> · {note.detail}</span> : null}
    </div>
  );
}

export const TranscriptItem = memo(function TranscriptItem({ item, driver }: { item: RowItem; driver?: DriverDescriptor }) {
  switch (item.kind) {
    case "user":
      return (
        <div className="msg-user" data-delivery={item.delivery.state}>
          {item.text ? <Markdown text={item.text} variant="user" /> : null}
          <Delivery item={item} />
          <AttachmentsView attachments={item.attachments} />
          {item.text ? <CopyButton className="msg-copy" text={item.text} label="Copy prompt" /> : null}
        </div>
      );
    case "assistant":
      return (
        <div className="msg-ai">
          <div className="who">{driver ? <DriverBadge driver={driver} compact /> : null} {driver?.displayName ?? "Agent"}</div>
          <Markdown text={item.text} />
          <MentionedFiles text={item.text} />
          <CopyButton className="msg-copy" text={item.text} label="Copy response" />
        </div>
      );
    case "media":
      return item.role === "user" ? (
        <div className="msg-user"><PartsView parts={item.parts} /></div>
      ) : (
        <div className="msg-ai"><PartsView parts={item.parts} /></div>
      );
    case "thinking":
      return <Thinking text={item.text} />;
    case "tool":
      return item.subagent ? <Subagent item={item} /> : <Tool item={item} />;
    case "toolrun":
      return <ToolRun item={item} />;
    case "notice":
      return <div className={`fs12 ${item.tone === "err" ? "c-failed" : "faint"}`} role={item.tone === "err" ? "alert" : undefined}>{item.text}</div>;
  }
});
