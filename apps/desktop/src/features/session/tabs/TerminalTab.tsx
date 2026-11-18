import { useEffect, useRef, useState } from "react";
import { errorMessage } from "../../../bridge/api";
import { splitKey } from "../../../bridge/wire";
import { useApi } from "../../../state";
import { Button, EmptyState } from "../../../ui";

const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/**
 * Terminal tab: a shell in the session's folder over `pty.*`, rendered by xterm.js (loaded on
 * demand). Reconnecting replays the host's bounded scrollback from offset 0.
 */
export function TerminalTab({ sessionKey }: { sessionKey: string }) {
  const { host, id } = splitKey(sessionKey);
  const api = useApi();
  const box = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<{ kind: "starting" } | { kind: "open"; pty: string } | { kind: "error"; message: string } | { kind: "exited"; code: number | null }>({ kind: "starting" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let disposed = false;
    const cleanup: (() => void)[] = [];
    (async () => {
      try {
        const info = await api.pty.start(host, { target: { session: id }, cols: 100, rows: 30 });
        if (disposed) return void api.pty.close(host, info.id).catch(() => undefined);
        const { createTerminal } = await import("./xterm");
        if (disposed || !box.current) return;
        const { term, fit } = createTerminal(box.current, { background: css("--sunken"), foreground: css("--fg"), cursor: css("--accent") });
        cleanup.push(() => term.dispose());
        setState({ kind: "open", pty: info.id });
        const off = api.pty.output(host, info.id, 0, (e) => {
          if (e.kind === "output") term.write(e.data);
          else setState({ kind: "exited", code: e.code ?? null });
        });
        cleanup.push(off);
        const sub = term.onData((d) => void api.pty.input(host, info.id, d).catch(() => undefined));
        cleanup.push(() => sub.dispose());
        const resize = () => {
          fit.fit();
          void api.pty.resize(host, info.id, term.cols, term.rows).catch(() => undefined);
        };
        const ro = new ResizeObserver(resize);
        ro.observe(box.current);
        cleanup.push(() => ro.disconnect());
        resize();
        term.focus();
      } catch (e) {
        if (!disposed) setState({ kind: "error", message: errorMessage(e) });
      }
    })();
    return () => {
      disposed = true;
      cleanup.forEach((c) => c());
    };
  }, [api, host, id, attempt]);

  if (state.kind === "error")
    return (
      <div className="placeholder">
        <EmptyState icon="term" title="Terminal not available" action={<Button size="sm" icon="refresh" onClick={() => { setState({ kind: "starting" }); setAttempt((a) => a + 1); }}>Try again</Button>}>
          {state.message}
        </EmptyState>
      </div>
    );
  return (
    <div className="termwrap">
      {state.kind === "starting" ? <div className="faint term-status" role="status">Opening a shell on the host…</div> : null}
      {state.kind === "exited" ? <div className="faint term-status" role="status">The shell exited{state.code != null ? ` with code ${state.code}` : ""}. <Button size="sm" variant="ghost" onClick={() => { setState({ kind: "starting" }); setAttempt((a) => a + 1); }}>Start a new one</Button></div> : null}
      <div ref={box} className="xterm-box" aria-label="Terminal" />
    </div>
  );
}
