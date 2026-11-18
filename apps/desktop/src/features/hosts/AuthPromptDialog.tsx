import { useEffect, useRef, useState } from "react";
import type { AuthPrompt } from "../../bridge/wire";
import { useActions, useApi, useAppState, usePending } from "../../state";
import { Button, Dialog, Icon, useNow } from "../../ui";

/** The first sign-in prompt waiting on any host (the fleet carries at most one per host). */
export function useAuthPrompt(): { prompt: AuthPrompt; hostName: string } | null {
  return useAppState(
    (s) => {
      for (const id of s.hostOrder) {
        const h = s.hosts[id];
        if (h?.authPrompt) return { prompt: h.authPrompt, hostName: h.entry.name };
      }
      return null;
    },
    (a, b) => a?.prompt === b?.prompt,
  );
}

/**
 * Sign-in prompt of a host's link: the server's prompt text verbatim, one input per kind
 * (secret, one-time code, yes/no, info / push approval). The answer goes straight to
 * `hosts.auth_answer` and is never kept: the field is cleared as soon as it is sent.
 */
export function AuthPromptDialog({ prompt, hostName }: { prompt: AuthPrompt; hostName: string }) {
  const api = useApi();
  const actions = useActions();
  const [value, setValue] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const pk = `auth:${prompt.host}:${prompt.id}`;
  const pending = !!usePending(pk);
  const now = useNow(true);
  useEffect(() => setValue(""), [prompt.id]);
  const left = Math.max(0, Math.round((prompt.expires_at_ms - now) / 1000));

  const answer = (a: string) => {
    setValue("");
    void actions.run(pk, "Could not send your answer", () => api.hosts.authAnswer({ host: prompt.host, id: prompt.id, answer: a }));
  };
  const cancel = () => void actions.run(`${pk}:cancel`, "Could not cancel the sign-in", () => api.hosts.authCancel({ host: prompt.host, id: prompt.id }));
  const typed = prompt.kind === "secret" || prompt.kind === "code";

  return (
    <Dialog open onClose={cancel} title={`Sign in to ${hostName}`} width={480} initialFocus={typed ? input : undefined}
      footer={
        <>
          <span className="faint small" aria-live="polite">{left > 0 ? `Gives up in ${left >= 60 ? `${Math.floor(left / 60)}m ${left % 60}s` : `${left}s`}` : "Expired"}</span>
          <span className="sp" />
          <Button onClick={cancel}>Cancel sign-in</Button>
          {typed ? <Button variant="primary" disabled={!value} pending={pending} onClick={() => answer(value)}>Sign in</Button> : null}
          {prompt.kind === "confirm" ? (
            <>
              <Button variant="deny" pending={pending} onClick={() => answer("no")}>No</Button>
              <Button variant="primary" pending={pending} onClick={() => answer("yes")}>Yes</Button>
            </>
          ) : null}
          {prompt.kind === "info" ? <Button pending={pending} onClick={() => answer("")}>Continue</Button> : null}
        </>
      }>
      <form className="authp" onSubmit={(e) => { e.preventDefault(); if (typed && value) answer(value); }}>
        {prompt.attempt > 1 ? <div className="authp-retry c-failed" role="alert"><Icon name="warn" size="sm" />That did not work. Try again (attempt {prompt.attempt}).</div> : null}
        <div className="eyebrow">{hostName} asks</div>
        <pre className="authp-text" data-testid="auth-prompt-text">{prompt.prompt}</pre>
        {typed ? (
          <input ref={input} className="authp-input" type={prompt.kind === "secret" ? "password" : "text"} inputMode={prompt.kind === "code" ? "numeric" : undefined}
            autoComplete={prompt.kind === "code" ? "one-time-code" : "off"} aria-label={prompt.kind === "secret" ? "Password or passphrase" : "One-time code"} value={value} onChange={(e) => setValue(e.target.value)} spellCheck={false} />
        ) : null}
        {prompt.kind === "info" ? <div className="authp-wait" role="status"><Icon name="working" size="sm" spin className="c-working" />Waiting for approval on your device…</div> : null}
        <p className="faint small">Marsad passes your answer to ssh on this machine and does not store it.</p>
      </form>
    </Dialog>
  );
}
