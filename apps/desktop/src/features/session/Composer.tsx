import { useEffect, useMemo, useRef, useState } from "react";
import type { SessionSnapshot } from "../../bridge/wire";
import { splitKey } from "../../bridge/wire";
import { errorMessage } from "../../bridge/api";
import { continueState, notContinuableWords, sessionModesState, sessionModelsState, useActions, useApi, useHost, usePending, useSessionReach, useSessionView } from "../../state";
import { modelChoice } from "../../state/domain";
import { Button, Icon, IconButton, Kbd, Select, useEscape } from "../../ui";
import { formatBytes } from "../media/fetch";
import { batchProblem, fromFile, fromPicked, handling, promptFiles, type PendingFile } from "./attachments";
import { clearDraft, peekDraft } from "./draft";
import { usePromptRecall } from "./usePromptRecall";
import { inShell } from "../../lib/shell";
import { ShellRunPanel, shellRunsFor, useShellRun } from "./ShellRun";

/** Why a reported slash command cannot be sent with attachments: a command takes text only, so the files or the "/" must go. */
const COMMAND_WITH_FILES = "Commands can't take attachments. Remove the files, or remove the /.";

/** Why a `!` shell command cannot be sent with attachments: a command takes text only, so the files or the "!" must go. */
const SHELL_WITH_FILES = "Shell commands can't take attachments. Remove the files, or remove the !.";
/** What the composer says when files are pasted or dropped into a `!` command: they are not taken. */
const SHELL_NO_FILES = "Shell commands can't take attachments.";

/** The command in a `!` line: what follows the leading "!", trimmed. */
const shellLineOf = (text: string) => text.slice(1).trim();

