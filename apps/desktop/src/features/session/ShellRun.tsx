import { useSyncExternalStore } from "react";
import { errorMessage, type Api, type ExecEvent, type LocalShellRecord } from "../../bridge/api";
import type { Unsubscribe } from "../../bridge/backend";
import { splitKey, type SessionKey } from "../../bridge/wire";
import type { Actions } from "../../state/actions";
import { Button, Icon } from "../../ui";
import "./ShellRun.css";

/** The panel shows the end of the output (a long run stays readable); the record keeps all of it that the host kept. */
export const SHOWN_OUTPUT_CHARS = 64 * 1024;

/** Where a run is: started on the host, streaming, ended, or never started. */
export type ShellRunStatus = "starting" | "running" | "exited" | "failed";
/** Whether the run is in the conversation: not yet (running), being saved, saved, not saved because the session ended, or refused. */
export type ShellRecord = "none" | "saving" | "saved" | "not_saved_ended" | "failed";

export interface ShellRun {
  readonly id: number;
  readonly key: SessionKey;
  readonly cwd: string;
  readonly line: string;
  readonly status: ShellRunStatus;
  readonly execId: string | null;
  readonly output: string;
  /** The host kept only the first part of the output (its cap); the rest was produced and dropped. */
  readonly outputTruncated: boolean;
  readonly exit: { code: number | null; signal: number | null; reason: "time_limit" | "cancelled" | null } | null;
  readonly cancelling: boolean;
  /** Why it could not start or could not be cancelled. */
  readonly error: string | null;
  readonly record: ShellRecord;
  readonly recordError: string | null;
}

/** What a run needs from the composer that started it: whether the session is running now, and the actions that save it. */
export interface ShellRunContext {
  running: () => boolean;
  actions: Actions;
}

/** A refusal because the session has no process running (the daemon's `not_running`). */
const isNotRunning = (e: unknown) => (e as { code?: unknown } | null)?.code === "not_running";

/**
 * The host-side runs of one app. They live here, not in the composer, so a run keeps streaming and is saved when it
 * ends even if the user has moved to another tab meanwhile. One run per session at a time.
 */
export class ShellRuns {
  private runs = new Map<SessionKey, ShellRun>();
  private listeners = new Set<() => void>();
  private seq = 0;

  constructor(private api: Api) {}

  get = (key: SessionKey): ShellRun | undefined => this.runs.get(key);

  subscribe = (fn: () => void): Unsubscribe => {
    this.listeners.add(fn);
    return () => {
      this.listeners.delete(fn);
    };
  };

  /** Start `line` in `cwd` on the session's host. Nothing is sent when a run of this session is still going. */
  start(key: SessionKey, cwd: string, line: string, ctx: ShellRunContext): void {
    const prev = this.runs.get(key);
    if (prev && (prev.status === "starting" || prev.status === "running")) return;
    const id = ++this.seq;
    this.put(key, { id, key, cwd, line, status: "starting", execId: null, output: "", outputTruncated: false, exit: null, cancelling: false, error: null, record: "none", recordError: null });
    const { host } = splitKey(key);
    this.api.exec.start(host, { cwd, line }).then(
      ({ exec_id }) => {
        this.update(key, id, { status: "running", execId: exec_id });
        this.api.exec.output(
          host,
          exec_id,
          (e) => this.onEvent(key, id, ctx, e),
          // The stream ends for good only after `exited`; ending before it means the host lost the command.
          () => this.update(key, id, this.current(key, id)?.status === "running" ? { status: "failed", error: "The connection to the host ended before the command did." } : {}),
        );
      },
      (e) => this.update(key, id, { status: "failed", error: errorMessage(e) }),
    );
  }

  /** Ends the running command (`host.exec.cancel`); the stream then reports `exited` with reason `cancelled`. */
  cancel(key: SessionKey): void {
    const r = this.runs.get(key);
    if (!r || r.status !== "running" || !r.execId || r.cancelling) return;
    this.update(key, r.id, { cancelling: true });
    this.api.exec.cancel(splitKey(key).host, r.execId).catch((e) => this.update(key, r.id, { cancelling: false, error: `Could not cancel: ${errorMessage(e)}` }));
  }

  /** Removes a finished run from the panel. A run that is being saved stays until it is. */
  dismiss(key: SessionKey): void {
    const r = this.runs.get(key);
    if (!r || r.status === "starting" || r.status === "running" || r.record === "saving") return;
    this.runs.delete(key);
    this.emit();
  }

