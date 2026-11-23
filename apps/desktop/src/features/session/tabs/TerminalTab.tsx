import { toast } from "../../../ui/toast";
import { useEffect, useRef, useState } from "react";
import { errorMessage } from "../../../bridge/api";
import { splitKey } from "../../../bridge/wire";
import { useApi, useReach } from "../../../state";
import { Button, ConfirmDialog, EmptyState, Icon } from "../../../ui";

const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/** The size asked for when this tab starts a shell; the real size is sent as soon as the terminal is attached. */
const START_COLS = 100;
const START_ROWS = 30;

/**
 * What the tab knows of the session's terminal, and only what the host has said:
 * `attaching`: asking which terminal the session has; `starting`: it has none, a shell is being
 * started; `live`: the host named a live terminal and the output follows it; `exited`: its shell
 * exited (the host's status, the scrollback stays readable); `closing`: the user closed it and the
 * host has not answered; `closed`: it was closed, none is open; `lost`: the output stream ended
 * without the shell exiting; `error`: the host could not be asked, or refused.
 */
type State =
  | { kind: "attaching" }
  | { kind: "starting" }
  | { kind: "live"; pty: string; cwd: string }
  | { kind: "exited"; pty: string; code: number | null }
  | { kind: "closing"; pty: string }
  | { kind: "closed" }
  | { kind: "lost"; message: string }
  | { kind: "error"; message: string };

/** `attach`: this session's terminal, started when it has none. `open`: start a new one (the old one is gone or exited). `look`: only ask. */
type Mode = "attach" | "open" | "look";

/**
 * Terminal tab: the session's terminal on its host over `pty.*`, rendered by xterm.js (loaded on
 * demand). A session has one terminal, kept by the host: opening this tab attaches to it (asks the
 * host, starts a shell only when there is none), replays the scrollback the host still has and
 * follows it live; leaving the tab only detaches. "Close terminal" and the shell exiting are what
 * end it, and the tab says which happened.
 */
