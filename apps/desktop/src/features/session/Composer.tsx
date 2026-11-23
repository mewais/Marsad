import { useEffect, useMemo, useRef, useState } from "react";
import type { SessionSnapshot } from "../../bridge/wire";
import { splitKey } from "../../bridge/wire";
import { errorMessage } from "../../bridge/api";
import { continueState, notContinuableWords, sessionModesState, sessionModelsState, useActions, useApi, usePending, useSessionReach, useSessionView } from "../../state";
import { modelChoice } from "../../state/domain";
import { Button, Icon, IconButton, Kbd, Select, useEscape } from "../../ui";
import { formatBytes } from "../media/fetch";
import { batchProblem, fromFile, fromPicked, handling, promptFiles, type PendingFile } from "./attachments";
import { clearDraft, peekDraft } from "./draft";
import { usePromptRecall } from "./usePromptRecall";
import { inShell } from "../../lib/shell";

/** Pending attachments: thumbnails or chips, each with its tooltip and a remove button. */
export function PendingList({ files, input, onRemove }: { files: PendingFile[]; input: SessionSnapshot["prompt_input"]; onRemove: (k: string) => void }) {
  if (!files.length) return null;
  return (
    <div className="comp-atts" aria-label="Attachments">
      {files.map((f) => {
        const h = handling(f, input);
        return (
          <div key={f.key} className="comp-att" data-testid="pending-attachment" data-error={!h.ok} title={`${f.name} · ${formatBytes(f.size)}\n${h.note}`}>
            {f.thumb && h.ok ? <img src={f.thumb} alt="" /> : <Icon name={h.ok ? "file" : "warn"} size="sm" />}
            <span className="n">{f.name}</span>
            <span className="faint">{formatBytes(f.size)}</span>
            <IconButton className="x" icon="x" label={`Remove ${f.name}`} onClick={() => onRemove(f.key)} />
          </div>
        );
      })}
    </div>
  );
}

