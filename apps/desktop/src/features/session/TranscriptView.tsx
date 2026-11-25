import { memo, useContext, useMemo, type ReactNode } from "react";
import { Code, Tokens, useHighlighted } from "../highlight/Code";
import { languageFromName } from "../highlight/lang";
import { CopyButton, DriverBadge, ErrorBoundary, Icon, StatusPill, type DriverDescriptor } from "../../ui";
import { dayTime } from "../../lib/format";
import { Markdown } from "./Markdown";
import { AttachmentsView, PartsView } from "../media/Parts";
import { useMediaScope } from "../media/Parts";
import { parsePathRefs } from "../media/pathRefs";
import { FileRefs, toolFileRefs } from "../media/refs";
import type { ContentPart, SubagentInfo, SubagentSupport } from "../../bridge/wire";
import { Link, sessionPath } from "../../router";
import { useAppState } from "../../state";
import { MAIN, type TranscriptScope } from "../../state/scope";
import { StopTask, SubagentMeta } from "./SessionWork";
import { NO_SUBAGENT_SUPPORT, subagentState } from "./tasks";
import { TranscriptCtx, type TranscriptScopeInfo } from "./transcriptContext";
import { MessageMenu } from "./MessageMenu";
import { diffPartsOf, turnWords, type DiffPart } from "./itemText";
import { useOpenRow } from "./expansion";
import { deliveryNote, resultText, toolArg, type Item, type Part, type ShellItem, type ThinkingItem, type ToolItem, type TurnItem } from "./transcript";
import type { RowItem, ToolRunItem } from "./toolRuns";

const DIFF_CAP = 2000;

/**
 * Old and new text of an edit as `-` / `+` rows, each side coloured by the file's language. No old text (`null`) means the file was
 * created: every line is an addition.
 */
function EditDiff({ oldText, newText, file }: { oldText: string | null; newText: string; file: string }) {
  const lang = languageFromName(file);
  const o = useHighlighted(oldText ?? "", lang);
  const n = useHighlighted(newText, lang);
  return (
    <div className="diff" data-testid="tool-diff">
      {oldText ? o.lines.slice(0, DIFF_CAP).map((l, i) => <div key={`d${i}`} className="ln d"><span /><span /><span>-</span><span><Tokens line={l} /></span></div>) : null}
      {n.lines.slice(0, DIFF_CAP).map((l, i) => <div key={`a${i}`} className="ln a"><span /><span /><span>+</span><span><Tokens line={l} /></span></div>)}
    </div>
  );
}

