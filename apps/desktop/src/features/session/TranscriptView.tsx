import { memo, useMemo, useState } from "react";
import { DriverBadge, Icon, StatusPill, type DriverDescriptor } from "../../ui";
import { Markdown } from "./Markdown";
import { AttachmentsView, PartsView } from "../media/Parts";
import { FileRefs, textPathCandidates, toolFileRefs } from "../media/refs";
import { resultText, toolArg, type Item } from "./transcript";

function ToolBody({ item }: { item: Extract<Item, { kind: "tool" }> }) {
  const i = item.input as { old_string?: string; new_string?: string } | undefined;
  if (typeof i?.old_string === "string" && typeof i.new_string === "string") {
    const old = i.old_string.split("\n");
    const nw = i.new_string.split("\n");
    return (
      <div className="diff" data-testid="tool-diff">
        {old.map((l, n) => <div key={`d${n}`} className="ln d"><span /><span /><span>-</span><span>{l}</span></div>)}
        {nw.map((l, n) => <div key={`a${n}`} className="ln a"><span /><span /><span>+</span><span>{l}</span></div>)}
      </div>
    );
  }
  const text = resultText(item.result);
  return <div className="tool-b">{text || (item.done ? (item.parts.length ? "" : "(no output)") : "Running…")}</div>;
}

function Tool({ item }: { item: Extract<Item, { kind: "tool" }> }) {
  const [open, setOpen] = useState(false);
  const icon = !item.done ? "working" : item.ok === false ? "failed" : "done";
  const tone = !item.done ? "c-working" : item.ok === false ? "c-failed" : "c-done";
  const first = resultText(item.result).split("\n")[0].slice(0, 60) || (item.parts.length ? `${item.parts.length} attachment${item.parts.length > 1 ? "s" : ""}` : "");
  const files = useMemo(() => (item.done && item.ok !== false ? toolFileRefs(item.name, item.input) : []), [item.done, item.ok, item.name, item.input]);
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
      <PartsView parts={item.parts} />
      {files.length ? <FileRefs paths={files} label="Files this tool wrote" /> : null}
    </div>
  );
}

function Subagent({ item }: { item: Extract<Item, { kind: "tool" }> }) {
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
  const paths = useMemo(() => textPathCandidates(text), [text]);
  return paths.length ? <FileRefs paths={paths} label="Files mentioned" /> : null;
}

export const TranscriptItem = memo(function TranscriptItem({ item, driver }: { item: Item; driver?: DriverDescriptor }) {
  switch (item.kind) {
    case "user":
      return (
        <div className="msg-user">
          {item.text}
          {item.queued ? <span className="faint"> · queued</span> : null}
          <AttachmentsView attachments={item.attachments} />
        </div>
      );
    case "assistant":
      return (
        <div className="msg-ai">
          <div className="who">{driver ? <DriverBadge driver={driver} compact /> : null} {driver?.displayName ?? "Agent"}</div>
          <Markdown text={item.text} />
          <MentionedFiles text={item.text} />
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
    case "notice":
      return <div className={`fs12 ${item.tone === "err" ? "c-failed" : "faint"}`} role={item.tone === "err" ? "alert" : undefined}>{item.text}</div>;
  }
});