  private onEvent(key: SessionKey, id: number, ctx: ShellRunContext, e: ExecEvent): void {
    const r = this.current(key, id);
    if (!r) return;
    if (e.type === "output") return this.update(key, id, { output: r.output + e.text });
    const exit = { code: e.code, signal: e.signal, reason: e.reason ?? null };
    this.update(key, id, { status: "exited", exit, outputTruncated: e.output_truncated, cancelling: false });
    // A command the user ran is part of the conversation only while the session is running to hear about it.
    if (!ctx.running()) return this.update(key, id, { record: "not_saved_ended" });
    const done = this.current(key, id)!;
    this.update(key, id, { record: "saving" });
    const record: LocalShellRecord = { command: done.line, cwd: done.cwd, exit_code: e.code, output: done.output, output_truncated: e.output_truncated };
    void ctx.actions
      .recordLocalShell(key, record, {
        onError: (err) => {
          if (isNotRunning(err)) this.update(key, id, { record: "not_saved_ended" });
          else this.update(key, id, { record: "failed", recordError: errorMessage(err) });
          return true;
        },
      })
      .then((saved) => {
        if (saved) this.update(key, id, { record: "saved" });
        else if (this.current(key, id)?.record === "saving") this.update(key, id, { record: "failed", recordError: "The host did not take it." });
      });
  }

  private current(key: SessionKey, id: number): ShellRun | undefined {
    const r = this.runs.get(key);
    return r?.id === id ? r : undefined;
  }

  /** Changes the run only while it is still the session's current one (a newer run replaces it). */
  private update(key: SessionKey, id: number, patch: Partial<ShellRun>): void {
    const r = this.current(key, id);
    if (!r || Object.keys(patch).length === 0) return;
    this.put(key, { ...r, ...patch });
  }

  private put(key: SessionKey, r: ShellRun): void {
    this.runs.set(key, r);
    this.emit();
  }

  private emit(): void {
    for (const fn of [...this.listeners]) fn();
  }
}

/** The runs of an app instance: one registry per API, so tests and windows do not share them. */
const registries = new WeakMap<Api, ShellRuns>();
export function shellRunsFor(api: Api): ShellRuns {
  let runs = registries.get(api);
  if (!runs) {
    runs = new ShellRuns(api);
    registries.set(api, runs);
  }
  return runs;
}

/** The session's current run, re-rendered on every change. */
export function useShellRun(runs: ShellRuns, key: SessionKey): ShellRun | undefined {
  return useSyncExternalStore(runs.subscribe, () => runs.get(key), () => runs.get(key));
}

/** How the command ended, in words. */
export function exitWords(exit: NonNullable<ShellRun["exit"]>): string {
  if (exit.reason === "time_limit") return "Stopped at the 1-hour limit";
  if (exit.reason === "cancelled") return "Cancelled";
  if (exit.code != null) return `Exit code ${exit.code}`;
  if (exit.signal != null) return `Ended by signal ${exit.signal}`;
  return "Ended without an exit code";
}

function stateWords(r: ShellRun): string {
  switch (r.status) {
    case "starting":
      return "Starting…";
    case "running":
      return r.cancelling ? "Cancelling…" : "Running…";
    case "failed":
      return "Not run";
    case "exited":
      return exitWords(r.exit!);
  }
}

function recordWords(r: ShellRun): string | null {
  switch (r.record) {
    case "none":
      return null;
    case "saving":
      return "Saving to the conversation…";
    case "saved":
      return "Saved to the conversation";
    case "not_saved_ended":
      return "Not saved to the conversation (the session has ended)";
    case "failed":
      return `Not saved to the conversation. ${r.recordError ?? ""}`.trim();
  }
}

/**
 * A command run on the session's host: its state, streamed output (the end of it, monospace), Cancel while it runs,
 * and Dismiss once it has ended and been saved or not.
 */
export function ShellRunPanel({ run, onCancel, onDismiss }: { run: ShellRun; onCancel: () => void; onDismiss: () => void }) {
  const live = run.status === "starting" || run.status === "running";
  const shown = run.output.length > SHOWN_OUTPUT_CHARS ? run.output.slice(-SHOWN_OUTPUT_CHARS) : run.output;
  const record = recordWords(run);
  return (
    <section className="shell-run" aria-label="Shell command" data-testid="shell-run" data-status={run.status}>
      <header className="shell-run-h">
        <Icon name="term" size="xs" />
        <code className="shell-run-line" title={run.line}>{run.line}</code>
        <span className="shell-run-cwd faint" title={run.cwd}>{run.cwd}</span>
        <span className="shell-run-state" role="status" data-testid="shell-run-state">{stateWords(run)}</span>
        {live ? (
          <Button size="sm" icon="stop" onClick={onCancel} disabled={run.status !== "running" || run.cancelling}>Cancel</Button>
        ) : (
          <Button size="sm" onClick={onDismiss} disabled={run.record === "saving"}>Dismiss</Button>
        )}
      </header>
      {run.error ? <div className="comp-msg" role="alert" data-testid="shell-run-error">{run.error}</div> : null}
      <pre className="shell-run-out" aria-label="Command output" data-testid="shell-output">{shown}</pre>
      {run.output.length > SHOWN_OUTPUT_CHARS ? <div className="shell-run-note faint">Earlier output is not shown.</div> : null}
      {run.outputTruncated ? <div className="shell-run-note faint">The host kept only the first part of the output.</div> : null}
      {record ? <div className="shell-run-note" role="status" data-testid="shell-run-record">{record}</div> : null}
    </section>
  );
}
