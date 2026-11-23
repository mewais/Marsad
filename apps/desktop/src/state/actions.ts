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
   * on the link. `local`: answered by the desktop shell itself, needs no daemon.
   */
  run<T>(pk: string, failTitle: string, fn: () => Promise<T>, opts?: { keepSent?: boolean; host?: HostId; local?: boolean }): Promise<T | undefined>;
  /** Fire a command at a session. Resolves true when the host accepted it. */
  command(key: SessionKey, command: Command, opts?: { pendingKey?: string; failTitle?: string; keepSent?: boolean }): Promise<boolean>;
  answerGate(key: SessionKey, requestId: string, decision: GateDecision): Promise<boolean>;
  answerQuestion(key: SessionKey, requestId: string, answer: QuestionAnswer): Promise<boolean>;
  /** Send a prompt; with `files`, upload them first and send them with it. Resolves true when the host accepted it. */
  sendPrompt(key: SessionKey, text: string, files?: PromptFiles): Promise<boolean>;
  /** Launch a session (hub): pending flag under `pk`, toast on failure. Resolves to its reply, or undefined when it did not start. */
  launch(pk: string, request: LaunchRequest): Promise<LaunchReply | undefined>;
  /** Begin a user action that takes several calls (a launch that uploads files): its trace id and `ui.*` events. */
  trace(): Trace;
  /** Record something the app did that is not a command (`ui.developer_mode`, `ui.render_error`…). Never waits, never fails. */
  record(event: UiEvent): void;
  stop(key: SessionKey): Promise<boolean>;
  /** End the session's process (sessions.stop). */
  stopSession(key: SessionKey): Promise<boolean>;
  retry(key: SessionKey): Promise<boolean>;
  askToFix(key: SessionKey, reason: string): Promise<boolean>;
  /** Record that the human looked at these sessions. Takes effect when the daemon confirms it. */
  markSeen(keys: SessionKey[]): void;
  markAllSeen(): void;
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
      }, { keepSent: o.keepSent, host }),
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
    sendPrompt(key, text, files) {
      if (!files?.count) return command(key, { command: "prompt", text }, { failTitle: "Could not send your message" });
      const { host, id } = splitKey(key);
      return ok(
        run(`cmd:${key}:prompt`, "Could not send your message", async () => {
          const t = trace();
          t.prompt(host, id, "prompt", charCount(text), files);
          const attachments = await t.upload(files, files.upload);
          await api.command(host, id, { command: "prompt", text, attachments }, t.id);
          return true;
        }, { host }),
      );
    },
    launch: (pk, request) =>
      run(pk, "Could not start the session", () => {
        const t = trace();
        t.launch(request.host ?? localHost(store.get()), request, request.first_prompt != null, 0);
        return api.launch(request, t.id);
      }),
    trace,
    record: diag.log,
    stop: (key) => command(key, { command: "interrupt" }, { failTitle: "Could not interrupt the session" }),
    stopSession: (key) => {
      const { host, id } = splitKey(key);
      return ok(run(`stop:${key}`, "Could not stop the session", () => {
        const t = trace();
        t.command(host, id, "stop");
        return api.stopSession(host, id, t.id);
      }, { host }));
    },
    retry: (key) => {
      const { host, id } = splitKey(key);
      return ok(
        run(`retry:${key}`, "Could not continue the session", async () => {
          const t = trace();
          t.command(host, id, "continue");
          await api.continueSession(host, id, t.id);
          toast({ tone: "ok", title: "Session continued", detail: "It picked up where it stopped." });
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
