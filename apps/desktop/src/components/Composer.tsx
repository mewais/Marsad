import React, { useEffect, useRef, useState } from "react";
import type { SlashCommandInfo, SlashEntry } from "../bridge/types";
import { filterSlash } from "../bridge/api";
import { SlashPopover } from "./SlashPopover";

// Per-session draft store, MODULE-level (not component state) so a draft survives
// the Composer unmounting — which happens when the user switches tabs within a
// session (ActivityTab is conditionally rendered) as well as between sessions.
// Keyed by session id; cleared on send.
const composerDrafts = new Map<string, string>();

interface ComposerProps {
  /** The session this composer is editing. Drafts are kept PER session so an
   *  in-progress prompt typed in one session does not bleed into another (the
   *  Composer instance is reused across session switches, so a single shared
   *  `text` state would leak). */
  sessionId: string;
  onSend: (text: string) => void;
  onInterrupt: () => void;
  onSlashSelect?: (entry: SlashEntry) => void;
  disabled?: boolean;
  /** Previously sent prompts for this session, oldest first. Used for Up/Down history navigation. */
  history?: string[];
  /** Live slash commands from the current session's system/init event — the
   *  popover's ONLY source. Empty/absent (pre-init, or a driver that reports
   *  none) renders an honest "waiting for the session to report commands"
   *  hint, never a preset fallback (dynamic-capabilities.md §1.3). */
  slashCommands?: SlashCommandInfo[];
}

