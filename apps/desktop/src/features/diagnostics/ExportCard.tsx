import { useEffect, useState } from "react";
import type { ExportStatus } from "../../bridge/wire";
import { clockTime } from "../../lib/format";
import { currentExport, useActions, useApi, useApp, useAppState, useHosts, useReach } from "../../state";
import { Button, Card, Icon, Segmented } from "../../ui";
import { completeness, finishedSources, sourceName, sourceState, WINDOWS, type WindowId } from "./exportWords";
import { Problem, Reading } from "./parts";

/** What the export contains, and what it never does. */
const ABOUT =
  "One file with the connection steps, session states, timings and error codes recorded by this app, this computer and the hosts you are connected to. It never contains prompts, replies, tool output, files or secrets.";

/** `s` as a sentence: the reasons the app holds end without a full stop, the ones the hub gave may have one. */
const sentence = (s: string) => (/[.!?]$/.test(s.trim()) ? s.trim() : `${s.trim()}.`);

const when = (ms: number) => new Date(ms).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });

/** One row per source of the export, in plain words. */
function Sources({ x }: { x: ExportStatus }) {
  const hosts = useHosts();
  return (
    <ul className="dgx-src" aria-label="Sources of the export">
      {x.sources.map((s) => {
        const st = sourceState(s);
        return (
          <li key={s.source} className="dgx-row" data-testid="export-source" data-source={s.source} data-state={s.state}>
            <b>{sourceName(s, hosts)}</b>
            <span className={`dgx-st ${st.tone}`}>{s.state === "reading" ? <Icon name="working" size="xs" spin /> : null}{st.word}</span>
            <span className="faint small tnum dgx-sz">{st.bytes}</span>
            <span className="faint small dgx-why">{st.reason ?? ""}</span>
          </li>
        );
      })}
    </ul>
  );
}

/** A finished export's file: how big, whether it is complete, and where it goes from here. */
function Done({ x, live }: { x: ExportStatus; live: boolean }) {
  const { exports } = useApp();
  const api = useApi();
  const actions = useActions();
  const save = useAppState((s) => (x.file ? s.exports?.saves[x.file] : undefined));
  const c = completeness(x);
  return (
    <>
      <div data-testid="export-result" data-complete={String(c.complete)}>
        {c.size ? <b>{c.size}</b> : null}{c.size ? " · " : ""}<span className={c.complete === true ? "c-done" : "c-needs"}>{c.words}</span>
      </div>
      <div className="dgx-acts">
        <Button size="sm" variant="primary" icon="download" pending={save?.state === "saving"} blocked={!x.file ? "The hub did not name the file." : !live ? "Export progress is not live; try again first." : null} onClick={() => x.file && void exports.save(x.file)}>Save…</Button>
        {save?.state === "saved" ? (
          <>
            <span className="faint small" role="status" data-testid="export-saved">Saved to <span className="mono">{save.path}</span></span>
            <Button size="sm" icon="folder" onClick={() => void actions.run("export:reveal", "Could not show the file", () => api.desktop.reveal(save.path), { local: true })}>Show in folder</Button>
          </>
        ) : save?.state === "cancelled" ? <span className="faint small" role="status" data-testid="export-cancelled">Not saved: you closed the dialog.</span> : null}
      </div>
      {save?.state === "failed" ? <Problem title="Could not save the file." detail={save.reason} /> : null}
    </>
  );
}

/**
 * Settings → Diagnostics → Export diagnostics: asks the hub for one file with the logs of this app,
 * this computer and the connected hosts, follows its progress source by source, and saves the finished
 * file where the user chooses. Always available (Developer mode is not needed). It shows what the hub
 * reports on `diagnostics.exports` and nothing it has not confirmed, including an export that was
 * already running or finished when the page opened.
 */
export function ExportCard() {
  const { exports } = useApp();
  const reach = useReach();
  const down = !!reach;
  const slice = useAppState((s) => s.exports);
  const feedError = useAppState((s) => s.feedErrors.exports);
  const actions = useActions();
  const [win, setWin] = useState<WindowId>("24h");
  useEffect(() => (down ? undefined : exports.open()), [exports, down]);

  const x = slice ? currentExport(slice.list) : undefined;
  const running = slice?.list.some((y) => y.state === "running") ?? false;
  const starting = slice?.start.state === "starting" || slice?.start.state === "awaiting";
  const live = !down && slice?.state === "live" && !feedError;
  const start = () => void exports.start(Date.now() - WINDOWS.find((w) => w.id === win)!.ms);
  const blocked = reach ?? (feedError ? "Export progress is not live; try again first." : !live ? "Waiting for the local service to report on exports." : running ? "An export is already running." : null);

  return (
    <Card className="setcard dgx" data-testid="export-diagnostics" aria-label="Export diagnostics">
      <div className="eyebrow">Export diagnostics</div>
      <p className="dgx-note faint small">{ABOUT}</p>
      {down ? (
        <div className="dg-problem warn" role="status" data-testid="export-unreachable">
          <Icon name="clock" size="sm" />
          <div className="dg-pt">
            <b>An export cannot be made now.</b>
            <span className="small"> {sentence(reach)} The app&apos;s own log file is still in the data folder (<span className="mono">diag/app.ndjson</span>), and can be read there.</span>
          </div>
        </div>
      ) : (
        <>
          <fieldset className="dgx-pick">
            <Segmented<WindowId> label="Time window" value={win} onChange={setWin} options={WINDOWS.map((w) => ({ value: w.id, label: w.label }))} />
            <span className="sp" />
            <Button variant="primary" icon="download" pending={starting} blocked={blocked} onClick={start}>Export</Button>
          </fieldset>
          <div className="dgx-body">
            {feedError ? <Problem title="Export progress stopped." detail={feedError} onRetry={actions.retryLive} /> : null}
            {!feedError && slice?.state !== "live" ? <Reading what="Asking the local service about exports…" /> : null}
            {slice?.start.state === "refused" ? <div className="c-failed" role="alert" data-testid="export-refused"><Icon name="warn" size="sm" /> Could not start the export. {slice.start.reason}</div> : null}
            {slice?.start.state === "awaiting" ? <Reading what="Export accepted; waiting for its first report…" /> : null}
            {x ? (
              <div className="dgx-cur" data-testid="export-status" data-state={x.state}>
                <div className="dgx-head">
                  {x.state === "running" ? <><Icon name="working" size="sm" spin /><b>Exporting…</b><span className="faint small">{finishedSources(x)} of {x.sources.length} sources finished</span></>
                    : x.state === "done" ? <><span className="c-done"><Icon name="done" size="sm" /></span><b>Export finished</b></>
                    : <><span className="c-failed"><Icon name="failed" size="sm" /></span><b>Export failed</b></>}
                  <span className="sp" />
                  <span className="faint small">Started {clockTime(x.started_ms)} · covers the time since {when(x.since_ms)}</span>
                </div>
                {x.sources.length ? <Sources x={x} /> : null}
                {x.state === "running" ? <div className="faint small">Another export cannot start until this one ends.</div> : null}
                {x.state === "done" ? <Done x={x} live={live} /> : null}
                {x.state === "failed" ? (
                  <div className="dgx-acts">
                    <span className="c-failed" role="alert" data-testid="export-failure">{x.error || "The hub gave no reason."}</span>
                    <Button size="sm" icon="refresh" blocked={blocked} pending={starting} onClick={start}>Try again</Button>
                  </div>
                ) : null}
              </div>
            ) : slice?.state === "live" ? <div className="faint small" data-testid="export-none">No export has been made yet.</div> : null}
          </div>
        </>
      )}
    </Card>
  );
}
