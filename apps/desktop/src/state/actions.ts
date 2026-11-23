import type { Api } from "../bridge/api";
import { errorMessage } from "../bridge/api";
import type { Attachment, Command, GateDecision, HostId, LaunchReply, LaunchRequest, QuestionAnswer, SessionKey, UiEvent } from "../bridge/wire";
import { splitKey } from "../bridge/wire";
import { toast } from "../ui/toast";
import { seenState } from "./controller";
import type { Controller } from "./controller";
import { charCount, type FilesSummary, type Trace } from "./diaglog";
import type { AppState, AppStore, LoadKey } from "./model";
import { reachBlock } from "./selectors";

/** Files for a prompt: how many and how big (for the log), and what uploads them to the session's host. */
export interface PromptFiles extends FilesSummary {
  upload: () => Promise<Attachment[]>;
}

export interface Actions {
  /**
   * Fire-and-track any request: pending flag under `pk`, toast with the backend's message on failure.
   * A request that needs the daemon is not made while the app is not connected to it (or, with
   * `host`, while that host is not connected): it fails at once with the reason instead of waiting
   * on the link. `local`: answered by the desktop shell itself, needs no daemon. `onError`: the failure goes
   * to the caller first; when it returns `true` it has shown the reason where the user is looking (next to the field,
   * under the message box) and no toast is raised.
   */
  run<T>(pk: string, failTitle: string, fn: () => Promise<T>, opts?: { keepSent?: boolean; host?: HostId; local?: boolean; onError?: (e: unknown) => boolean | void }): Promise<T | undefined>;
  /** Fire a command at a session. Resolves true when the host accepted it. */
  command(key: SessionKey, command: Command, opts?: { pendingKey?: string; failTitle?: string; keepSent?: boolean; onError?: (e: unknown) => boolean | void }): Promise<boolean>;
  answerGate(key: SessionKey, requestId: string, decision: GateDecision): Promise<boolean>;
  answerQuestion(key: SessionKey, requestId: string, answer: QuestionAnswer): Promise<boolean>;
  /**
   * Send a prompt; with `files`, upload them first and send them with it. Resolves true when the host accepted it.
   * To an ended session the daemon continues it first and then delivers the prompt; it refuses with
   * `continue_failed` (reason attached) when it cannot, and nothing was sent.
   */
  sendPrompt(key: SessionKey, text: string, files?: PromptFiles, opts?: { onError?: (e: unknown) => boolean | void }): Promise<boolean>;
  /** Launch a session (hub): pending flag under `pk`, toast on failure. Resolves to its reply, or undefined when it did not start. */
  launch(pk: string, request: LaunchRequest, opts?: { onError?: (e: unknown) => boolean | void }): Promise<LaunchReply | undefined>;
  /** Begin a user action that takes several calls (a launch that uploads files): its trace id and `ui.*` events. */
  trace(): Trace;
  /** Record something the app did that is not a command (`ui.developer_mode`, `ui.render_error`…). Never waits, never fails. */
  record(event: UiEvent): void;
  /** Cancel the session's current turn; the agent keeps running (the `interrupt` command). */
  interrupt(key: SessionKey): Promise<boolean>;
  /** Stop one background task or subagent the agent runs, by the agent's task id (the `stop_task` command). */
  stopTask(key: SessionKey, taskId: string): Promise<boolean>;
  /** Shut the agent's process down and keep the session (`sessions.stop`). */
  endSession(key: SessionKey): Promise<boolean>;
  /** Start a new process for an ended session, with its earlier transcript (`sessions.continue`). */
  continueSession(key: SessionKey): Promise<boolean>;
  askToFix(key: SessionKey, reason: string): Promise<boolean>;
  /** Record that the human looked at these sessions. Takes effect when the daemon confirms it. */
  markSeen(keys: SessionKey[]): void;
  markAllSeen(): void;
  /**
   * Ask the hub to connect a host now (`hosts.reconnect`; pending under `reconnectPendingKey`). For a host
   * that needs the user at sign-in this is the one click that starts the sign-in: the hub arms one attempt
   * and the host's questions follow. Resolves true when the hub took the request; its refusal is shown.
   */
  reconnectHost(id: HostId, name: string): Promise<boolean>;
  /**
   * Sign out of a host (`hosts.signout`; pending under `signOutPendingKey`): the hub ends the shared ssh
   * connection it keeps open to it, the link drops and the host waits for the user to sign in again. Only
   * offered for a host whose entry says it has such a connection. Resolves true when the hub took the request.
   */
  signOutHost(id: HostId, name: string): Promise<boolean>;
  /** Connect to the local daemon again now (the app's own link). */
  reconnectHub(): Promise<void>;
  /** Start the subscriptions that ended again. */
  retryLive(): void;
  /** Load a stored document again after it failed to load. */
  reload(what: LoadKey): void;
  /** Ask the hub to refresh the git summary of a session's folder now (e.g. after a commit). */
  refreshGit(key: SessionKey): void;
}