export function TerminalTab({ sessionKey }: { sessionKey: string }) {
  const { host, id } = splitKey(sessionKey);
  const api = useApi();
  const reach = useReach(host);
  const box = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<State>({ kind: "attaching" });
  const [run, setRun] = useState<{ n: number; mode: Mode }>({ n: 0, mode: "attach" });
  const [confirm, setConfirm] = useState(false);
  /** The one gate for what goes to the shell: set by the host's answers and events, never by what was rendered. */
  const gate = useRef({ live: false, reach: null as string | null });
  gate.current.reach = reach;
  /** Detaches the current attachment (subscription, xterm, observers); safe to call again. */
  const release = useRef<() => void>(() => undefined);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => void (mounted.current = false);
  }, []);

  const again = (mode: Mode, to: State) => {
    setState(to);
    setRun((r) => ({ n: r.n + 1, mode }));
  };

  useEffect(() => {
    let disposed = false;
    const cleanup: (() => void)[] = [];
    const detach = () => {
      gate.current.live = false;
      cleanup.splice(0).reverse().forEach((c) => c());
    };
    release.current = detach;
    gate.current.live = false;
    (async () => {
      try {
        let info = run.mode === "open" ? undefined : (await api.pty.list(host)).find((p) => p.session === id);
        if (disposed) return;
        if (!info && run.mode === "look") return void setState({ kind: "closed" });
        if (!info) {
          setState({ kind: "starting" });
          // Not closed when this tab is left meanwhile: it is the session's terminal, and the next visit attaches to it.
          info = (await api.pty.start(host, { target: { session: id }, cols: START_COLS, rows: START_ROWS })).pty;
          if (disposed) return;
        }
        const t = info;
        const { createTerminal } = await import("./xterm");
        if (disposed || !box.current) return;
        const { term, fit } = createTerminal(box.current, { background: css("--sunken"), foreground: css("--fg"), cursor: css("--accent") });
        cleanup.push(() => term.dispose());
        let over = t.exited;
        gate.current.live = !t.exited;
        setState(t.exited ? { kind: "exited", pty: t.id, code: t.exit_code ?? null } : { kind: "live", pty: t.id, cwd: t.cwd });
        const off = api.pty.output(
          host,
          t.id,
          0,
          (e) => {
            // `data` is the raw terminal bytes, base64 (marsad-proto `PtyEvent::Output`).
            if (e.kind === "output") return term.write(base64Bytes(e.data));
            over = true;
            gate.current.live = false;
            // The user's own close is answered by its own result.
            setState((s) => (s.kind === "closing" ? s : { kind: "exited", pty: t.id, code: e.code ?? null }));
          },
          (end) => {
            // After `exited` (or the user's close) the end is expected; anything else is the host or the link.
            if (over) return;
            gate.current.live = false;
            setState((s) => (s.kind === "closing" ? s : { kind: "lost", message: end ? errorMessage(end) : "The host ended the terminal's output." }));
          },
        );
        cleanup.push(off);
        // Keystrokes go out strictly in order: one request at a time, typed-ahead input coalesced
        // (concurrent requests may be served out of order). Only to a terminal the host said is live.
        let pending = "";
        let sending = false;
        const flush = async () => {
          if (sending) return;
          sending = true;
          while (pending && !disposed) {
            const chunk = pending;
            pending = "";
            if (!gate.current.live || gate.current.reach) continue;
            await api.pty.input(host, t.id, chunk).catch((e) => toast({ tone: "err", title: "Could not send input to the terminal", detail: errorMessage(e) }));
          }
          sending = false;
        };
        const sub = term.onData((d) => {
          if (!gate.current.live || gate.current.reach) return;
          pending += d;
          void flush();
        });
        cleanup.push(() => sub.dispose());
        // The size follows the box; the host is told once attached and on every change after.
        const resize = () => {
          fit.fit();
          if (!gate.current.live || gate.current.reach) return;
          api.pty.resize(host, t.id, term.cols, term.rows).catch((e) => toast({ tone: "err", title: "Could not resize the terminal", detail: errorMessage(e) }));
        };
        const ro = new ResizeObserver(resize);
        ro.observe(box.current);
        cleanup.push(() => ro.disconnect());
        resize();
        if (!t.exited) term.focus();
      } catch (e) {
        if (!disposed) setState({ kind: "error", message: errorMessage(e) });
      }
    })();
    return () => {
      disposed = true;
      detach();
    };
  }, [api, host, id, run]);

  const closeTerminal = async (pty: string) => {
    gate.current.live = false;
    setState({ kind: "closing", pty });
    try {
      await api.pty.close(host, pty);
    } catch (e) {
      if (!mounted.current) return;
      toast({ tone: "err", title: "Could not close the terminal", detail: errorMessage(e) });
      // What became of it is the host's to say: ask again rather than guess.
      return again("look", { kind: "attaching" });
    }
    if (!mounted.current) return;
    release.current();
    setState({ kind: "closed" });
  };

  if (state.kind === "error")
    return (
      <div className="placeholder">
        <EmptyState icon="term" title="Terminal not available" action={<Button size="sm" icon="refresh" onClick={() => again("attach", { kind: "attaching" })}>Try again</Button>}>
          {state.message}
        </EmptyState>
      </div>
    );
  if (state.kind === "closed")
    return (
      <div className="placeholder">
        <EmptyState icon="term" title="No terminal is open" action={<Button size="sm" variant="primary" icon="term" blocked={reach} onClick={() => again("open", { kind: "starting" })}>Open a new terminal</Button>}>
          The shell and anything that was running in it have ended.
        </EmptyState>
      </div>
    );
  return (
    <div className="termwrap">
      <div className="faint term-status" role="status">
        {state.kind === "attaching" ? "Looking for this session's terminal on the host…" : null}
        {state.kind === "starting" ? "Starting a shell on the host…" : null}
        {state.kind === "live" ? (
          reach ? (
            <span className="c-needs"><Icon name="warn" size="xs" /> {reach} Typing is off until it is back.</span>
          ) : (
            <span className="mono term-cwd">{state.cwd}</span>
          )
        ) : null}
        {state.kind === "closing" ? "Closing the terminal…" : null}
        {state.kind === "exited" ? `The shell exited${state.code != null ? ` with status ${state.code}` : ""}.` : null}
        {state.kind === "lost" ? `Lost the terminal: ${state.message}` : null}
        <span className="sp" />
        {state.kind === "live" || state.kind === "closing" ? (
          <Button size="sm" variant="ghost" icon="stop" blocked={reach} pending={state.kind === "closing"} onClick={() => setConfirm(true)}>Close terminal</Button>
        ) : null}
        {state.kind === "exited" ? <Button size="sm" variant="ghost" icon="term" blocked={reach} onClick={() => again("open", { kind: "starting" })}>Open a new terminal</Button> : null}
        {state.kind === "lost" ? <Button size="sm" variant="ghost" icon="refresh" onClick={() => again("look", { kind: "attaching" })}>Reattach</Button> : null}
      </div>
      <div ref={box} className="xterm-box" aria-label="Terminal" />
      <ConfirmDialog
        open={confirm}
        title="Close this terminal?"
        confirmLabel="Close terminal"
        danger
        onConfirm={() => {
          setConfirm(false);
          if (state.kind === "live") void closeTerminal(state.pty);
        }}
        onClose={() => setConfirm(false)}
      >
        <p>The shell and anything running in it will be ended. Marsad cannot tell whether something is still running there.</p>
      </ConfirmDialog>
    </div>
  );
}

function base64Bytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
