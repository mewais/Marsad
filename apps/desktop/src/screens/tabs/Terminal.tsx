/**
 * Terminal tab — REAL interactive pty escape-hatch terminals (WAVE 3b, v2).
 *
 * A raw shell running on the session's host (local, or the session's existing
 * SSH/WSL target) — a SEPARATE channel from claude's transcript (Activity tab).
 *
 * v2: MULTIPLE terminals per session, tabbed, each independently openable and
 * closable (＋ / ✕), plus per-terminal restart (↻ = kill + fresh shell, same
 * tab). Each terminal is keyed by a client-generated `terminal_id`; the backend
 * pty registry keys off it too, so one session can run many concurrent shells.
 *
 * LIVENESS: each terminal's xterm instance (DOM element, fit addon, pty listener)
 * is cached in a module-level map for the LIFETIME OF THE UI — once opened it
 * stays live across tab/session switches, only detaching its element on unmount
 * and reattaching on return. The pty listener stays subscribed so scrollback
 * keeps updating while hidden. (The server-side pty outlives even that — until
 * closed or the session stops.)
 *
 * Browser/mock mode (no Tauri): pty calls are no-ops (see bridge/api.ts), so this
 * renders honestly idle — no fake output.
 */
import { useEffect, useReducer, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { useVm } from "../../bridge/useVm";
import { listenPty, ptyClose, ptyInput, ptyResize, ptyStart } from "../../bridge/api";
import type { PtyEvent } from "../../bridge/types";

/** Decode a base64 `PtyOutput.data` payload into raw bytes (xterm's `write`
 *  accepts a Uint8Array and handles multi-byte UTF-8 split across writes). */
function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function newTerminalId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c && typeof c.randomUUID === "function") return `term-${c.randomUUID()}`;
  return `term-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e9).toString(36)}`;
}

// Force xterm's own layers transparent so the panel glass shows through. Setting
// only the theme background isn't enough — xterm.css paints the viewport/screen.
// Injected once.
let styleInjected = false;
function ensureTransparentStyle() {
  if (styleInjected || typeof document === "undefined") return;
  styleInjected = true;
  const s = document.createElement("style");
  s.textContent = `
    .xterm, .xterm .xterm-viewport, .xterm .xterm-screen { background-color: transparent !important; }
    .xterm-viewport::-webkit-scrollbar { width: 8px; }
    .xterm-viewport::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.14); border-radius: 4px; }
  `;
  document.head.appendChild(s);
}

// ---------------------------------------------------------------------------
// Module-level caches (survive component unmount/remount — see LIVENESS note).
// ---------------------------------------------------------------------------

interface TermEntry {
  term: Terminal;
  fit: FitAddon;
  el: HTMLDivElement;
  unlisten?: () => void;
  started: boolean;
  sessionId: string;
  terminalId: string;
}

/** All live terminals, keyed by terminal_id. */
const TERMINALS = new Map<string, TermEntry>();
/** Per-session terminal roster: ordered ids + which one is active. */
const SESSION_TERMS = new Map<string, { ids: string[]; active: string }>();

function roster(sessionId: string): { ids: string[]; active: string } {
  let s = SESSION_TERMS.get(sessionId);
  if (!s) {
    const id = newTerminalId();
    s = { ids: [id], active: id };
    SESSION_TERMS.set(sessionId, s);
  }
  return s;
}

function getOrCreate(sessionId: string, terminalId: string): TermEntry {
  const existing = TERMINALS.get(terminalId);
  if (existing) return existing;

  ensureTransparentStyle();

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
    // keyword "transparent", which xterm's parser rejects → opaque black) so the
    // panel glass shows through.
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

  // Ctrl+Shift+C copies the selection (xterm otherwise forwards it to the pty as
  // SIGINT-ish bytes). Ctrl+Shift+V paste already works via the browser default.
  term.attachCustomKeyEventHandler((e) => {
    if (e.type === "keydown" && e.ctrlKey && e.shiftKey && (e.key === "C" || e.key === "c")) {
      const sel = term.getSelection();
      if (sel) {
        void navigator.clipboard?.writeText(sel);
        return false; // handled — don't also send to the pty
      }
    }
    return true;
  });

  const entry: TermEntry = { term, fit, el, started: false, sessionId, terminalId };
  TERMINALS.set(terminalId, entry);

  // Keystrokes + resizes go to THIS terminal's pty. Registered once.
  term.onData((data) => void ptyInput(sessionId, terminalId, data));
  term.onResize(({ cols, rows }) => void ptyResize(sessionId, terminalId, cols, rows));

  // Subscribe once; route strictly by terminal_id.
  void listenPty((event: PtyEvent) => {
    if (event.terminal_id !== terminalId) return;
    const e = TERMINALS.get(terminalId);
    if (!e) return;
    if (event.kind === "pty_output") {
      e.term.write(base64ToBytes(event.data));
    } else if (event.kind === "pty_exit") {
      e.term.write(`\r\n\x1b[90m[process exited — code ${event.code}]\x1b[0m\r\n`);
    }
  }).then((fn) => {
    const e = TERMINALS.get(terminalId);
    if (e) e.unlisten = fn;
  });

  return entry;
}

