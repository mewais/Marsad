/**
 * Terminal tab — REAL interactive pty escape-hatch terminal (WAVE 3b).
 *
 * A raw shell running on the session's host (local, or the session's
 * existing SSH/WSL target) — a SEPARATE channel from claude's own
 * transcript (Activity tab): no parsing/rendering of claude's stream-json
 * output here, just a plain interactive shell, rendered with xterm.js.
 *
 * Browser/mock mode (no Tauri backend): `ptyStart`/`ptyInput`/`ptyResize`/
 * `listenPty` are all no-ops (see bridge/api.ts), so this renders an
 * honestly IDLE terminal — no fake/synthetic output.
 *
 * BUILT-NOT-VERIFIED (WAVE 3b): wired against the real `pty_start`/
 * `pty_input`/`pty_resize` Tauri commands and the `"pty"` event channel, but
 * — like the rest of the desktop app's real (non-mock) path — never
 * actually run against a compiled `src-tauri` here (no webkit2gtk in this
 * sandbox). See docs/GAPS.md. The mock-mode path (this file's only
 * observable behavior in this sandbox) is exercised by `pnpm build`/`tsc`.
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
// TerminalTab
// ---------------------------------------------------------------------------

interface TerminalTabProps {
  sessionId: string;
}

export function TerminalTab({ sessionId }: TerminalTabProps) {
  const { selectedSession } = useVm();
  const containerRef = useRef<HTMLDivElement>(null);

  // Mount xterm.js once per (sessionId) — the whole component unmounts when
  // the user switches away from the Terminal tab (SessionDetail.tsx renders
  // it conditionally), so this effect's cleanup is the only pty-session
  // teardown on this side; the pty itself keeps running server-side until
  // the SESSION stops (see bridge/api.ts's WAVE 3b section header).
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      convertEol: true,
      fontFamily: "'JetBrains Mono','Marsad Symbols',monospace",
      fontSize: 13,
      lineHeight: 1.4,
      cursorBlink: true,
      // allowTransparency lets the xterm canvas be transparent so the --panel-dark
      // glass background on the wrapping div shows through (BUG 2).
      allowTransparency: true,
      theme: {
        // xterm's color parser does NOT accept the CSS keyword "transparent"
        // (it silently falls back to opaque black); a fully-transparent rgba is
        // required so the --panel-dark glass on the wrapping div shows through.
        background: "rgba(0,0,0,0)",
        foreground: "#e8ecf1",
        cursor: "#e8ecf1",
        selectionBackground: "rgba(91,141,239,0.25)",
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);

    // Guards against a listener/unlisten resolving after unmount (both
    // `listenPty` and the initial `fit()` are async-adjacent races).
    let disposed = false;
    let unlisten: (() => void) | undefined;

    // Defer ptyStart until after the post-layout RAF: flex containers measure
    // zero clientHeight synchronously on mount (before the browser has painted).
    // fit.fit() here sets cols/rows from those 0-height dims; ptyStart called
    // with cols=0/rows=0 would spawn a mis-sized pty. The RAF re-fit below runs
    // after the first paint, when the container has real dimensions — call
    // ptyStart only then so the initial pty size is correct.
    fit.fit();
    term.focus();

    // Post-layout re-fit + deferred ptyStart: one paint pass lets the flex
    // container measure its real height before we start the pty.
    const rafHandle = requestAnimationFrame(() => {
      if (!disposed) {
        fit.fit();
        void ptyStart(sessionId, term.cols, term.rows);
      }
    });

    void listenPty((event: PtyEvent) => {
      if (disposed || event.session_id !== sessionId) return;
      if (event.kind === "pty_output") {
        term.write(base64ToBytes(event.data));
      } else if (event.kind === "pty_exit") {
        term.write(`\r\n\x1b[90m[process exited — code ${event.code}]\x1b[0m\r\n`);
      }
    }).then((fn) => {
      if (disposed) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    const dataSub = term.onData((data) => {
      void ptyInput(sessionId, data);
    });
    const resizeSub = term.onResize(({ cols, rows }) => {
      void ptyResize(sessionId, cols, rows);
    });

    // Re-fit on both a window resize and a layout-only resize of the
    // terminal's own container (e.g. the inspector panel toggling, or the
    // tab bar wrapping at a narrow width) — a `ResizeObserver` on the
    // container catches the latter, which a `window` resize listener alone
    // would miss.
    const handleWindowResize = () => fit.fit();
    window.addEventListener("resize", handleWindowResize);
    const resizeObserver = new ResizeObserver(() => fit.fit());
    resizeObserver.observe(el);

    return () => {
      disposed = true;
      cancelAnimationFrame(rafHandle);
      window.removeEventListener("resize", handleWindowResize);
      resizeObserver.disconnect();
      dataSub.dispose();
      resizeSub.dispose();
      unlisten?.();
      term.dispose();
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
      {/* xterm.js mount point — clicking anywhere in it focuses the terminal
          natively (xterm.js's own textarea capture), no handler needed.
          background here shows through the xterm canvas (allowTransparency: true). */}
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