/** Message box: slash menu, mode and model pickers all come from the session snapshot. */
export function Composer({ sessionKey, snap }: { sessionKey: string; snap: SessionSnapshot }) {
  const actions = useActions();
  const api = useApi();
  const view = useSessionView(sessionKey);
  const blocked = useSessionReach(view);
  const stale = !!view?.stale;
  const modesState = sessionModesState(snap, stale);
  const modelsState = sessionModelsState(snap, stale);
  // A launch that created this session but could not send the first message leaves it as a draft.
  const draft = useRef(peekDraft(sessionKey));
  useEffect(() => clearDraft(sessionKey), [sessionKey]);
  const [text, setText] = useState(draft.current?.text ?? "");
  const [files, setFiles] = useState<PendingFile[]>(draft.current?.files ?? []);
  const [drag, setDrag] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const input = snap.prompt_input;
  const canAttach = input.max_attachments > 0;
  const problem = useMemo(() => (files.length ? batchProblem(files, input) : null), [files, input]);
  const filesRef = useRef(files);
  filesRef.current = files;
  // Thumbnails are object URLs: release them with the composer.
  useEffect(() => () => filesRef.current.forEach((f) => f.thumb && URL.revokeObjectURL(f.thumb)), []);
  const add = (list: PendingFile[]) => list.length && setFiles((fs) => [...fs, ...list]);
  const remove = (k: string) =>
    setFiles((fs) => {
      fs.find((f) => f.key === k)?.thumb && URL.revokeObjectURL(fs.find((f) => f.key === k)!.thumb!);
      return fs.filter((f) => f.key !== k);
    });
  const attach = async () => {
    if (!inShell()) return fileInput.current?.click();
    const r = await actions.run(`pick:${sessionKey}`, "Could not open the file picker", () => api.desktop.pickFiles("Attach files"), { local: true });
    if (r) add(r.files.map((p) => fromPicked(api, p)));
  };
  const attachHint = !canAttach
    ? "This agent takes no attachments"
    : input.files === "none"
      ? `Attach images (${input.image_mimes.map((m) => m.slice(6).toUpperCase()).join(", ")})`
      : "Attach images or files — or paste / drop them here";
  const [active, setActive] = useState(0);
  const ta = useRef<HTMLTextAreaElement>(null);
  const promptPending = usePending(`cmd:${sessionKey}:prompt`);
  const slashPending = usePending(`cmd:${sessionKey}:run_slash`);
  const sending = promptPending === "pending" || slashPending === "pending";
  const recall = usePromptRecall(sessionKey, text, setText, ta);
  // An ended session takes a prompt when the daemon says it can be continued (the daemon starts it again and then delivers
  // the prompt); when it cannot, the box says why and takes nothing.
  const cont = continueState(snap);
  const noSend = cont.kind === "blocked" ? notContinuableWords(cont.why) : null;
  // Mode and model are set on a running agent.
  const setupBlock = cont.kind === "running" ? null : "The session has ended; set this once it is continued.";
  const [failure, setFailure] = useState<string | null>(null);
  // The slash menu belongs to typing a command; a recalled prompt is not being typed. Esc closes it, and nothing else.
  const [menuClosed, setMenuClosed] = useState(false);
  const slash = text.startsWith("/") && !text.includes(" ") && !text.includes("\n") && !recall.browsing && !menuClosed;
  const menuOpen = slash && snap.slash_commands.length > 0;
  useEscape(menuOpen, () => setMenuClosed(true));
  const items = useMemo(() => (slash ? snap.slash_commands.filter((c) => c.name.toLowerCase().startsWith(text.slice(1).toLowerCase())) : []), [slash, text, snap.slash_commands]);

  // What is typed stays until the daemon has taken it: nothing is shown as sent that was not.
  const submit = async () => {
    const t = text.trim();
    if ((!t && !files.length) || sending || problem || blocked || noSend) return;
    setFailure(null);
    const onError = (e: unknown) => (setFailure(errorMessage(e)), true);
    let ok: boolean;
    if (files.length) {
      const { host, id } = splitKey(sessionKey);
      const sent = files;
      ok = await actions.sendPrompt(sessionKey, t, promptFiles(api, host, id, sent), { onError });
      if (ok) sent.forEach((f) => f.thumb && URL.revokeObjectURL(f.thumb));
    } else {
      const m = /^\/(\S+)\s*(.*)$/s.exec(t);
      const cmd = m && snap.slash_commands.some((c) => c.name === m[1]) ? m : null;
      ok = cmd ? await actions.command(sessionKey, { command: "run_slash", name: cmd[1], args: cmd[2] }, { failTitle: "Could not run the command", onError }) : await actions.sendPrompt(sessionKey, t, undefined, { onError });
    }
    if (!ok) return;
    if (files.length) setFiles([]);
    recall.reset();
    setText("");
  };
  const pick = (name: string) => {
    recall.reset();
    setMenuClosed(false);
    setText(`/${name} `);
    ta.current?.focus();
  };

  return (
    <div className="composer-wrap">
      <div
        className={`composer${drag ? " drop" : ""}`}
        onDragOver={(e) => {
          if (!canAttach || !e.dataTransfer?.types.includes("Files")) return;
          e.preventDefault();
          setDrag(true);
        }}
        onDragLeave={() => setDrag(false)}
        onDrop={(e) => {
          setDrag(false);
          if (!canAttach || !e.dataTransfer?.files.length) return;
          e.preventDefault();
          add(Array.from(e.dataTransfer.files).map(fromFile));
        }}
      >
        {menuOpen ? (
          <div className="slash" role="listbox" aria-label="Slash commands">
            {items.length === 0 ? <div className="faint pad8">No command starts with “{text}”</div> : null}
            {items.map((c, i) => (
              <div key={c.name} role="option" aria-selected={i === active} className={`it ${i === active ? "on" : ""}`} onMouseDown={(e) => { e.preventDefault(); pick(c.name); }}>
                <b>/{c.name}</b>
                <span>{c.description}</span>
                <span className="src">{c.argument_hint}</span>
              </div>
            ))}
          </div>
        ) : null}
        <PendingList files={files} input={input} onRemove={remove} />
        {problem ? <div className="comp-msg" role="alert">{problem}</div> : null}
        {blocked ? <div className="comp-msg" role="status" data-testid="composer-blocked">Cannot send now. {blocked}</div> : null}
        {noSend ? <div className="comp-msg" role="status" data-testid="composer-ended">Cannot send: {noSend}</div> : null}
        {failure ? <div className="comp-msg" role="alert" data-testid="composer-failure">Not sent. {failure} Your message is still here.</div> : null}
        <input ref={fileInput} type="file" multiple hidden data-testid="attach-input" onChange={(e) => { add(Array.from(e.target.files ?? []).map(fromFile)); e.target.value = ""; }} />
        <textarea
          ref={ta}
          onPaste={(e) => {
            const pasted = Array.from(e.clipboardData?.files ?? []);
            if (!pasted.length || !canAttach) return;
            e.preventDefault();
            add(pasted.map(fromFile));
          }}
          aria-label="Message"
          placeholder={cont.kind === "running" ? "Message the agent" : noSend ?? "This session has ended; sending a message continues it"}
          value={text}
          disabled={!!noSend}
          readOnly={sending}
          onChange={(e) => { recall.reset(); setText(e.target.value); setActive(0); setMenuClosed(false); setFailure(null); }}
          onKeyDown={(e) => {
            if (menuOpen && items.length) {
              if (e.key === "ArrowDown") { e.preventDefault(); return setActive((a) => Math.min(items.length - 1, a + 1)); }
              if (e.key === "ArrowUp") { e.preventDefault(); return setActive((a) => Math.max(0, a - 1)); }
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && text.slice(1) !== items[active]?.name)) { e.preventDefault(); return pick(items[active].name); }
            }
            if (recall.onKeyDown(e)) return;
            if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void submit(); }
          }}
        />
        <div className="comp-bar">
          <div className="comp-tools">
            <IconButton icon="clip" label={attachHint} onClick={() => void attach()} disabled={!canAttach || !!blocked || !!noSend} />
            <IconButton icon="slash" label="Slash commands" onClick={() => { recall.reset(); setMenuClosed(false); setText("/"); ta.current?.focus(); }} disabled={!snap.capabilities.slash_commands || !!noSend} />
            {modesState === "ready" ? (
              <Select compact up label="Mode" blocked={blocked ?? setupBlock} leading={<Icon name="shield" size="xs" />} value={snap.mode ?? undefined} options={snap.available_modes.map((m) => ({ value: m.id, label: m.label, description: m.description }))}
                onChange={(v) => void actions.command(sessionKey, { command: "set_mode", mode: v }, { failTitle: "Could not change the mode" })} />
            ) : modesState === "loading" ? (
              <span className="comp-loading" role="status" data-testid="composer-modes-loading"><Icon name="working" size="xs" spin />Mode: loading…</span>
            ) : null}
            {modelsState === "ready" ? (
              <Select compact up label="Model" blocked={blocked ?? setupBlock} leading={<Icon name="sparkle" size="xs" />} value={modelChoice(snap).value} options={modelChoice(snap).options}
                onChange={(v) => void actions.command(sessionKey, { command: "set_model", model: v }, { failTitle: "Could not change the model" })} />
            ) : modelsState === "loading" ? (
              <span className="comp-loading" role="status" data-testid="composer-models-loading"><Icon name="working" size="xs" spin />Model: loading…</span>
            ) : null}
          </div>
          <div className="comp-tail">
            {recall.paging ? (
              <span className="faint fs115 comp-hint" role="status">Loading earlier prompts…</span>
            ) : (
              <span className="faint fs115 comp-hint">
                <span className="comp-hint-p"><Kbd>Enter</Kbd> send</span>
                <span aria-hidden="true">·</span>
                <span className="comp-hint-p"><Kbd>⇧ Enter</Kbd> new line</span>
                <span aria-hidden="true">·</span>
                <span className="comp-hint-p"><Kbd>↑</Kbd> earlier prompts</span>
              </span>
            )}
            <Button variant="primary" size="sm" icon="send" pending={sending} blocked={blocked} disabled={(!text.trim() && !files.length) || !!problem || !!noSend} onClick={() => void submit()}>Send</Button>
          </div>
        </div>
      </div>
    </div>
  );
}