function disposeTerminal(terminalId: string) {
  const e = TERMINALS.get(terminalId);
  if (!e) return;
  try {
    e.unlisten?.();
  } catch {
    /* ignore */
  }
  if (e.el.parentNode) e.el.parentNode.removeChild(e.el);
  e.term.dispose();
  TERMINALS.delete(terminalId);
}

// ---------------------------------------------------------------------------
// TerminalTab
// ---------------------------------------------------------------------------

export function TerminalTab({ sessionId }: { sessionId: string }) {
  const { selectedSession } = useVm();
  const containerRef = useRef<HTMLDivElement>(null);
  const [, bump] = useReducer((x: number) => x + 1, 0);

  // Ensure a roster exists (creates the first terminal id on first open).
  const sess = roster(sessionId);
  const activeId = sess.active;

  useEffect(() => {
    const container = containerRef.current;
    if (!container || !activeId) return;

    const entry = getOrCreate(sessionId, activeId);
    container.appendChild(entry.el);
    const firstOpen = !entry.term.element;
    if (firstOpen) entry.term.open(entry.el);

    const raf = requestAnimationFrame(() => {
      entry.fit.fit();
      if (!entry.started) {
        entry.started = true;
        void ptyStart(sessionId, activeId, entry.term.cols, entry.term.rows);
      }
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
      // Detach (keep alive in cache), don't dispose.
      if (entry.el.parentNode === container) container.removeChild(entry.el);
    };
    // `bump`-driven re-render is intentional in deps via activeId + roster ids.
  }, [sessionId, activeId]);

  if (!selectedSession) {
    return <div style={{ padding: 20, color: "var(--txt3)" }}>No session selected.</div>;
  }

  function selectTerminal(id: string) {
    const s = roster(sessionId);
    if (s.active !== id) {
      s.active = id;
      bump();
    }
  }

  function addTerminal() {
    const s = roster(sessionId);
    const id = newTerminalId();
    s.ids.push(id);
    s.active = id;
    bump();
  }

  function closeTerminal(id: string) {
    void ptyClose(sessionId, id);
    disposeTerminal(id);
    const s = roster(sessionId);
    s.ids = s.ids.filter((x) => x !== id);
    if (s.active === id) s.active = s.ids[s.ids.length - 1] ?? "";
    bump();
  }

  function restartTerminal(id: string) {
    // Kill the pty + drop the xterm; a fresh one (same tab id) is recreated on
    // the next mount, which fires a new PtyStart → clean shell.
    void ptyClose(sessionId, id);
    disposeTerminal(id);
    bump();
  }

  const tabLabel = (i: number) => `sh ${i + 1}`;

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
      {/* Tab strip */}
      <div
        data-testid="terminal-tabs"
        style={{
          display: "flex",
          alignItems: "center",
          gap: 4,
          padding: "6px 8px",
          borderBottom: "1px solid var(--border)",
          flexShrink: 0,
        }}
      >
        {sess.ids.map((id, i) => {
          const isActive = id === activeId;
          return (
            <div
              key={id}
              onClick={() => selectTerminal(id)}
              data-testid={`terminal-tab-${i}`}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                padding: "4px 9px",
                borderRadius: 7,
                cursor: "pointer",
                fontSize: "calc(12.5px * var(--fs,1))",
                fontFamily: "'JetBrains Mono',monospace",
                color: isActive ? "var(--txt)" : "var(--txt3)",
                background: isActive ? "var(--panel2)" : "transparent",
                border: isActive ? "1px solid var(--border2)" : "1px solid transparent",
              }}
            >
              <span>{tabLabel(i)}</span>
              <span
                title="Restart shell"
                onClick={(e) => {
                  e.stopPropagation();
                  restartTerminal(id);
                }}
                style={{ opacity: 0.6, cursor: "pointer", fontSize: "11px" }}
              >
                ↻
              </span>
              <span
                title="Close terminal"
                onClick={(e) => {
                  e.stopPropagation();
                  closeTerminal(id);
                }}
                style={{ opacity: 0.6, cursor: "pointer", fontSize: "13px", lineHeight: 1 }}
              >
                ✕
              </span>
            </div>
          );
        })}
        <button
          onClick={addTerminal}
          title="New terminal"
          data-testid="terminal-add"
          style={{
            border: "1px solid var(--border2)",
            background: "var(--panel2)",
            color: "var(--txt2)",
            borderRadius: 7,
            width: 24,
            height: 24,
            cursor: "pointer",
            fontSize: "15px",
            lineHeight: 1,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          ＋
        </button>
      </div>

      {/* Active terminal mount point, or an empty prompt when all are closed. */}
      {activeId ? (
        <div
          ref={containerRef}
          data-testid="terminal-output"
          style={{ flex: 1, overflow: "hidden", padding: "8px 12px" }}
        />
      ) : (
        <div
          style={{
            flex: 1,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "var(--txt3)",
            fontSize: "calc(14px * var(--fs,1))",
          }}
        >
          No terminals open —{" "}
          <button
            onClick={addTerminal}
            style={{
              marginLeft: 6,
              border: "1px solid var(--border2)",
              background: "var(--panel2)",
              color: "var(--txt)",
              borderRadius: 7,
              padding: "4px 10px",
              cursor: "pointer",
              fontFamily: "inherit",
            }}
          >
            ＋ open one
          </button>
        </div>
      )}
    </div>
  );
}
