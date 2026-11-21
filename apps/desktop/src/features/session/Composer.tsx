import { useEffect, useMemo, useRef, useState } from "react";
import type { Attachment, SessionSnapshot } from "../../bridge/wire";
import { splitKey } from "../../bridge/wire";
import { useActions, useApi, usePending } from "../../state";
import { modelChoice } from "../../state/domain";
import { Button, Icon, IconButton, Kbd, Select } from "../../ui";
import { formatBytes } from "../media/fetch";
import { batchProblem, fromFile, fromPicked, handling, upload, type PendingFile } from "./attachments";

const inShell = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Pending attachments: thumbnails or chips, each with its tooltip and a remove button. */
function PendingList({ files, input, onRemove }: { files: PendingFile[]; input: SessionSnapshot["prompt_input"]; onRemove: (k: string) => void }) {
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
  const [text, setText] = useState("");
  const [files, setFiles] = useState<PendingFile[]>([]);
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
    const r = await actions.run(`pick:${sessionKey}`, "Could not open the file picker", () => api.desktop.pickFiles("Attach files"));
    if (r) add(r.files.map((p) => fromPicked(api, p)));
  };
  const attachHint = !canAttach
    ? "This agent takes no attachments"
    : input.files === "none"
      ? `Attach images (${input.image_mimes.map((m) => m.slice(6).toUpperCase()).join(", ")})`
      : "Attach images or files — or paste / drop them here";
  const [active, setActive] = useState(0);
  const ta = useRef<HTMLTextAreaElement>(null);
  const sending = usePending(`cmd:${sessionKey}:prompt`) === "pending";
  const slash = text.startsWith("/") && !text.includes(" ") && !text.includes("\n");
  const items = useMemo(() => (slash ? snap.slash_commands.filter((c) => c.name.toLowerCase().startsWith(text.slice(1).toLowerCase())) : []), [slash, text, snap.slash_commands]);
  const live = snap.phase.state !== "exited" && snap.phase.state !== "lost";

  const submit = async () => {
    const t = text.trim();
    if ((!t && !files.length) || sending || problem) return;
    if (files.length) {
      const { host, id } = splitKey(sessionKey);
      const sent = files;
      const ok = await actions.run(`cmd:${sessionKey}:prompt`, "Could not send your message", async () => {
        const attachments: Attachment[] = [];
        for (const f of sent) attachments.push(await upload(api, host, id, f));
        await api.command(host, id, { command: "prompt", text: t, attachments });
        return true;
      });
      if (ok) {
        sent.forEach((f) => f.thumb && URL.revokeObjectURL(f.thumb));
        setFiles([]);
        setText("");
      }
      return;
    }
    const m = /^\/(\S+)\s*(.*)$/s.exec(t);
    const cmd = m && snap.slash_commands.some((c) => c.name === m[1]) ? m : null;
    setText("");
    const ok = cmd ? await actions.command(sessionKey, { command: "run_slash", name: cmd[1], args: cmd[2] }, { failTitle: "Could not run the command" }) : await actions.sendPrompt(sessionKey, t);
    if (!ok) setText(t);
  };
  const pick = (name: string) => {
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
        {slash && snap.slash_commands.length ? (
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
          placeholder={live ? "Message the agent" : "This session has ended; sending continues it"}
          value={text}
          onChange={(e) => { setText(e.target.value); setActive(0); }}
          onKeyDown={(e) => {
            if (slash && items.length) {
              if (e.key === "ArrowDown") { e.preventDefault(); return setActive((a) => Math.min(items.length - 1, a + 1)); }
              if (e.key === "ArrowUp") { e.preventDefault(); return setActive((a) => Math.max(0, a - 1)); }
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && text.slice(1) !== items[active]?.name)) { e.preventDefault(); return pick(items[active].name); }
            }
            if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void submit(); }
            if (e.key === "Escape") (e.target as HTMLElement).blur();
          }}
        />
        <div className="comp-bar">
          <IconButton icon="clip" label={attachHint} onClick={() => void attach()} disabled={!canAttach} />
          <IconButton icon="slash" label="Slash commands" onClick={() => { setText("/"); ta.current?.focus(); }} disabled={!snap.capabilities.slash_commands} />
          {snap.available_modes.length ? (
            <Select compact up label="Mode" leading={<Icon name="shield" size="xs" />} value={snap.mode ?? undefined} options={snap.available_modes.map((m) => ({ value: m.id, label: m.label, description: m.description }))}
              onChange={(v) => void actions.command(sessionKey, { command: "set_mode", mode: v }, { failTitle: "Could not change the mode" })} />
          ) : null}
          {snap.available_models.length ? (
            <Select compact up label="Model" leading={<Icon name="sparkle" size="xs" />} value={modelChoice(snap).value} options={modelChoice(snap).options}
              onChange={(v) => void actions.command(sessionKey, { command: "set_model", model: v }, { failTitle: "Could not change the model" })} />
          ) : null}
          <span className="sp" />
          <span className="faint fs115"><Kbd>Enter</Kbd> send · <Kbd>⇧ Enter</Kbd> new line</span>
          <Button variant="primary" size="sm" icon="send" pending={sending} disabled={(!text.trim() && !files.length) || !!problem} onClick={() => void submit()}>Send</Button>
        </div>
      </div>
    </div>
  );
}
