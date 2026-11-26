import { useEffect, useRef, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { AuthPrompt } from "../../bridge/wire";
import { fleetLive, isPromptGone, NO_SCREEN, reconcileSignInScreen, signInRepeatWords, useActions, useApi, useAppState, usePending, useSignInQueue, type SignInQuestion, type SignInScreen } from "../../state";
import { Button, Dialog, Icon, useNow } from "../../ui";

const GONE_WORDS = "That question is no longer pending: its sign-in attempt ended, so your answer was not used.";

/** The sign-in dialog has no way out but its own buttons (Esc and a click outside do nothing). */
const stay = () => undefined;

/**
 * The sign-in dialog: the questions of every host that asks, one at a time, oldest first. The one on
 * screen is bound to its id, so an answer typed for it can only reach that question (the daemon
 * refuses an id it no longer holds); when it is withdrawn the dialog says so rather than putting
 * another host's question under the user's cursor. While the app has no live link to its daemon
 * nothing can be answered, so nothing is shown (the connection banner says why) and the question
 * is shown again, or reported withdrawn, once the daemon has told the truth.
 */
export function SignInDialogs() {
  const queue = useSignInQueue();
  const live = useAppState(fleetLive);
  // Questions the user answered or cancelled here: their disappearance is the expected result, not a withdrawal.
  const settled = useRef(new Set<number>());
  const [screen, setScreen] = useState<SignInScreen>(NO_SCREEN);
  const next = reconcileSignInScreen(screen, queue, settled.current);
  if (next !== screen) setScreen(next);
  if (!live) return null;
  if (next.kind === "asking") {
    const q = queue.find((x) => x.prompt.id === next.id)!;
    return <AuthPromptDialog key={q.prompt.id} prompt={q.prompt} hostName={q.hostName} others={queue.filter((x) => x !== q)} settle={(id, yes) => (yes ? settled.current.add(id) : settled.current.delete(id))} />;
  }
  if (next.kind === "withdrawn") {
    return <WithdrawnDialog hostName={next.hostName} others={queue} onClose={() => setScreen(NO_SCREEN)} />;
  }
  return null;
}

/** "gpu-box", "gpu-box and pi", "gpu-box, pi and hz". */
const names = (qs: SignInQuestion[]) => qs.map((q) => q.hostName).reduce((acc, n, i, all) => (i === 0 ? n : i === all.length - 1 ? `${acc} and ${n}` : `${acc}, ${n}`), "");

/** The questions of other hosts that wait their turn (`after`: behind the one on screen). */
function Waiting({ others, after }: { others: SignInQuestion[]; after?: boolean }) {
  if (!others.length) return null;
  return (
    <div className="authp-queue" role="status" data-testid="auth-queue">
      <Icon name="key" size="sm" />
      <span>{others.length === 1 ? "1 more host is waiting" : `${others.length} more hosts are waiting`}{after ? " after this one" : ""}: {names(others)}. They are asked one at a time, oldest first.</span>
    </div>
  );
}

/**
 * One sign-in question of a host's link: the daemon's text verbatim, one input per kind (secret,
 * one-time code, yes/no, info / push approval). The answer goes straight to `hosts.auth_answer`, with
 * this question's id, and is never kept: the field is cleared as soon as it is sent.
 */
export function AuthPromptDialog({ prompt, hostName, others, settle }: { prompt: AuthPrompt; hostName: string; others: SignInQuestion[]; settle: (id: number, settled: boolean) => void }) {
  const api = useApi();
  const actions = useActions();
  const [value, setValue] = useState("");
  const [refused, setRefused] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const text = useRef<HTMLPreElement>(null);
  const mounted = useRef(true);
  useEffect(() => () => void (mounted.current = false), []);
  const pk = `auth:${prompt.host}:${prompt.id}`;
  const answering = !!usePending(pk);
  const cancelling = !!usePending(`${pk}:cancel`);
  const asksEachTime = useAppState((s) => !!s.hosts[prompt.host]?.entry.interactive_auth);
  const now = useNow(true);
  // The daemon's own deadline for this question; the countdown is only its difference to now.
  const left = Math.max(0, Math.round((prompt.expires_at_ms - now) / 1000));
  const again = signInRepeatWords(prompt.repeat);

  /** One request about this question, bound to its id; the daemon's refusal is shown here, not in a toast behind the dialog. */
  const send = async (key: string, failTitle: string, call: () => Promise<unknown>) => {
    settle(prompt.id, true);
    setRefused(null);
    const r = await actions.run(key, failTitle, call, {
      onError: (e) => {
        // A dialog that is already gone cannot show it: the toast does.
        if (!mounted.current) return false;
        setRefused(isPromptGone(e) ? GONE_WORDS : `${key === pk ? "Your answer was not accepted" : "Cancelling did not go through"}: ${errorMessage(e)}`);
        return true;
      },
    });
    if (r === undefined) settle(prompt.id, false);
  };
  const answer = (a: string) => {
    setValue("");
    void send(pk, "Could not send your answer", () => api.hosts.authAnswer({ host: prompt.host, id: prompt.id, answer: a }));
  };
  const cancel = () => void send(`${pk}:cancel`, "Could not cancel the sign-in", () => api.hosts.authCancel({ host: prompt.host, id: prompt.id }));
  // Every question but ssh's own host-key confirmation takes a typed answer, masked. The kind only
  // words the field; it never decides whether the answer is hidden (the daemon's guess at the kind
  // is from the prompt's text, and a password can mention a token).
  const typed = prompt.kind !== "confirm";
  const busy = answering || cancelling;

  return (
    <Dialog open persistent onClose={stay} title={`Sign in to ${hostName}`} width={500} initialFocus={typed ? input : text}
      footer={
        <>
          <span className="faint small authp-timer tnum" aria-live="polite">{left > 0 ? `Gives up in ${left >= 60 ? `${Math.floor(left / 60)}m ${left % 60}s` : `${left}s`}` : "Expired"}</span>
          <div className="authp-acts">
            <Button pending={cancelling} disabled={answering} onClick={cancel}>Cancel sign-in</Button>
            {typed ? <Button variant="primary" disabled={!value || cancelling} pending={answering} onClick={() => answer(value)}>Sign in</Button> : null}
            {prompt.kind === "confirm" ? (
              <>
                <Button disabled={cancelling} pending={answering} onClick={() => answer("no")}>No</Button>
                <Button variant="primary" disabled={cancelling} pending={answering} onClick={() => answer("yes")}>Yes</Button>
              </>
            ) : null}
            {prompt.kind === "info" ? <Button disabled={cancelling} pending={answering} onClick={() => answer("")}>Continue</Button> : null}
          </div>
        </>
      }>
      <form className="authp" onSubmit={(e) => { e.preventDefault(); if (typed && value && !busy) answer(value); }}>
        <Waiting others={others} after />
        {again ? <div className="authp-again" role="note" data-testid="auth-repeat"><Icon name="refresh" size="sm" />{again}</div> : null}
        {prompt.attempt > 1 ? <div className="authp-retry c-failed" role="alert"><Icon name="warn" size="sm" />That did not work. Try again (attempt {prompt.attempt}).</div> : null}
        <div className="eyebrow">{hostName} asks</div>
        <pre ref={text} tabIndex={-1} className="authp-text" data-testid="auth-prompt-text">{prompt.prompt}</pre>
        {typed ? (
          <input ref={input} className="authp-input" type="password" inputMode={prompt.kind === "code" ? "numeric" : undefined} readOnly={busy}
            autoComplete={prompt.kind === "code" ? "one-time-code" : "off"} aria-label={fieldLabel(prompt.kind)} value={value} onChange={(e) => setValue(e.target.value)} spellCheck={false} />
        ) : null}
        {prompt.kind === "info" ? <div className="authp-wait" role="status"><Icon name="working" size="sm" spin className="c-working" />Waiting for approval on your device…</div> : null}
        {refused ? <div className="authp-retry c-failed" role="alert" data-testid="auth-refused"><Icon name="warn" size="sm" />{refused}</div> : null}
        <p className="faint small">Marsad passes your answer to ssh on this machine and does not store it.</p>
        <p className="faint small" data-testid="auth-cancel-note">
          Cancel sign-in ends this attempt.{" "}
          {asksEachTime ? `${hostName} does not reconnect on its own, so you will have to click Sign in again to try again.` : `To try again, connect ${hostName} from Hosts.`}
        </p>
      </form>
    </Dialog>
  );
}

/** The field's name, by what the question seems to ask (wording only). */
function fieldLabel(kind: AuthPrompt["kind"]): string {
  if (kind === "code") return "One-time code";
  if (kind === "secret") return "Password or passphrase";
  return "Answer";
}

/** The question on screen was withdrawn by the daemon (its attempt ended) before the user acted on it. */
function WithdrawnDialog({ hostName, others, onClose }: { hostName: string; others: SignInQuestion[]; onClose: () => void }) {
  const ok = useRef<HTMLButtonElement>(null);
  return (
    <Dialog open persistent onClose={stay} title={`Sign in to ${hostName}`} width={480} initialFocus={ok}
      footer={<><span className="sp" /><Button ref={ok} variant="primary" onClick={onClose}>{others.length ? "Show the next question" : "OK"}</Button></>}>
      <div className="authp" data-testid="auth-withdrawn">
        <div className="authp-retry c-needs" role="alert"><Icon name="warn" size="sm" />The question from {hostName} was withdrawn.</div>
        <p>Its sign-in attempt ended before you answered, so nothing you typed was used and nothing was sent. If {hostName} still needs you, sign in again from the Hosts page.</p>
        <Waiting others={others} />
      </div>
    </Dialog>
  );
}