/** The file changes a tool reported, as `-` / `+` blocks: a hunk when the driver gave one, else the old and new text. */
function DiffParts({ parts }: { parts: DiffPart[] }) {
  return (
    <>
      {parts.map((p, i) => (
        <div key={i} className="diff-part" data-testid="tool-diff-part">
          <div className="hunk" data-testid="diff-path">{p.path}</div>
          {p.hunk ? <DiffText text={p.hunk} /> : <EditDiff oldText={p.old_text ?? null} newText={p.new_text} file={p.path} />}
        </div>
      ))}
    </>
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

/** The body of an opened tool call: its command or the file it wrote, the file changes it reported, then its output. */
function ToolBody({ item }: { item: ToolItem }) {
  const i = item.input as { file_path?: string; path?: string; content?: string; command?: string } | undefined;
  const file = i?.file_path ?? i?.path ?? "";
  const diffs = diffPartsOf(item.parts);
  const text = resultText(item.result);
  // A file a tool writes is shown by its extension; a shell command as shell. A change the driver reported as a diff is shown as that diff.
  const written = !diffs.length && typeof i?.content === "string" && file ? <Code text={i.content} name={file} lineNumbers className="tool-code" testId="tool-write" copyable /> : null;
  const command = typeof i?.command === "string" ? <Code text={i.command} lang="bash" className="tool-code" testId="tool-command" copyable /> : null;
  let result: ReactNode;
  if (!text) result = diffs.length ? null : <div className="tool-b">{item.done ? (item.parts.length ? "" : "(no output)") : "Running…"}</div>;
  else if (looksLikeDiff(text)) result = <DiffText text={text} />;
  else if (/^\s*[[{]/.test(text) && text.length < 200_000 && isJson(text)) result = <Code text={text} lang="json" className="tool-code" testId="tool-json" copyable />;
  else result = <div className="tool-b">{text}</div>;
  return <>{command}{written}{diffs.length ? <DiffParts parts={diffs} /> : null}{result}</>;
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
  const files = useMemo(() => (item.done && item.ok !== false ? toolFileRefs(item.name, item.input, item.parts) : []), [item.done, item.ok, item.name, item.input, item.parts]);
  return (
    <>
      <PartsView parts={withoutDiffs(item.parts)} />
      {files.length ? <FileRefs paths={files} label="Files this tool wrote" /> : null}
    </>
  );
}

const Tool = memo(function Tool({ item }: { item: ToolItem }) {
  const [open, toggle] = useOpenRow(`tool:${item.key}`);
  const icon = !item.done ? "working" : item.ok === false ? "failed" : "done";
  const tone = !item.done ? "c-working" : item.ok === false ? "c-failed" : "c-done";
  const first = resultText(item.result).split("\n")[0].slice(0, 60) || (item.parts.length ? `${item.parts.length} attachment${item.parts.length > 1 ? "s" : ""}` : "");
  return (
    <div className="tool">
      <button type="button" className="tool-h plainbtn wfull" aria-expanded={open} onClick={toggle}>
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
  const [open, toggle] = useOpenRow(`run:${item.key}`);
  const latest = item.calls[item.calls.length - 1];
  const running = item.calls.some((c) => !c.done);
  const failed = item.calls.filter((c) => c.done && c.ok === false).length;
  const icon = running ? "working" : latest.ok === false ? "failed" : "done";
  const tone = running ? "c-working" : latest.ok === false ? "c-failed" : "c-done";
  return (
    <div className="tool run" data-testid="tool-run">
      <button type="button" className="tool-h plainbtn wfull" aria-expanded={open} onClick={toggle}>
        <Icon name={icon} size="sm" className={tone} spin={running} />
        <span className="nm">{item.calls.length} tool calls</span>
        <span className="arg" data-testid="tool-run-latest">{latest.name} {toolArg(latest.input)}</span>
        <span className="res">{running ? latest.status ?? "running" : failed ? `${failed} failed` : "done"}</span>
        <Icon name={open ? "chev" : "chevr"} size="xs" className="faint" />
      </button>
      {open ? (
        <div className="run-b">
          {item.entries.map((e) => (e.kind === "tool" ? <Tool key={e.key} item={e} /> : <Thinking key={e.key} item={e} />))}
        </div>
      ) : (
        // What the calls produced stays in view; opened, each call shows its own.
        item.calls.map((c) => <ToolOutputs key={c.key} item={c} />)
      )}
    </div>
  );
});

const sameInfo = (a: SubagentInfo | undefined, b: SubagentInfo | undefined) => a === b || JSON.stringify(a) === JSON.stringify(b);

/**
 * The entry point into a subagent: the tool call that spawned it, with the subagent's state as the daemon reports it
 * (running until the agent says its task ended, however long ago the tool call returned), its tool calls and how long it
 * ran, and the way to its own transcript. A row outside a session (no daemon facts) shows only the call.
 */
function Subagent({ item }: { item: ToolItem }) {
  const scope = useContext(TranscriptCtx);
  return scope ? <LiveSubagent item={item} scope={scope} /> : <SubagentCard item={item} />;
}

function LiveSubagent({ item, scope }: { item: ToolItem; scope: TranscriptScopeInfo }) {
  const info = useAppState((s) => s.sessions[scope.key]?.subagents.find((x) => x.call_id === item.callId), sameInfo);
  const caps = useAppState((s) => s.sessions[scope.key]?.capabilities.subagents);
  return <SubagentCard item={item} info={info} caps={caps} scope={scope} />;
}

function SubagentCard({ item, info, caps, scope }: { item: ToolItem; info?: SubagentInfo; caps?: SubagentSupport; scope?: TranscriptScopeInfo }) {
  const i = (item.input ?? {}) as { description?: string; subagent_type?: string };
  const st = info ? subagentState(info) : undefined;
  const label = i.description ?? toolArg(item.input);
  return (
    <div className="subag" data-testid="subagent" data-running={info?.running ? "" : undefined}>
      <div className="subag-h">
        <Icon name="sub" size="sm" />
        <b>Subagent · {info?.subagent_type ?? i.subagent_type ?? item.name}</b>
        <span className="muted">“{label}”</span>
        <span className="flex1" />
        {st ? <span title={st.why}><StatusPill status={st.status} label={st.word} /></span> : null}
        {info ? <span className="faint" data-testid="subagent-meta"><SubagentMeta s={info} caps={caps ?? NO_SUBAGENT_SUPPORT} /></span> : null}
        {scope && info?.running && info.task_id ? <StopTask sessionKey={scope.key} taskId={info.task_id} what={`subagent ${label}`} /> : null}
        {scope ? <Link className="btn sm ghost" to={sessionPath(scope.host, scope.id, undefined, item.callId)} data-testid="subagent-open"><Icon name="chevr" size="xs" />Open transcript</Link> : null}
      </div>
      {item.done && typeof item.result === "string" ? <Markdown text={item.result} /> : null}
    </div>
  );
}

/**
 * A shell command the session ran: its command and folder, and how it ended, in one row; opened, its output (monospace, the end of
 * it is kept by the host only up to its cap, which is said). Its open state is kept in the transcript's expansion like any row's.
 */
const ShellRow = memo(function ShellRow({ item }: { item: ShellItem }) {
  const [open, toggle] = useOpenRow(`shell:${item.key}`);
  const exited = item.exitCode !== undefined;
  const tone = !exited ? "c-needs" : item.exitCode === 0 ? "c-done" : "c-failed";
  return (
    <div className="shell-row" data-testid="shell-row">
      <button type="button" className="tool-h plainbtn wfull" aria-expanded={open} onClick={toggle}>
        <span className="faint">$</span> <span className="nm">{item.command}</span>
        {item.cwd ? <span className="shell-row-cwd" title={item.cwd}>{item.cwd}</span> : null}
        <span data-testid="shell-exit" className={tone}>{exited ? `exit ${item.exitCode}` : "stopped"}</span>
        <Icon name={open ? "chev" : "chevr"} size="xs" className="faint" />
      </button>
      {open ? (
        <>
          <pre data-testid="shell-output">{item.output || "(no output)"}</pre>
          {item.outputTruncated ? <div className="shell-truncated" data-testid="shell-truncated">Output truncated: the host kept only the first part.</div> : null}
        </>
      ) : null}
    </div>
  );
});

/**
 * The model's reasoning, shown as the markdown it is written in, muted and under the row that opens it. Raw HTML in it is text
 * (see Markdown), so reasoning that quotes markup shows the markup and runs nothing.
 */
function Thinking({ item }: { item: ThinkingItem }) {
  const [open, toggle] = useOpenRow(`think:${item.key}`);
  return (
    <div>
      <button type="button" className="think plainbtn" aria-expanded={open} onClick={toggle}>
        <Icon name={open ? "chev" : "chevr"} size="xs" />
        <Icon name="sparkle" size="xs" />
        Thinking
      </button>
      {open ? <div className="think-body"><Markdown text={item.text} variant="thinking" /></div> : null}
    </div>
  );
}

/**
 * The end of a turn, in one muted line. Only what the turn reported is said: a turn whose start was not seen has no duration, a turn
 * the user stopped says so instead of "Done", and tokens and cost appear only when the driver reported them for this turn.
 */
function TurnFooter({ item }: { item: TurnItem }) {
  return <div className="turn-end faint fs12" data-testid="turn-footer" data-interrupted={item.interrupted ? "" : undefined}>{turnWords(item)}</div>;
}

/** The parts a message or tool call shows as files and media; a diff is drawn by the tool body instead. */
const withoutDiffs = (parts: ContentPart[]) => parts.filter((p) => p.type !== "diff");

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

const TranscriptItemView = memo(function TranscriptItemView({ item, driver }: { item: RowItem; driver?: DriverDescriptor }) {
  switch (item.kind) {
    case "user":
      return (
        <div className="msg-user" data-delivery={item.delivery.state}>
          {item.text ? <Markdown text={item.text} variant="user" /> : null}
          <Delivery item={item} />
          {item.at !== undefined ? <div className="msg-time faint" data-testid="msg-time">{dayTime(item.at)}</div> : null}
          <AttachmentsView attachments={item.attachments} />
          {item.text ? <CopyButton className="msg-copy" text={item.text} label="Copy prompt" /> : null}
        </div>
      );
    case "assistant":
      return (
        <div className="msg-ai">
          <div className="who">{driver ? <DriverBadge driver={driver} compact /> : null} {driver?.displayName ?? "Agent"}{item.at !== undefined ? <span data-testid="msg-time">· {dayTime(item.at)}</span> : null}</div>
          <Markdown text={item.text} />
          <MentionedFiles text={item.text} />
          <CopyButton className="msg-copy" text={item.text} label="Copy response" />
        </div>
      );
    case "media":
      return item.role === "user" ? (
        <div className="msg-user"><PartsView parts={withoutDiffs(item.parts)} /></div>
      ) : (
        <div className="msg-ai"><PartsView parts={withoutDiffs(item.parts)} /></div>
      );
    case "thinking":
      return <Thinking item={item} />;
    case "turn":
      return <TurnFooter item={item} />;
    case "tool":
      return item.subagent ? <Subagent item={item} /> : <Tool item={item} />;
    case "toolrun":
      return <ToolRun item={item} />;
    case "shell":
      return <ShellRow item={item} />;
    case "notice":
      return item.tone === "warn" ? (
        <div className="fs12 c-needs" role="status"><Icon name="warn" size="xs" /> {item.text}</div>
      ) : (
        <div className={`fs12 ${item.tone === "err" ? "c-failed" : "faint"}`} role={item.tone === "err" ? "alert" : undefined}>{item.text}</div>
      );
  }
});

/**
 * One transcript row, with its ⋯ menu. A row that cannot be drawn says so in its place and offers to try again; the rest of the
 * transcript is unaffected. It is drawn again by itself when its item changes. `part` and `scope` say which list and which
 * transcript the row belongs to (the raw events behind it are read from there).
 */
export const TranscriptItem = memo(function TranscriptItem({ item, driver, part = "live", scope = MAIN }: { item: RowItem; driver?: DriverDescriptor; part?: Part; scope?: TranscriptScope }) {
  return (
    <div className="row-wrap">
      <ErrorBoundary variant="item" name="transcript_entry" what="this entry" resetKey={item}>
        <TranscriptItemView item={item} driver={driver} />
      </ErrorBoundary>
      <MessageMenu row={item} part={part} scope={scope} />
    </div>
  );
});