/** The answer reached a gate whose request is gone (the runtime's typed `gate_withdrawn`). */
export function isGateWithdrawn(e: unknown): boolean {
  const r = e as { code?: string; data?: { error?: string } | null } | undefined;
  return r?.code === "gate_withdrawn" || r?.data?.error === "gate_withdrawn";
}

/** Pending-flag keys of the session actions (the buttons that start them show their state). */
export const interruptPendingKey = (key: SessionKey) => `cmd:${key}:interrupt`;
export const stopTaskPendingKey = (key: SessionKey, taskId: string) => `cmd:${key}:stop_task:${taskId}`;
export const endPendingKey = (key: SessionKey) => `end:${key}`;
export const continuePendingKey = (key: SessionKey) => `continue:${key}`;
export const reconnectPendingKey = (host: HostId) => `reconnect:${host}`;
export const signOutPendingKey = (host: HostId) => `signout:${host}`;
export const gatePendingKey = (key: SessionKey, requestId: string) => `gate:${key}:${requestId}`;

/**
 * Every action is fire, pending state, result: the UI is never blocked on the backend.
 * `pending[...]` drives disabled buttons and spinners; failures become toasts.
 */
export function createActions(store: AppStore, api: Api, controller: Controller, diag: { trace: () => Trace; log: (e: UiEvent) => void }): Actions {
  const { trace } = diag;
  const setPending = (k: string, v: "pending" | "sent" | null) =>
    store.set((s) => {
      const p = { ...s.pending };
      if (v) p[k] = v;
      else delete p[k];
      return { ...s, pending: p };
    });

  const run: Actions["run"] = async (pk, failTitle, fn, o = {}) => {
    if (store.get().pending[pk]) return undefined;
    const blocked = o.local ? null : reachBlock(store.get(), o.host);
    if (blocked) {
      toast({ tone: "err", title: failTitle, detail: blocked });
      return undefined;
    }
    setPending(pk, "pending");
    try {
      const r = await fn();
      if (o.keepSent) {
        setPending(pk, "sent");
        // The host's snapshot normally removes the gate first; never leave buttons dead for long.
        setTimeout(() => store.get().pending[pk] === "sent" && setPending(pk, null), 10_000);
      } else setPending(pk, null);
      return r ?? (true as never);
    } catch (e) {
      setPending(pk, null);
      if (o.onError?.(e) === true) return undefined;
      if (isGateWithdrawn(e)) toast({ tone: "info", title: "No longer needed", detail: "The request was withdrawn before your answer arrived; nothing was done." });
      else toast({ tone: "err", title: failTitle, detail: errorMessage(e) });
      return undefined;
    }
  };
  const ok = (p: Promise<unknown>) => p.then((r) => r !== undefined);

  // Every command to a session, and every launch, is one traced user action. The trace and its
  // `ui.*` events begin when the request is about to go out: a request refused before that (the
  // host is not connected) was never sent and leaves no trail of having been.
  const command: Actions["command"] = (key, c, o = {}) => {
    const { host, id } = splitKey(key);
    return ok(
      run(o.pendingKey ?? `cmd:${key}:${c.command}`, o.failTitle ?? "That did not go through", () => {
        const t = trace();
        t.sent(host, id, c);
        return api.command(host, id, c, t.id);
      }, { keepSent: o.keepSent, host, onError: o.onError }),
    );
  };

  const localHost = (s: AppState) => s.hostOrder.find((h) => s.hosts[h]?.link.link === "local") ?? "";

  const mark = (key: SessionKey, now: number) => {
    const snap = store.get().sessions[key];
    if (!snap) return null;
    const { host, id } = splitKey(key);
    return { host, session: id, cursor: snap.cursor, at_ms: now };
  };

  return {
    run,
    command,
    answerGate: (key, requestId, decision) =>
      command(key, { command: "answer_gate", request_id: requestId, decision }, { pendingKey: gatePendingKey(key, requestId), failTitle: "Could not send your answer", keepSent: true }),
    answerQuestion: (key, requestId, answer) =>
      command(key, { command: "answer_question", request_id: requestId, answer }, { pendingKey: gatePendingKey(key, requestId), failTitle: "Could not send your answer", keepSent: true }),
    sendPrompt(key, text, files, opts) {
      if (!files?.count) return command(key, { command: "prompt", text }, { failTitle: "Could not send your message", onError: opts?.onError });
      const { host, id } = splitKey(key);
      return ok(
        run(`cmd:${key}:prompt`, "Could not send your message", async () => {
          const t = trace();
          t.prompt(host, id, "prompt", charCount(text), files);
          const attachments = await t.upload(files, files.upload);
          await api.command(host, id, { command: "prompt", text, attachments }, t.id);
          return true;
        }, { host, onError: opts?.onError }),
      );
    },
    launch: (pk, request, opts) =>
      run(pk, "Could not start the session", () => {
        const t = trace();
        t.launch(request.host ?? localHost(store.get()), request, request.first_prompt != null, 0);
        return api.launch(request, t.id);
      }, { onError: opts?.onError }),
    trace,
    record: diag.log,
    interrupt: (key) => command(key, { command: "interrupt" }, { failTitle: "Could not interrupt the session" }),
    stopTask: (key, taskId) => command(key, { command: "stop_task", task_id: taskId }, { pendingKey: stopTaskPendingKey(key, taskId), failTitle: "Could not stop the task" }),
    endSession: (key) => {
      const { host, id } = splitKey(key);
      return ok(run(endPendingKey(key), "Could not end the session", () => {
        const t = trace();
        t.command(host, id, "stop");
        return api.stopSession(host, id, t.id);
      }, { host }));
    },
    continueSession: (key) => {
      const { host, id } = splitKey(key);
      return ok(
        run(continuePendingKey(key), "Could not continue the session", async () => {
          const t = trace();
          t.command(host, id, "continue");
          await api.continueSession(host, id, t.id);
          return true;
        }, { host }),
      );
    },
    askToFix: (key, reason) => command(key, { command: "prompt", text: `The last step failed: ${reason}\nPlease find the cause and fix it.` }, { pendingKey: `fix:${key}`, failTitle: "Could not message the agent" }),
    async markSeen(keys) {
      const now = Date.now();
      const marks = keys.map((k) => mark(k, now)).filter((m) => !!m);
      // Seen only once the daemon has it. Without a link there is nothing to confirm it: the session stays unseen.
      if (!marks.length || reachBlock(store.get())) return;
      const results = await Promise.allSettled(marks.map((m) => api.store.markSeen(m)));
      const saved = marks.filter((_, i) => results[i].status === "fulfilled");
      if (saved.length)
        store.set((s) => {
          const seen = { ...s.seen };
          for (const m of saved) seen[`${m.host}/${m.session}`] = m;
          return { ...s, seen, lookedAt: Math.max(s.lookedAt, now) };
        });
      const failed = results.find((r): r is PromiseRejectedResult => r.status === "rejected");
      if (failed) toast({ tone: "err", title: "Could not save “seen”", detail: errorMessage(failed.reason) });
    },
    markAllSeen() {
      void run("seen:all", "Could not mark everything seen", async () => {
        const r = await api.store.markAllSeen();
        store.set((s) => ({ ...s, ...seenState(r.markers) }));
        return r;
      });
    },
    reconnectHost: (id, name) => ok(run(reconnectPendingKey(id), `Could not connect to ${name}`, () => api.hosts.reconnect(id))),
    signOutHost: (id, name) => ok(run(signOutPendingKey(id), `Could not sign out of ${name}`, () => api.hosts.signOut(id))),
    reconnectHub: async () => {
      // The shell's answer is the link changing; a failure to even ask is the only thing to report here.
      try {
        await api.hub.reconnect();
      } catch (e) {
        toast({ tone: "err", title: "Could not reconnect to the local service", detail: errorMessage(e) });
      }
    },
    retryLive: controller.retryFeeds,
    reload: (what) => void controller.reload(what),
    refreshGit(key) {
      const snap = store.get().sessions[key];
      if (!snap?.cwd) return;
      api.refreshGit(splitKey(key).host, snap.cwd).catch((e) => toast({ tone: "err", title: "Could not refresh git status", detail: errorMessage(e) }));
    },
  };
}