/** Whether `text` starts with a slash command the agent reported. An unreported `/foo` is ordinary prompt text, sent as it is. */
function isReportedCommand(text: string, commands: readonly { name: string }[]): boolean {
  const m = /^\/(\S+)/.exec(text);
  return !!m && commands.some((c) => c.name === m[1]);
}

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
  // A line starting with "!" is a shell command, not a prompt: the composer says so and runs it on the session's side.
  const shell = text.startsWith("!");
  const shellLine = shellLineOf(text);
  const [refused, setRefused] = useState<string | null>(null);
  const [files, setFiles] = useState<PendingFile[]>(draft.current?.files ?? []);
  const [drag, setDrag] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const input = snap.prompt_input;
  const canAttach = input.max_attachments > 0;
  // A reported command takes no files: that is said while both are here, and Send stays off until one of them goes.
  const problem = useMemo(() => {
    if (!files.length) return null;
    if (shell) return SHELL_WITH_FILES;
    return batchProblem(files, input) ?? (isReportedCommand(text.trim(), snap.slash_commands) ? COMMAND_WITH_FILES : null);
  }, [files, input, text, snap.slash_commands, shell]);
  const filesRef = useRef(files);
  filesRef.current = files;
  // Thumbnails are object URLs: release them with the composer.
  useEffect(() => () => filesRef.current.forEach((f) => f.thumb && URL.revokeObjectURL(f.thumb)), []);
  const add = (list: PendingFile[]) => list.length && setFiles((fs) => [...fs, ...list]);
  // Files are not taken while the line is a shell command: the composer says so where the user is typing.
  const refuseFiles = () => setRefused(SHELL_NO_FILES);
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
  const attachHint = shell
    ? "Shell commands take no attachments"
    : !canAttach
      ? "This agent takes no attachments"
      : input.files === "none"
        ? `Attach images (${input.image_mimes.map((m) => m.slice(6).toUpperCase()).join(", ")})`
        : "Attach images or files — or paste / drop them here";
  const [active, setActive] = useState(0);
  const ta = useRef<HTMLTextAreaElement>(null);
  const promptPending = usePending(`cmd:${sessionKey}:prompt`);
  const slashPending = usePending(`cmd:${sessionKey}:run_slash`);
  const shellPending = usePending(`cmd:${sessionKey}:run_shell`);
  const sending = promptPending === "pending" || slashPending === "pending" || shellPending === "pending";
  const recall = usePromptRecall(sessionKey, text, setText, ta);
  // An ended session takes a prompt when the daemon says it can be continued (the daemon starts it again and then delivers
  // the prompt); when it cannot, the box says why and takes nothing.
  const cont = continueState(snap);
  const noSend = cont.kind === "blocked" ? notContinuableWords(cont.why) : null;
  // Mode and model are set on a running agent.
  const setupBlock = cont.kind === "running" ? null : "The session has ended; set this once it is continued.";
  // A shell command: in the agent's own shell when the agent can run one (only while it is running), else on the
  // session's host in its folder, one run at a time and streamed into the panel above the box.
  const runningNow = cont.kind === "running";
  // Read when a run ends, which can be after this render: the latest value is kept here.
  const runningRef = useRef(runningNow);
  runningRef.current = runningNow;
  const agentShell = snap.capabilities.run_shell;
  const runs = shellRunsFor(api);
  const shellRun = useShellRun(runs, sessionKey);
  const hostName = useHost(splitKey(sessionKey).host)?.name ?? splitKey(sessionKey).host;
  const shellBusy = shellRun?.status === "starting" || shellRun?.status === "running";
  const shellBlock: string | null = !shell || !shellLine || files.length
    ? null
    : agentShell
      ? runningNow ? null : "The agent is not running, so its shell is not available. Continue the session first."
      : shellBusy ? "A command is still running here. Cancel it, or wait for it to finish."
      : !snap.cwd ? "This session has no folder to run a command in."
      : null;
  const shellHint = agentShell ? "Runs in the agent's shell (full access)" : `Runs on ${hostName} in ${snap.cwd ?? "the session folder"}`;
  const [failure, setFailure] = useState<string | null>(null);
  // The slash menu belongs to typing a command; a recalled prompt is not being typed. Esc closes it, and nothing else.
  const [menuClosed, setMenuClosed] = useState(false);
  const slash = text.startsWith("/") && !text.includes(" ") && !text.includes("\n") && !recall.browsing && !menuClosed;
  const menuOpen = slash && snap.slash_commands.length > 0;
  useEscape(menuOpen, () => setMenuClosed(true));
  const items = useMemo(() => (slash ? snap.slash_commands.filter((c) => c.name.toLowerCase().startsWith(text.slice(1).toLowerCase())) : []), [slash, text, snap.slash_commands]);

  // A shell command: the agent's own shell takes it as a command (its output comes back as the agent's tool row); otherwise
  // the host runs it and the panel shows it, and the run is recorded to the conversation when it ends.
  const submitShell = async () => {
    if (!shellLine || shellBlock || blocked || sending) return;
    setFailure(null);
    if (agentShell) {
      const ok = await actions.command(sessionKey, { command: "run_shell", line: shellLine }, { failTitle: "Could not run the command", onError: (e) => (setFailure(errorMessage(e)), true) });
      if (!ok) return;
    } else {
      runs.start(sessionKey, snap.cwd ?? "", shellLine, { running: () => runningRef.current, actions });
    }
    recall.reset();
    setText("");
  };

  // What is typed stays until the daemon has taken it: nothing is shown as sent that was not.
  const submit = async () => {
    if (shell) return submitShell();
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
      {shellRun ? <ShellRunPanel run={shellRun} onCancel={() => runs.cancel(sessionKey)} onDismiss={() => runs.dismiss(sessionKey)} /> : null}
      <div
        className={`composer${drag ? " drop" : ""}`}
        onDragOver={(e) => {
          if (!canAttach || shell || !e.dataTransfer?.types.includes("Files")) return;
          e.preventDefault();
          setDrag(true);
        }}
        onDragLeave={() => setDrag(false)}
        onDrop={(e) => {
          setDrag(false);
          if (!canAttach || !e.dataTransfer?.files.length) return;
          e.preventDefault();
          if (shell) return refuseFiles();
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
        {refused ? <div className="comp-msg" role="alert" data-testid="shell-refused">{refused}</div> : null}
        {shellBlock ? <div className="comp-msg" role="status" data-testid="shell-blocked">{shellBlock}</div> : null}
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
            if (shell) return refuseFiles();
            add(pasted.map(fromFile));
          }}
          aria-label="Message"
          placeholder={cont.kind === "running" ? "Message the agent" : noSend ?? "This session has ended; sending a message continues it"}
          value={text}
          disabled={!!noSend}
          readOnly={sending}
          onChange={(e) => { recall.reset(); setText(e.target.value); setActive(0); setMenuClosed(false); setFailure(null); setRefused(null); }}
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
            {shell ? <span className="comp-chip" data-testid="shell-mode"><Icon name="term" size="xs" />Shell</span> : null}
            <IconButton icon="clip" label={attachHint} onClick={() => void attach()} disabled={!canAttach || shell || !!blocked || !!noSend} />
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
            {shell ? (
              <span className="faint fs115 comp-hint" data-testid="shell-hint">{shellHint}</span>
            ) : recall.paging ? (
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
            <Button variant="primary" size="sm" icon="send" pending={sending} blocked={blocked} disabled={(!text.trim() && !files.length) || !!problem || !!noSend || (shell && (!shellLine || !!shellBlock))} onClick={() => void submit()}>Send</Button>
          </div>
        </div>
      </div>
    </div>
  );
}
