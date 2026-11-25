import { useEffect, useRef, useState } from "react";
import { errorMessage, type Api } from "../../bridge/api";
import type { ExportFile, HostId, SessionId } from "../../bridge/wire";
import { formatBytes } from "../media/fetch";
import { useApi } from "../../state";
import { Button, Checkbox, Dialog, Icon, Segmented } from "../../ui";
import { toast } from "../../ui/toast";
import { applyEvent, endedRun, exportIO, exportRequest, maskedWords, saveExportFiles, startedRun, DEFAULT_EXPORT_OPTIONS, TOOL_OUTPUT_CAP, type ExportOptions, type ExportRun, type FormatChoice } from "./conversationExport";
import "./ExportDialog.css";

const FORMAT_OPTIONS: { value: FormatChoice; label: string; title: string }[] = [
  { value: "markdown", label: "Markdown", title: "A readable transcript" },
  { value: "json", label: "JSON", title: "The session's events, as recorded" },
  { value: "both", label: "Both", title: "One file of each" },
];

/** The header's entry: opens the export dialog for the session. The dialog exists only while it is open. */
export function ExportConversation({ host, session, name, blocked }: { host: HostId; session: SessionId; name: string; blocked: string | null }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size="sm" variant="ghost" icon="download" blocked={blocked} title="Save this conversation to a file" onClick={() => setOpen(true)}>
        Export conversation…
      </Button>
      {open ? <ExportDialog host={host} session={session} name={name} onClose={() => setOpen(false)} /> : null}
    </>
  );
}

/**
 * Exports a session's conversation on its host and saves the files the user picks. Options first; then the
 * export's progress (Cancel ends it); then the result, with what was masked and the Save. Closing the dialog
 * while the export runs cancels it, so nothing is left running on the host. The state is the dialog's own:
 * it is mounted for one opening.
 */
