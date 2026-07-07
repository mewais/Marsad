/**
 * Terminal tab — REAL interactive pty escape-hatch terminal (WAVE 3b).
 *
 * A raw shell running on the session's host (local, or the session's
 * existing SSH/WSL target) — a SEPARATE channel from claude's own
 * transcript (Activity tab): no parsing/rendering of claude's stream-json
 * output here, just a plain interactive shell, rendered with xterm.js.
 *
 * LIVENESS: each session's xterm instance (its DOM element, fit addon and pty
 * event listener) is cached in a module-level map for the LIFETIME OF THE UI —
 * once a terminal is opened it stays live across tab switches, session switches
 * and board trips, only detaching its DOM from the container on unmount and
 * reattaching it on return. The pty listener stays subscribed the whole time so
 * scrollback keeps updating even while the tab is hidden. (The server-side pty
 * itself outlives even that — until the session stops.) Nothing here persists
 * across a UI reload, which is the intended scope.
 *
 * Browser/mock mode (no Tauri backend): `ptyStart`/`ptyInput`/`ptyResize`/
 * `listenPty` are all no-ops (see bridge/api.ts), so this renders an
 * honestly IDLE terminal — no fake/synthetic output.
 */
import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { useVm } from "../../bridge/useVm";
import { listenPty, ptyInput, ptyResize, ptyStart } from "../../bridge/api";
import type { PtyEvent } from "../../bridge/types";

/** Decode a base64 `PtyOutput.data` payload into raw bytes. `xterm.js`'s
 *  `Terminal.write` accepts a `Uint8Array` directly and handles multi-byte
 *  UTF-8 sequences split across writes correctly (its own parser is
 *  byte-oriented) — this is exactly why the wire carries base64 rather than
 *  plain text (see `marsad_proto::IpcMsg::PtyOutput`'s doc comment). */
function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

// ---------------------------------------------------------------------------
// Module-level live-terminal cache (keyed by session id). Survives component
// unmount/remount — see the LIVENESS note in the file header.
// ---------------------------------------------------------------------------

interface TermEntry {
  term: Terminal;
  fit: FitAddon;
  /** The element xterm was `open()`-ed into; moved between containers on
   *  reattach, never destroyed while cached. */
  el: HTMLDivElement;
  unlisten?: () => void;
  /** True once `ptyStart` has been sent (only fired once per session). */
  started: boolean;
}

const TERMINALS = new Map<string, TermEntry>();

function getOrCreate(sessionId: string): TermEntry {
  const existing = TERMINALS.get(sessionId);
  if (existing) return existing;

  const el = document.createElement("div");
  el.style.width = "100%";
  el.style.height = "100%";

  const term = new Terminal({
    convertEol: true,
    fontFamily: "'JetBrains Mono','Marsad Symbols',monospace",
    fontSize: 13,
    lineHeight: 1.4,
    cursorBlink: true,
    // allowTransparency + a fully-transparent rgba background (NOT the CSS
    // keyword "transparent", which xterm's color parser rejects and silently
    // treats as opaque black) so the --panel-dark glass shows through.
    allowTransparency: true,
    theme: {
      background: "rgba(0,0,0,0)",
      foreground: "#e8ecf1",
      cursor: "#e8ecf1",
      selectionBackground: "rgba(91,141,239,0.25)",
    },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);

  const entry: TermEntry = { term, fit, el, started: false };
  TERMINALS.set(sessionId, entry);

  // Keystrokes + resizes go to the pty. Registered ONCE (they persist with the
  // cached term across mounts).
  term.onData((data) => void ptyInput(sessionId, data));
  term.onResize(({ cols, rows }) => void ptyResize(sessionId, cols, rows));

  // Subscribe ONCE; the listener writes into the cached term (which keeps its
  // scrollback even while its element is detached from the DOM).
  void listenPty((event: PtyEvent) => {
    if (event.session_id !== sessionId) return;
    const e = TERMINALS.get(sessionId);
    if (!e) return;
    if (event.kind === "pty_output") {
      e.term.write(base64ToBytes(event.data));
    } else if (event.kind === "pty_exit") {
      e.term.write(`\r\n\x1b[90m[process exited — code ${event.code}]\x1b[0m\r\n`);
    }
  }).then((fn) => {
    const e = TERMINALS.get(sessionId);
    if (e) e.unlisten = fn;
  });

  return entry;
}

// ---------------------------------------------------------------------------
// TerminalTab
// ---------------------------------------------------------------------------

interface TerminalTabProps {
  sessionId: string;
}

export function TerminalTab({ sessionId }: TerminalTabProps) {
  const { selectedSession } = useVm();
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    // Create-or-reuse this session's cached terminal, and (re)attach its element
    // into our container. `term.open()` must run while the element is in the DOM
    // so xterm can measure it, so open() happens here on first creation.
    const entry = getOrCreate(sessionId);
    container.appendChild(entry.el);
    const firstOpen = !entry.term.element;
    if (firstOpen) {
      entry.term.open(entry.el);
    }

    // Post-layout fit: flex containers report 0 height synchronously on mount,
    // so defer the fit (and the one-time ptyStart) to after the first paint.
    const raf = requestAnimationFrame(() => {
      entry.fit.fit();
      if (!entry.started) {
        entry.started = true;
        void ptyStart(sessionId, entry.term.cols, entry.term.rows);
      }
      // Reattached-from-cache: repaint the buffered scrollback into the freshly
      // re-inserted element.
      if (!firstOpen) entry.term.refresh(0, entry.term.rows - 1);
      entry.term.focus();
    });

    const onWinResize = () => entry.fit.fit();
    window.addEventListener("resize", onWinResize);
    const resizeObserver = new ResizeObserver(() => entry.fit.fit());
    resizeObserver.observe(container);

    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", onWinResize);
      resizeObserver.disconnect();
      // DETACH, don't destroy: keep the term/listener/scrollback alive in the
      // cache so returning to this session's terminal shows its full history.
      if (entry.el.parentNode === container) {
        container.removeChild(entry.el);
      }
    };
  }, [sessionId]);

  if (!selectedSession) {
    return (
      <div style={{ padding: 20, color: "var(--txt3)" }}>
        No session selected.
      </div>
    );
  }

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        background: "var(--panel-dark)",
        backdropFilter: "blur(22px)",
      }}
    >
      {/* xterm.js mount point — the cached terminal's element is appended here on
          mount and removed (kept alive) on unmount. */}
      <div
        ref={containerRef}
        data-testid="terminal-output"
        style={{
          flex: 1,
          overflow: "hidden",
          padding: "8px 12px",
          background: "var(--panel-dark)",
        }}
      />
    </div>
  );
}
