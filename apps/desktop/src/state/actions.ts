import type { Backend } from "../bridge/backend";
import type { Command, GateDecision, HostId, QuestionAnswer, RpcError, SessionKey } from "../bridge/types";
import { methods, splitKey } from "../bridge/types";
import { toast } from "../ui/toast";
import type { AppStore } from "./model";
import { saveSeen } from "./seen";

export interface Actions {
  /** Fire a command at a session. Resolves true when the host accepted it. */
  command(key: SessionKey, command: Command, opts?: { pendingKey?: string; failTitle?: string; keepSent?: boolean }): Promise<boolean>;
  answerGate(key: SessionKey, requestId: string, decision: GateDecision): Promise<boolean>;
  answerQuestion(key: SessionKey, requestId: string, answer: QuestionAnswer): Promise<boolean>;
  decideHandoff(key: SessionKey, requestId: string, choice: "allow" | "always" | "deny", localHost: HostId): Promise<boolean>;
  sendPrompt(key: SessionKey, text: string): Promise<boolean>;
  stop(key: SessionKey): Promise<boolean>;
  /** End the session's process (sessions.stop). */
  stopSession(key: SessionKey): Promise<boolean>;
  retry(key: SessionKey): Promise<boolean>;
  askToFix(key: SessionKey, reason: string): Promise<boolean>;
  markSeen(keys: SessionKey[]): void;
  markAllSeen(): void;
}

export const gatePendingKey = (key: SessionKey, requestId: string) => `gate:${key}:${requestId}`;

function describe(e: unknown): { title: string; detail: string } {
  const r = e as Partial<RpcError>;
  const detail = r?.message ?? (e instanceof Error ? e.message : String(e));
  return { title: r?.code === "host_unreachable" ? "Host unreachable" : "That did not go through", detail };
}

/**
 * Every action is fire, pending state, result: the UI is never blocked on the backend.
 * `pending[...]` drives disabled buttons and spinners; failures become toasts.
 */
export function createActions(store: AppStore, backend: Backend): Actions {
  const setPending = (k: string, v: "pending" | "sent" | null) =>
    store.set((s) => {
      const p = { ...s.pending };
      if (v) p[k] = v;
      else delete p[k];
      return { ...s, pending: p };
    });

  async function run(pk: string, failTitle: string | undefined, fn: () => Promise<unknown>, keepSent = false): Promise<boolean> {
    if (store.get().pending[pk]) return false;
    setPending(pk, "pending");
    try {
      await fn();
      if (keepSent) {
        setPending(pk, "sent");
        // The host's snapshot normally removes the gate first; never leave buttons dead for long.
        setTimeout(() => store.get().pending[pk] === "sent" && setPending(pk, null), 10_000);
      } else setPending(pk, null);
      return true;
    } catch (e) {
      setPending(pk, null);
      const d = describe(e);
      toast({ tone: "err", title: failTitle ?? d.title, detail: d.detail });
      return false;
    }
  }

  const command: Actions["command"] = (key, command, o = {}) => {
    const { host, id } = splitKey(key);
    return run(o.pendingKey ?? `cmd:${key}:${command.command}`, o.failTitle, () => backend.request(methods.SESSION_COMMAND, { id, command }, { host }), o.keepSent);
  };

  const markSeen: Actions["markSeen"] = (keys) => {
    store.set((s) => {
      const seen = { ...s.seen };
      const now = Date.now();
      for (const k of keys) seen[k] = now;
      saveSeen({ seen, lookedAt: s.lookedAt });
      return { ...s, seen };
    });
  };

  return {
    command,
    answerGate: (key, requestId, decision) =>
      command(key, { command: "answer_gate", request_id: requestId, decision }, { pendingKey: gatePendingKey(key, requestId), failTitle: "Could not send your answer", keepSent: true }),
    answerQuestion: (key, requestId, answer) =>
      command(key, { command: "answer_question", request_id: requestId, answer }, { pendingKey: gatePendingKey(key, requestId), failTitle: "Could not send your answer", keepSent: true }),
    decideHandoff: (key, requestId, choice, localHost) =>
      run(gatePendingKey(key, requestId), "Could not record the handoff decision", () => backend.request(methods.HANDOFF_DECIDE, { request_id: requestId, choice }, { host: localHost }), true),
    sendPrompt: (key, text) => command(key, { command: "prompt", text }, { failTitle: "Could not send your message" }),
    stop: (key) => command(key, { command: "interrupt" }, { failTitle: "Could not stop the session" }),
    stopSession: (key) => {
      const { host, id } = splitKey(key);
      return run(`stop:${key}`, "Could not stop the session", () => backend.request(methods.SESSIONS_STOP, { id }, { host }));
    },
    retry: (key) => {
      const { host, id } = splitKey(key);
      return run(`retry:${key}`, "Could not continue the session", async () => {
        await backend.request(methods.SESSIONS_CONTINUE, { id }, { host });
        toast({ tone: "ok", title: "Session continued", detail: "It picked up where it stopped." });
      });
    },
    askToFix: (key, reason) => command(key, { command: "prompt", text: `The last step failed: ${reason}\nPlease find the cause and fix it.` }, { pendingKey: `fix:${key}`, failTitle: "Could not message the agent" }),
    markSeen,
    markAllSeen() {
      store.set((s) => {
        const seen = { ...s.seen };
        const now = Date.now();
        for (const k of Object.keys(s.sessions)) seen[k] = now;
        saveSeen({ seen, lookedAt: now });
        return { ...s, seen, lookedAt: now };
      });
    },
  };
}