export function ExportDialog({ host, session, name, onClose }: { host: HostId; session: SessionId; name: string; onClose: () => void }) {
  const api: Api = useApi();
  const [opts, setOpts] = useState<ExportOptions>(DEFAULT_EXPORT_OPTIONS);
  const [run, setRun] = useState<ExportRun>({ s: "idle" });
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState<string[] | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  // The export on the topic: its id, whether it has ended, and how to stop listening.
  const live = useRef<{ id: string; ended: boolean; off: () => void } | null>(null);

  useEffect(
    () => () => {
      const l = live.current;
      if (l && !l.ended) void api.sessionExport.cancel(host, l.id).catch(() => undefined);
      l?.off();
    },
    [api, host],
  );

  const start = async () => {
    setRun({ s: "starting" });
    setSaved(null);
    setSaveError(null);
    try {
      const { export_id } = await api.sessionExport.start(host, exportRequest(session, opts));
      setRun(startedRun(export_id));
      const l: { id: string; ended: boolean; off: () => void } = { id: export_id, ended: false, off: () => undefined };
      live.current = l;
      // The topic replays from its first event, so nothing said before this subscription is missed.
      l.off = api.sessionExport.events(
        host,
        export_id,
        (e) => {
          if (e.event !== "progress") l.ended = true;
          setRun((r) => applyEvent(r, e));
        },
        (end) => {
          l.ended = true;
          setRun((r) => endedRun(r, end));
        },
      );
    } catch (e) {
      setRun({ s: "failed", reason: errorMessage(e) });
    }
  };

  const cancel = async () => {
    const l = live.current;
    if (!l || l.ended) return;
    setRun((r) => (r.s === "running" ? { ...r, cancelling: true } : r));
    try {
      await api.sessionExport.cancel(host, l.id);
    } catch (e) {
      setRun((r) => (r.s === "running" ? { ...r, cancelling: false } : r));
      toast({ tone: "err", title: "Could not cancel the export", detail: errorMessage(e) });
    }
  };

  const save = async (files: readonly ExportFile[], id: string) => {
    setSaving(true);
    setSaveError(null);
    try {
      setSaved(await saveExportFiles(exportIO(api, host), id, files));
    } catch (e) {
      setSaveError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const setFormat = (format: FormatChoice) => setOpts((o) => ({ ...o, format }));
  const busy = run.s === "starting" || run.s === "running";
  const footer =
    run.s === "running" ? (
      <>
        <span className="sp" />
        <Button disabled={run.cancelling} onClick={() => void cancel()}>{run.cancelling ? "Cancelling…" : "Cancel export"}</Button>
      </>
    ) : run.s === "done" ? (
      <>
        <span className="sp" />
        <Button onClick={onClose}>Close</Button>
        <Button variant="primary" icon="download" pending={saving} onClick={() => void save(run.files, run.id)}>Save…</Button>
      </>
    ) : (
      <>
        <span className="sp" />
        <Button onClick={onClose}>Close</Button>
        <Button variant="primary" icon="download" pending={run.s === "starting"} onClick={() => void start()}>Export</Button>
      </>
    );

  return (
    <Dialog open onClose={onClose} title="Export conversation" subtitle={name} width={540} footer={footer}>
      <div className="xp">
        {run.s === "idle" || run.s === "starting" ? (
          <>
            <div className="xp-row">
              <span className="xp-label">Format</span>
              <Segmented label="Format" value={opts.format} onChange={setFormat} options={FORMAT_OPTIONS} />
            </div>
            <Checkbox label="Include subagents' conversations" hint="Off: the session's own conversation only." checked={opts.includeSubagents} disabled={busy}
              onChange={(includeSubagents) => setOpts((o) => ({ ...o, includeSubagents }))} />
            <Checkbox label="Include earlier history" hint="The conversation before this session, when it was continued or forked." checked={opts.includeEarlier} disabled={busy}
              onChange={(includeEarlier) => setOpts((o) => ({ ...o, includeEarlier }))} />
            <p className="faint small xp-cap">Tool outputs longer than {TOOL_OUTPUT_CAP / 1024} KB are cut to {TOOL_OUTPUT_CAP / 1024} KB.</p>
          </>
        ) : null}
        {run.s === "failed" ? (
          <div className="banner bad xp-note" role="alert" data-testid="export-failed">
            <Icon name="failed" size="sm" />
            <div className="bt"><b>The export did not finish.</b> <span>{run.reason}</span></div>
          </div>
        ) : null}
        {run.s === "running" ? (
          <div className="xp-progress" role="status" data-testid="export-progress">
            <Icon name="working" size="sm" spin className="c-working" />
            <span>{run.cancelling ? "Cancelling the export…" : "Writing the conversation…"} {run.events} events · {formatBytes(run.bytes)}</span>
          </div>
        ) : null}
        {run.s === "done" ? (
          <>
            <ul className="xp-files" aria-label="Files of the export">
              {run.files.map((f) => (
                <li key={f.path}><b>{f.format === "json" ? "JSON" : "Markdown"}</b> <span className="mono">{f.path.split(/[\\/]/).pop()}</span> <span className="faint">{formatBytes(f.bytes)}</span></li>
              ))}
            </ul>
            <p className="xp-masked" data-testid="export-masked"><Icon name="shield" size="sm" /> {maskedWords(run.masked)}</p>
            <div className="banner warn xp-note" role="note">
              <div className="bt">The file contains conversation content: your prompts, the agent's replies and tool output. Check it before you share it.</div>
            </div>
            {saved ? (
              <p className="small" role="status" data-testid="export-saved">
                {saved.length ? <>Saved to {saved.join(", ")}.</> : "Not saved: the Save dialog was cancelled."}
              </p>
            ) : null}
            {saveError ? <div className="banner bad xp-note" role="alert" data-testid="export-save-failed"><div className="bt"><b>Could not save the file.</b> <span>{saveError}</span></div></div> : null}
          </>
        ) : null}
      </div>
    </Dialog>
  );
}
