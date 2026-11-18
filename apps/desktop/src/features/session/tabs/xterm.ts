/** Code-split: xterm.js and its fit addon load only when a terminal opens. */
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

export function createTerminal(el: HTMLElement, theme: { background: string; foreground: string; cursor: string }) {
  const term = new Terminal({ fontFamily: "'JetBrains Mono', ui-monospace, monospace", fontSize: 12.5, cursorBlink: true, scrollback: 5000, theme, allowProposedApi: false });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(el);
  fit.fit();
  return { term, fit };
}
