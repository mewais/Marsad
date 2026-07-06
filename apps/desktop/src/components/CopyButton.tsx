/**
 * CopyButton — a small GitHub/Atlassian-style "copy to clipboard" affordance
 * for code/command blocks (the two-overlapping-squares icon). Hover-reveals via
 * the `.copy-wrap`/`.copy-btn` CSS in index.html; flips to a check for ~1.4s
 * after a successful copy. Place inside a `position: relative` (`.copy-wrap`)
 * container; it pins to the block's top-right.
 */
import { useState, type CSSProperties } from "react";

function CopyIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="9" y="9" width="11" height="11" rx="2" stroke="currentColor" strokeWidth="2" />
      <path
        d="M5 15V5a2 2 0 0 1 2-2h8"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M5 13l4 4L19 7"
        stroke="currentColor"
        strokeWidth="2.2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

async function writeClipboard(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    // Fallback for webviews without the async clipboard API.
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try {
      document.execCommand("copy");
    } catch {
      /* best-effort; nothing more we can do */
    }
    document.body.removeChild(ta);
  }
}

export function CopyButton({ text, style }: { text: string; style?: CSSProperties }) {
  const [copied, setCopied] = useState(false);

  async function handleCopy(e: React.MouseEvent) {
    // Don't let the click bubble to a card's onClick (e.g. opening a session).
    e.stopPropagation();
    await writeClipboard(text);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
  }

  return (
    <button
      type="button"
      className="copy-btn"
      onClick={handleCopy}
      title={copied ? "Copied" : "Copy"}
      aria-label="Copy to clipboard"
      style={{
        position: "absolute",
        top: 6,
        right: 6,
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        width: 26,
        height: 26,
        padding: 0,
        borderRadius: 6,
        border: "1px solid var(--border)",
        background: "var(--panel)",
        color: copied ? "var(--done, #46c08a)" : "var(--txt2)",
        cursor: "pointer",
        ...style,
      }}
    >
      {copied ? <CheckIcon /> : <CopyIcon />}
    </button>
  );
}
