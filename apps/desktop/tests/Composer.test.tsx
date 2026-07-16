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

describe("Composer hard-block when disconnected/paused", () => {
  it("disables the textarea + send, shows the reason, and never sends or clears the draft", () => {
    const onSend = vi.fn();
    render(
      <Composer
        sessionId="sess-block"
        onSend={onSend}
        onInterrupt={vi.fn()}
        disabled
        disabledReason="Reconnecting… your text is kept"
      />,
    );
    // Placeholder reflects the reason; textarea is disabled.
    const ta = screen.getByPlaceholderText(
      "Reconnecting… your text is kept",
    ) as HTMLTextAreaElement;
    expect(ta.disabled).toBe(true);

    // Even if a submit is forced (Enter), nothing is sent while blocked.
    fireEvent.keyDown(ta, { key: "Enter" });
    expect(onSend).not.toHaveBeenCalled();
  });

  it("keeps the typed draft intact across a disable (text is never lost)", () => {
    const { rerender } = render(
      <Composer sessionId="sess-keep" onSend={vi.fn()} onInterrupt={vi.fn()} />,
    );
    const enabled = () =>
      screen.getByPlaceholderText(/Send a prompt/i) as HTMLTextAreaElement;
    fireEvent.change(enabled(), { target: { value: "important prompt" } });

    // Connection drops → composer becomes disabled. The draft must survive.
    rerender(
      <Composer
        sessionId="sess-keep"
        onSend={vi.fn()}
        onInterrupt={vi.fn()}
        disabled
        disabledReason="Session paused — resume it to send"
      />,
    );
    // Re-enable (reconnected) → the text is still there to send.
    rerender(
      <Composer sessionId="sess-keep" onSend={vi.fn()} onInterrupt={vi.fn()} />,
    );
    expect(enabled().value).toBe("important prompt");
  });
});
