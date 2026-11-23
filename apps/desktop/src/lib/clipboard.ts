/**
 * Put `text` on the system clipboard. Resolves only once the write has succeeded; rejects with a
 * readable Error otherwise, so callers never claim a copy that did not happen.
 *
 * The async Clipboard API is a webview feature (no Tauri plugin involved) but a webview may refuse
 * it (no secure context, permission denied, no focus): then a hidden textarea and the legacy
 * `copy` command, which works from a user gesture in all three webviews, is tried.
 */
export async function copyText(text: string): Promise<void> {
  let first: unknown;
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch (e) {
      first = e;
    }
  }
  if (legacyCopy(text)) return;
  const why = first instanceof Error ? first.message : first ? String(first) : "the clipboard is not available here";
  throw new Error(`Could not copy: ${why}`);
}

function legacyCopy(text: string): boolean {
  if (typeof document === "undefined" || typeof document.execCommand !== "function") return false;
  const holder = document.activeElement as HTMLElement | null;
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.setAttribute("aria-hidden", "true");
  ta.style.cssText = "position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;pointer-events:none";
  document.body.appendChild(ta);
  try {
    ta.select();
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    ta.remove();
    holder?.focus?.();
  }
}
