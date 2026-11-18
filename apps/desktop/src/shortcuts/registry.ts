import { createStore } from "../lib/store";

export interface Shortcut {
  id: string;
  /** Key chords: "j", "mod+k", "shift+n", "?", "enter"; two entries = sequence ("g", "w"). */
  keys: string[];
  title: string;
  group: string;
  run: (e: KeyboardEvent) => void;
  /** Return false to let the key through (e.g. no row selected). */
  when?: () => boolean;
  /** Also fire while typing in an input / textarea. */
  allowInInput?: boolean;
  /** Higher wins when two shortcuts share a key (an open gate beats list navigation). */
  priority?: number;
}

export const shortcutStore = createStore<Shortcut[]>([]);

export function registerShortcut(s: Shortcut): () => void {
  shortcutStore.set((l) => [...l.filter((x) => x.id !== s.id), s]);
  return () => shortcutStore.set((l) => l.filter((x) => x !== s));
}

/** "mod+k", "shift+n", "enter", "?" ... from a keyboard event. */
export function chordOf(e: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">): string {
  let k = e.key;
  if (k === " ") k = "space";
  k = k.length === 1 ? k.toLowerCase() : k.toLowerCase();
  const isLetter = /^[a-z]$/.test(k);
  const parts: string[] = [];
  if (e.ctrlKey || e.metaKey) parts.push("mod");
  if (e.altKey) parts.push("alt");
  if (e.shiftKey && (isLetter || k.length > 1)) parts.push("shift");
  parts.push(k);
  return parts.join("+");
}

const isTyping = (t: EventTarget | null) => {
  const el = t as HTMLElement | null;
  if (!el || !el.tagName) return false;
  return el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable;
};

/** One global listener; supports two-key sequences ("g" then "w"). Skips while a modal is open. */
export function installShortcuts(target: Document = document): () => void {
  let pending: { key: string; at: number } | null = null;
  const onKey = (e: KeyboardEvent) => {
    if (e.defaultPrevented || e.isComposing) return;
    const chord = chordOf(e);
    if (["mod", "shift", "alt", "control", "meta"].includes(chord)) return;
    const modal = !!target.querySelector('[aria-modal="true"]');
    const typing = isTyping(e.target);
    const list = [...shortcutStore.get()].sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0));
    const seqKey = pending && Date.now() - pending.at < 900 ? pending.key : null;
    pending = null;
    for (const s of list) {
      const matchSeq = s.keys.length === 2 && seqKey === s.keys[0] && s.keys[1] === chord;
      const matchOne = s.keys.length === 1 && s.keys[0] === chord;
      if (!matchSeq && !matchOne) continue;
      if (modal && !chord.startsWith("mod+")) continue;
      if (typing && !s.allowInInput && !chord.startsWith("mod+")) continue;
      if (s.when && !s.when()) continue;
      e.preventDefault();
      s.run(e);
      return;
    }
    if (!typing && !modal && list.some((s) => s.keys.length === 2 && s.keys[0] === chord)) pending = { key: chord, at: Date.now() };
  };
  target.addEventListener("keydown", onKey);
  return () => target.removeEventListener("keydown", onKey);
}

export const prettyKey = (k: string): string =>
  k.split("+").map((p) => (p === "mod" ? "Ctrl" : p === "shift" ? "⇧" : p === "enter" ? "Enter" : p === "escape" ? "Esc" : p.length === 1 ? p.toUpperCase() : p)).join(" ");
