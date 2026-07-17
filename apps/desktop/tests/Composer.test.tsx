/**
 * Composer — per-session draft isolation.
 *
 * Regression: the Composer instance is reused across session switches, so a
 * single shared `text` state leaked an in-progress prompt from one session into
 * another. Drafts must be kept per session: isolated on switch, restored on return.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Composer } from "../src/components/Composer";

function renderComposer(sessionId: string) {
  return render(
    <Composer sessionId={sessionId} onSend={vi.fn()} onInterrupt={vi.fn()} />,
  );
}

describe("Composer per-session drafts", () => {
  it("isolates a draft from other sessions and restores it on return", () => {
    const { rerender } = renderComposer("sess-a");
    const ta = () =>
      screen.getByPlaceholderText(/Send a prompt/i) as HTMLTextAreaElement;

    // Type a draft in session A.
    fireEvent.change(ta(), { target: { value: "partial prompt for A" } });
    expect(ta().value).toBe("partial prompt for A");

    // Switch to session B — the textarea must NOT carry A's draft.
    rerender(
      <Composer sessionId="sess-b" onSend={vi.fn()} onInterrupt={vi.fn()} />,
    );
    expect(ta().value).toBe("");

    // Type something in B, then switch back to A — A's draft is restored.
    fireEvent.change(ta(), { target: { value: "B text" } });
    rerender(
      <Composer sessionId="sess-a" onSend={vi.fn()} onInterrupt={vi.fn()} />,
    );
    expect(ta().value).toBe("partial prompt for A");

    // And B's draft is independently preserved.
    rerender(
      <Composer sessionId="sess-b" onSend={vi.fn()} onInterrupt={vi.fn()} />,
    );
    expect(ta().value).toBe("B text");
  });
});

describe("Composer sendDisabled — textarea stays editable, only Send is blocked", () => {
  it("keeps the textarea EDITABLE, disables the Send button, and doesn't send on Enter", () => {
    const onSend = vi.fn();
    render(
      <Composer
        sessionId="sess-block"
        onSend={onSend}
        onInterrupt={vi.fn()}
        sendDisabled
        sendDisabledReason="Reconnecting…"
      />,
    );
    // Textarea uses the NORMAL placeholder and is NOT disabled — the user can type.
    const ta = screen.getByPlaceholderText(/Send a prompt/i) as HTMLTextAreaElement;
    expect(ta.disabled).toBe(false);
    fireEvent.change(ta, { target: { value: "typed while reconnecting" } });
    expect(ta.value).toBe("typed while reconnecting");

    // The Send button is disabled.
    const send = screen.getByTestId("composer-send") as HTMLButtonElement;
    expect(send.disabled).toBe(true);

    // Enter must NOT send while blocked (and must not clear the draft).
    fireEvent.keyDown(ta, { key: "Enter" });
    expect(onSend).not.toHaveBeenCalled();
    expect(ta.value).toBe("typed while reconnecting");

    // The reason hint is shown.
    expect(screen.getByTestId("composer-send-hint").textContent).toContain(
      "Reconnecting…",
    );
  });

  it("keeps the typed draft intact across a send-disable (text is never lost)", () => {
    const { rerender } = render(
      <Composer sessionId="sess-keep" onSend={vi.fn()} onInterrupt={vi.fn()} />,
    );
    const ta = () =>
      screen.getByPlaceholderText(/Send a prompt/i) as HTMLTextAreaElement;
    fireEvent.change(ta(), { target: { value: "important prompt" } });

    // Connection drops → sending disabled. The draft must survive AND stay editable.
    rerender(
      <Composer
        sessionId="sess-keep"
        onSend={vi.fn()}
        onInterrupt={vi.fn()}
        sendDisabled
        sendDisabledReason="Session paused — resume it to send"
      />,
    );
    expect(ta().value).toBe("important prompt");
    expect(ta().disabled).toBe(false);

    // Re-enable (reconnected) → the text is still there to send.
    rerender(
      <Composer sessionId="sess-keep" onSend={vi.fn()} onInterrupt={vi.fn()} />,
    );
    expect(ta().value).toBe("important prompt");
  });
});
