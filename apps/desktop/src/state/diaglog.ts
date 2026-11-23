import type { Backend } from "../bridge/backend";
import { errorMessage, newTraceId } from "../bridge/api";
import type { Command, LaunchRequest, UiEvent } from "../bridge/wire";
import { APP_EVENTS_KEEP, type AppStore } from "./model";

/**
 * The app's record of its own acts (`ui.*`, design §12.7). Only ids, codes, sizes, timings and
 * flags: what a user typed is counted, never kept, and nothing here is built from a prompt, a
 * reply, a name or a path.
 */
export interface DiagLog {
  /**
   * Record `event`: it goes into the store (the Diagnostics panels show the last few hundred) and,
   * with the others of the same moment, to the desktop shell's log. Never waits, never throws. If
   * the shell cannot take it, `diagLog` in the store counts it with the reason.
   */
  log(event: UiEvent): void;
}

export function createDiagLog(store: AppStore, backend: Pick<Backend, "diagLog">, now: () => number = Date.now): DiagLog {
  let batch: UiEvent[] = [];
  let scheduled = false;
  const flush = async () => {
    scheduled = false;
    const events = batch;
    batch = [];
    try {
      await backend.diagLog(events);
    } catch (e) {
      store.set((s) => ({ ...s, diagLog: { failed: s.diagLog.failed + events.length, error: errorMessage(e) } }));
    }
  };
  return {
    log(event) {
      store.set((s) => ({ ...s, appEvents: [...s.appEvents.slice(1 - APP_EVENTS_KEEP), { t: now(), event }] }));
      batch.push(event);
      // Everything logged in the same turn of the event loop goes to the shell in one call, after the action that logged it.
      if (!scheduled) {
        scheduled = true;
        queueMicrotask(() => void flush());
      }
    },
  };
}

/** An error's `code` when it is an RPC error's (the shell's and the daemons' errors have one). */
export const codeOf = (e: unknown): string | undefined => {
  const c = (e as { code?: unknown } | null | undefined)?.code;
  return typeof c === "string" && /^[A-Za-z0-9_.:-]{1,64}$/.test(c) ? c : undefined;
};

/**
 * `backend` with the outcome of every traced request recorded as `ui.rpc` (leg 3 of a prompt's
 * trail: it left the app, and how it was answered). One place, so no caller records its own.
 */
export function withRpcOutcomes(backend: Backend, log: (e: UiEvent) => void): Backend {
  return {
    request: async <T,>(method: string, params?: unknown, opts?: Parameters<Backend["request"]>[2]): Promise<T> => {
      const trace = opts?.trace;
      if (!trace) return backend.request<T>(method, params, opts);
      const started = performance.now();
      const done = (outcome: "ok" | "error", code?: string) => log({ ev: "ui.rpc", trace, method, ms: Math.round(performance.now() - started), outcome, ...(code ? { code } : {}) });
      try {
        const r = await backend.request<T>(method, params, opts);
        done("ok");
        return r;
      } catch (e) {
        done("error", codeOf(e));
        throw e;
      }
    },
    subscribe: (topic, opts, onEvent, onEnd) => backend.subscribe(topic, opts, onEvent, onEnd),
    diagLog: (events) => backend.diagLog(events),
    watchHubLink: (cb) => backend.watchHubLink(cb),
    reconnectHub: () => backend.reconnectHub(),
    appVersion: () => backend.appVersion(),
    ...(backend.onNavigate ? { onNavigate: (cb: Parameters<NonNullable<Backend["onNavigate"]>>[0]) => backend.onNavigate!(cb) } : {}),
  };
}

/** The files of a prompt, as counted for the log. */
export interface FilesSummary {
  count: number;
  bytes: number;
}
export const NO_FILES: FilesSummary = { count: 0, bytes: 0 };

/** Characters (code points), as the daemon counts them in a trail entry. */
export const charCount = (s: string): number => [...s].length;

/** The total size of files (attachments, picked files). */
export const sumBytes = (files: readonly { size: number }[]): number => files.reduce((a, f) => a + f.size, 0);

/**
 * One user action that sends something to a session or launches one: a trace id that travels
 * with every request of the action (the hub and the host record it), and the `ui.*` events that
 * say what the app did. Created by the actions layer, which is the only place that does.
 */
export interface Trace {
  readonly id: string;
  /** The app accepted a prompt (or a slash command) for the session. */
  prompt(host: string, session: string, kind: "prompt" | "slash", length: number, files: FilesSummary): void;
  /** The app sent `c` to the session: a prompt (`ui.prompt`) or another command (`ui.command`). */
  sent(host: string, session: string, c: Command): void;
  /** The app is stopping or continuing a session (`ui.command`). */
  command(host: string, session: string, command: "stop" | "continue"): void;
  /** The app launches (or resumes) a session; `firstPrompt`: it will be given one (never recorded). */
  launch(host: string, r: LaunchRequest, firstPrompt: boolean, attachments: number): void;
  /** Run an upload of attachments for this action, recording its size, time and outcome. */
  upload<T>(files: FilesSummary, run: () => Promise<T>): Promise<T>;
}

export function createTracer(log: (e: UiEvent) => void): () => Trace {
  return () => {
    const id = newTraceId();
    const prompt: Trace["prompt"] = (host, session, kind, length, files) => log({ ev: "ui.prompt", trace: id, host, session, kind, chars: length, attachments: files.count, bytes: files.bytes });
    return {
      id,
      prompt,
      sent(host, session, c) {
        if (c.command === "prompt") prompt(host, session, "prompt", charCount(c.text), { count: c.attachments?.length ?? 0, bytes: sumBytes(c.attachments ?? []) });
        else if (c.command === "run_slash") prompt(host, session, "slash", charCount(c.args), NO_FILES);
        else log({ ev: "ui.command", trace: id, host, session, command: c.command });
      },
      command: (host, session, command) => log({ ev: "ui.command", trace: id, host, session, command }),
      launch: (host, r, firstPrompt, attachments) =>
        log({ ev: "ui.launch", trace: id, host, driver: r.driver, resume: r.resume.kind === "new" ? "fresh" : r.resume.kind === "latest" ? "continue" : "resume", first_prompt: firstPrompt, attachments }),
      async upload(files, run) {
        const started = performance.now();
        const done = (outcome: "ok" | "error", code?: string) => log({ ev: "ui.upload", trace: id, files: files.count, bytes: files.bytes, ms: Math.round(performance.now() - started), outcome, ...(code ? { code } : {}) });
        try {
          const r = await run();
          done("ok");
          return r;
        } catch (e) {
          done("error", codeOf(e));
          throw e;
        }
      },
    };
  };
}