/** Composer textarea + slash button + send button, matching .dc.html layout. */
export function Composer({
  sessionId,
  onSend,
  onInterrupt,
  onSlashSelect,
  disabled,
  history = [],
  slashCommands,
}: ComposerProps) {
  // Initialize from the per-session draft store so a draft survives BOTH a
  // session switch AND a tab switch (the latter unmounts ActivityTab, so the
  // store must live OUTSIDE the component — see module-level `composerDrafts`).
  const [text, setText] = useState<string>(() => composerDrafts.get(sessionId) ?? "");
  const [slashOpen, setSlashOpen] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // History navigation state.
  // historyIndex: null = not navigating (live draft), or index into `history` (0 = oldest).
  // draft: the text the user had typed before entering history navigation.
  const historyIndexRef = useRef<number | null>(null);
  const draftRef = useRef<string>("");

  const prevSessionIdRef = useRef<string>(sessionId);
  const textRef = useRef<string>("");
  textRef.current = text;

  // Swap drafts when the session changes IN PLACE (Composer stays mounted).
  useEffect(() => {
    const prev = prevSessionIdRef.current;
    if (prev === sessionId) return;
    composerDrafts.set(prev, textRef.current); // save outgoing
    const incoming = composerDrafts.get(sessionId) ?? "";
    prevSessionIdRef.current = sessionId;
    historyIndexRef.current = null;
    draftRef.current = incoming;
    setText(incoming);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // The slash popover's ONLY source is the session's live slash_commands
  // (dynamic-capabilities.md §1.3) — no preset catalog fallback. An
  // empty/absent list is an honest empty state with its own hint below.
  const slashEntries = slashOpen ? filterSlash(text, slashCommands) : [];
  const hasSessionCommands = Boolean(slashCommands && slashCommands.length > 0);

  /** Returns true if the textarea caret is on the first line. */
  function caretOnFirstLine(): boolean {
    const el = textareaRef.current;
    if (!el) return true;
    const pos = el.selectionStart;
    // First line has no newline before the caret position
    return !el.value.slice(0, pos).includes("\n");
  }

  /** Returns true if the textarea caret is on the last line. */
  function caretOnLastLine(): boolean {
    const el = textareaRef.current;
    if (!el) return true;
    const pos = el.selectionEnd;
    // Last line has no newline after the caret position
    return !el.value.slice(pos).includes("\n");
  }

  /** Load a history entry or draft into the textarea and resize it. */
  function loadText(val: string) {
    setText(val);
    setSlashOpen(val.startsWith("/") && val.length > 0);
    const el = textareaRef.current;
    if (el) {
      el.style.height = "auto";
      el.style.height = Math.min(el.scrollHeight, 120) + "px";
    }
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      // Esc dismisses the slash popover if it's open; otherwise it interrupts
      // claude's current turn (same as the Interrupt button).
      if (slashOpen) {
        setSlashOpen(false);
      } else {
        onInterrupt();
      }
      return;
    }

    if (e.key === "ArrowUp" && caretOnFirstLine()) {
      // Only navigate if we have history entries to go back to
      if (history.length === 0) return;
      e.preventDefault();

      const currentIdx = historyIndexRef.current;
      if (currentIdx === null) {
        // Just entering history navigation — save the current draft
        draftRef.current = text;
        const newIdx = history.length - 1;
        historyIndexRef.current = newIdx;
        loadText(history[newIdx]);
      } else if (currentIdx > 0) {
        // Go further back
        const newIdx = currentIdx - 1;
        historyIndexRef.current = newIdx;
        loadText(history[newIdx]);
      }
      // If already at the oldest entry, do nothing further
      return;
    }

    if (e.key === "ArrowDown" && caretOnLastLine()) {
      const currentIdx = historyIndexRef.current;
      if (currentIdx === null) return; // not in history navigation, let default behavior run
      e.preventDefault();

      if (currentIdx < history.length - 1) {
        // Move forward in history
        const newIdx = currentIdx + 1;
        historyIndexRef.current = newIdx;
        loadText(history[newIdx]);
      } else {
        // Past the newest entry — restore the in-progress draft
        historyIndexRef.current = null;
        loadText(draftRef.current);
      }
    }
  }

  function handleInput(e: React.ChangeEvent<HTMLTextAreaElement>) {
    const val = e.target.value;
    // Any manual edit resets history navigation and updates the draft
    historyIndexRef.current = null;
    draftRef.current = val;
    composerDrafts.set(sessionId, val); // persist so it survives tab/session switch
    setText(val);
    setSlashOpen(val.startsWith("/") && val.length > 0);
    // Auto-resize textarea
    const el = textareaRef.current;
    if (el) {
      el.style.height = "auto";
      el.style.height = Math.min(el.scrollHeight, 120) + "px";
    }
  }

  function submit() {
    const t = text.trim();
    if (!t) return;
    // Reset history navigation on send
    historyIndexRef.current = null;
    draftRef.current = "";
    composerDrafts.delete(sessionId); // sent → clear the saved draft
    onSend(t);
    setText("");
    setSlashOpen(false);
    if (textareaRef.current) textareaRef.current.style.height = "auto";
  }

  function handleSlashSelect(entry: SlashEntry) {
    setSlashOpen(false);
    setText("");
    onSlashSelect?.(entry);
  }

  // `!`-prefixed input runs as a shell command on the session's host (handled by
  // Marsad, not sent to claude) — surface that mode visually, like `/` commands.
  const bangMode = text.trimStart().startsWith("!");

  return (
    <div style={{ position: "relative" }}>
      {bangMode && !slashOpen && (
        <div
          style={{
            position: "absolute",
            bottom: "calc(100% + 8px)",
            left: 0,
            right: 0,
            zIndex: 5,
            display: "flex",
            alignItems: "center",
            gap: 8,
            padding: "8px 12px",
            background: "var(--panel)",
            border: "1px solid var(--border2)",
            borderLeft: "3px solid var(--done)",
            borderRadius: 9,
            fontSize: "calc(13px * var(--fs,1))",
            color: "var(--txt2)",
            boxShadow: "var(--shadow-sm)",
          }}
          data-testid="composer-bang-hint"
        >
          <span style={{ color: "var(--done)", fontWeight: 800 }}>!</span>
          <span>
            Shell command — runs on this session&apos;s host (not sent to Claude)
          </span>
        </div>
      )}
      {slashOpen && slashEntries.length >= 0 && (
        <div
          style={{
            position: "absolute",
            bottom: "calc(100% + 8px)",
            left: 0,
            right: 0,
            zIndex: 5,
          }}
        >
          <SlashPopover
            entries={slashEntries}
            onSelect={handleSlashSelect}
            empty={slashEntries.length === 0}
            emptyMessage={
              hasSessionCommands
                ? "No matching command"
                : "Waiting for the session to report commands…"
            }
          />
        </div>
      )}

      <div
        style={{
          display: "flex",
          gap: 10,
          alignItems: "flex-end",
          background: "var(--panel2)",
          border: "1px solid var(--border2)",
          borderRadius: 13,
          padding: "10px 12px",
        }}
      >
        {/* Slash toggle button */}
        <button
          onClick={() => setSlashOpen((o) => !o)}
          title="Slash commands"
          style={{
            width: 32,
            height: 32,
            borderRadius: 8,
            border: "1px solid var(--border2)",
            background: "var(--panel)",
            color: slashOpen ? "var(--accent2)" : "var(--txt2)",
            cursor: "pointer",
            fontSize: "calc(18px * var(--fs,1))",
            fontWeight: 700,
            flexShrink: 0,
            lineHeight: 1,
          }}
        >
          /
        </button>

        {/* Textarea */}
        <textarea
          ref={textareaRef}
          rows={1}
          value={text}
          onChange={handleInput}
          onKeyDown={handleKeyDown}
          disabled={disabled}
          placeholder="Send a prompt · / commands · ! shell  (⏎ send, ⇧⏎ newline)"
          style={{
            flex: 1,
            background: "none",
            border: "none",
            outline: "none",
            color: "var(--txt)",
            fontFamily: "inherit",
            fontSize: "calc(15.5px * var(--fs,1))",
            resize: "none",
            lineHeight: 1.5,
            maxHeight: 120,
          }}
        />

        {/* Action buttons */}
        <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
          <button
            onClick={onInterrupt}
            title="Interrupt"
            style={{
              width: 32,
              height: 32,
              borderRadius: 8,
              border: "1px solid var(--border2)",
              background: "var(--panel)",
              color: "var(--txt2)",
              cursor: "pointer",
              fontSize: "calc(14.5px * var(--fs,1))",
            }}
          >
            ⎋
          </button>
          <button
            onClick={submit}
            disabled={disabled}
            style={{
              fontFamily: "inherit",
              fontWeight: 700,
              fontSize: "calc(15px * var(--fs,1))",
              padding: "8px 16px",
              borderRadius: 8,
              border: "none",
              background: bangMode ? "var(--done)" : "var(--accent)",
              color: bangMode ? "#0e1116" : "#fff",
              cursor: "pointer",
            }}
          >
            {bangMode ? "Run ↵" : "Send ↵"}
          </button>
        </div>
      </div>
    </div>
  );
}
