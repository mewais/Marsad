import type { Api } from "../bridge/api";
import { errorMessage } from "../bridge/api";
import type { Command, GateDecision, QuestionAnswer, SessionKey } from "../bridge/wire";
import { splitKey } from "../bridge/wire";
import { toast } from "../ui/toast";
import { seenState } from "./controller";
import type { AppStore } from "./model";

export type HandoffChoice = "allow" | "always" | "deny";

export interface Actions {
  /** Fire-and-track any request: pending flag under `pk`, toast with the backend's message on failure. */
  run<T>(pk: string, failTitle: string, fn: () => Promise<T>, opts?: { keepSent?: boolean }): Promise<T | undefined>;
  /** Fire a command at a session. Resolves true when the host accepted it. */
  command(key: SessionKey, command: Command, opts?: { pendingKey?: string; failTitle?: string; keepSent?: boolean }): Promise<boolean>;
  answerGate(key: SessionKey, requestId: string, decision: GateDecision): Promise<boolean>;
  answerQuestion(key: SessionKey, requestId: string, answer: QuestionAnswer): Promise<boolean>;
  /** Handoff gates are answered like any gate (wasl-proto `HandoffDecision::from_gate`). */
  decideHandoff(key: SessionKey, requestId: string, choice: HandoffChoice): Promise<boolean>;
  sendPrompt(key: SessionKey, text: string): Promise<boolean>;
  stop(key: SessionKey): Promise<boolean>;
  /** End the session's process (sessions.stop). */
  stopSession(key: SessionKey): Promise<boolean>;
  retry(key: SessionKey): Promise<boolean>;
  askToFix(key: SessionKey, reason: string): Promise<boolean>;
  markSeen(keys: SessionKey[]): void;
  markAllSeen(): void;
  /** Fetch `git.status` for a session once (branch, diffstat). */
  ensureGit(key: SessionKey, force?: boolean): void;
}

export const gatePendingKey = (key: SessionKey, requestId: string) => `gate:${key}:${requestId}`;

export const handoffDecision = (choice: HandoffChoice): GateDecision =>
  choice === "deny" ? { action: "deny", reason: null } : choice === "always" ? { action: "approve", permission_updates: { always_allow: true } } : { action: "approve" };

/**
 * Every action is fire, pending state, result: the UI is never blocked on the backend.
 * `pending[...]` drives disabled buttons and spinners; failures become toasts.
 */
export function createActions(store: AppStore, api: Api): Actions {
  const setPending = (k: string, v: "pending" | "sent" | null) =>
    store.set((s) => {
      const p = { ...s.pending };
      if (v) p[k] = v;
      else delete p[k];
      return { ...s, pending: p };
    });

  const run: Actions["run"] = async (pk, failTitle, fn, o = {}) => {
    if (store.get().pending[pk]) return undefined;
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
      toast({ tone: "err", title: failTitle, detail: errorMessage(e) });
      return undefined;
    }
  };
  const ok = (p: Promise<unknown>) => p.then((r) => r !== undefined);

  const command: Actions["command"] = (key, c, o = {}) => {
    const { host, id } = splitKey(key);
    return ok(run(o.pendingKey ?? `cmd:${key}:${c.command}`, o.failTitle ?? "That did not go through", () => api.command(host, id, c), { keepSent: o.keepSent }));
  };

  const gitInFlight = new Set<SessionKey>();
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
    decideHandoff: (key, requestId, choice) =>
      command(key, { command: "answer_gate", request_id: requestId, decision: handoffDecision(choice) }, { pendingKey: gatePendingKey(key, requestId), failTitle: "Could not record the handoff decision", keepSent: true }),
    sendPrompt: (key, text) => command(key, { command: "prompt", text }, { failTitle: "Could not send your message" }),
    stop: (key) => command(key, { command: "interrupt" }, { failTitle: "Could not interrupt the session" }),
    stopSession: (key) => {
      const { host, id } = splitKey(key);
      return ok(run(`stop:${key}`, "Could not stop the session", () => api.stopSession(host, id)));
    },
    retry: (key) => {
      const { host, id } = splitKey(key);
      return ok(
        run(`retry:${key}`, "Could not continue the session", async () => {
          await api.continueSession(host, id);
          toast({ tone: "ok", title: "Session continued", detail: "It picked up where it stopped." });
          return true;
        }),
      );
    },
    askToFix: (key, reason) => command(key, { command: "prompt", text: `The last step failed: ${reason}\nPlease find the cause and fix it.` }, { pendingKey: `fix:${key}`, failTitle: "Could not message the agent" }),
    markSeen(keys) {
      const now = Date.now();
      const marks = keys.map((k) => mark(k, now)).filter((m) => !!m);
      if (!marks.length) return;
      store.set((s) => {
        const seen = { ...s.seen };
        for (const m of marks) seen[`${m.host}/${m.session}`] = m;
        return { ...s, seen, lookedAt: Math.max(s.lookedAt, now) };
      });
      for (const m of marks) api.store.markSeen(m).catch((e) => toast({ tone: "err", title: "Could not save “seen”", detail: errorMessage(e) }));
    },
    markAllSeen() {
      const now = Date.now();
      store.set((s) => {
        const seen = { ...s.seen };
        for (const k of Object.keys(s.sessions)) seen[k] = mark(k, now)!;
        return { ...s, seen, lookedAt: now };
      });
      api.store.markAllSeen().then(
        (r) => store.set((s) => ({ ...s, ...seenState(r.markers) })),
        (e) => toast({ tone: "err", title: "Could not mark everything seen", detail: errorMessage(e) }),
      );
    },
    ensureGit(key, force) {
      if (gitInFlight.has(key) || (!force && key in store.get().git)) return;
      const { host, id } = splitKey(key);
      gitInFlight.add(key);
      api.git.status(host, { session: id }).then(
        (g) => store.set((s) => ({ ...s, git: { ...s.git, [key]: g } })),
        () => store.set((s) => ({ ...s, git: { ...s.git, [key]: null } })),
      ).finally(() => gitInFlight.delete(key));
    },
  };
}
