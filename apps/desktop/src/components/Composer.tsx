import React, { useRef, useState } from "react";
import type { SlashEntry } from "../bridge/types";
import { filterSlash } from "../bridge/api";
import { SlashPopover } from "./SlashPopover";
import { Escape } from "./Icon";

interface ComposerProps {
  onSend: (text: string) => void;
  onInterrupt: () => void;
  onSlashSelect?: (entry: SlashEntry) => void;
  disabled?: boolean;
}

/** Composer textarea + slash button + send button, matching .dc.html layout. */
export function Composer({
  onSend,
  onInterrupt,
  onSlashSelect,
  disabled,
}: ComposerProps) {
  const [text, setText] = useState("");
  const [slashOpen, setSlashOpen] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const slashEntries = slashOpen ? filterSlash(text) : [];

  function handleKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
    if (e.key === "Escape") {
      setSlashOpen(false);
    }
  }

  function handleInput(e: React.ChangeEvent<HTMLTextAreaElement>) {
    const val = e.target.value;
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

  return (
    <div style={{ position: "relative" }}>
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
            fontSize: 15,
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
          placeholder="Send a prompt, or / for commands…  (⏎ to send, ⇧⏎ newline)"
          style={{
            flex: 1,
            background: "none",
            border: "none",
            outline: "none",
            color: "var(--txt)",
            fontFamily: "inherit",
            fontSize: 13,
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
              fontSize: 12,
            }}
          >
            <Escape size={14} />
          </button>
          <button
            onClick={submit}
            disabled={disabled}
            style={{
              fontFamily: "inherit",
              fontWeight: 700,
              fontSize: 12.5,
              padding: "8px 16px",
              borderRadius: 8,
              border: "none",
              background: "var(--accent)",
              color: "#fff",
              cursor: "pointer",
            }}
          >
            Send ↵
          </button>
        </div>
      </div>
    </div>
  );
}
