import { linkView } from "../../src/state/hosts";
import { copyText } from "../../src/lib/clipboard";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  Button, Checkbox, CopyButton, deriveInitials, Dialog, Popover, pushEscape, DriverBadge, driverSlot, makeDriver, Menu, phaseToStatus, RadioGroup, Segmented, Select, StatusPill, Tabs, toast, ToastRegion, toastStore,
  VirtualList, ConnState, STATUS,
} from "../../src/ui";

describe("StatusPill", () => {
  it("shows a word and an icon for every status, never colour alone", () => {
    for (const k of Object.keys(STATUS) as (keyof typeof STATUS)[]) {
      const { container, unmount } = render(<StatusPill status={k} />);
      expect(container.textContent).toBe(STATUS[k].label);
      expect(container.querySelector("svg use")?.getAttribute("href")).toBe(`#i-${STATUS[k].icon}`);
      unmount();
    }
  });
  it("maps phases in one place", () => {
    expect(phaseToStatus("needs_you")).toBe("needs");
    expect(phaseToStatus("lost")).toBe("failed");
    expect(phaseToStatus("exited", true)).toBe("done");
    expect(phaseToStatus("idle")).toBe("idle");
    expect(phaseToStatus("starting")).toBe("working");
  });
});

describe("DriverBadge and host link", () => {
  it("derives initials from data, not a hardcoded list", () => {
    expect(deriveInitials("Claude Code")).toBe("CC");
    expect(deriveInitials("Codex")).toBe("CX");
    expect(deriveInitials("OpenCode")).toBe("OC");
    render(<DriverBadge driver={makeDriver("zed", "Zed Agent")} />);
    expect(screen.getByText("ZA")).toBeInTheDocument();
    expect(makeDriver("zed").slot).toBe(driverSlot("zed"));
    expect(new Set(["claude", "codex", "opencode", "kilo"].map(driverSlot)).size).toBe(4);
  });
  it("states the link in words", () => {
    const now = 1_000_000;
    const at = (ms: number) => ({ secs_since_epoch: Math.floor(ms / 1000), nanos_since_epoch: 0 });
    expect(linkView({ link: "remote", state: { state: "reconnecting", attempt: 2, next_at: at(now + 4000), last_error: "reset" } }, null, now).detail).toBe("Reconnecting in 4s");
    expect(linkView({ link: "remote", state: { state: "provisioning", reason: { kind: "command_not_found", detail: "" } } }).word).toBe("Installing helper");
    render(<ConnState host={{ link: { link: "remote", state: { state: "auth_needed", problem: "auth_needed", detail: "key rejected" } }, authPrompt: null }} />);
    expect(screen.getByText("Sign-in needed")).toBeInTheDocument();
  });
});

describe("Dialog", () => {
  function Harness({ onClose = () => {} }: { onClose?: () => void }) {
    const [open, setOpen] = useState(false);
    return (
      <>
        <button onClick={() => setOpen(true)}>opener</button>
        <Dialog open={open} onClose={() => { setOpen(false); onClose(); }} title="Hello" footer={<button>last</button>}>
          <input aria-label="first" />
        </Dialog>
      </>
    );
  }
  it("is a modal dialog with a name, traps focus, closes on Esc and restores focus", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Harness onClose={onClose} />);
    await user.click(screen.getByText("opener"));
    const dlg = screen.getByRole("dialog", { name: "Hello" });
    expect(dlg).toHaveAttribute("aria-modal", "true");
    const first = within(dlg).getByLabelText("first");
    expect(document.activeElement).toBe(within(dlg).getByRole("button", { name: "Close" }));
    await user.tab(); // first
    expect(document.activeElement).toBe(first);
    await user.tab(); // last
    await user.tab(); // wraps to Close
    expect(dlg.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).toBe(within(dlg).getByRole("button", { name: "Close" }));
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(screen.getByText("opener"));
  });
  it("scrolls inside itself (body is the overflow container, dialog capped to the viewport)", () => {
    render(<Dialog open onClose={() => {}} title="T">x</Dialog>);
    expect(screen.getByRole("dialog").className).toContain("dlg");
  });
});

describe("Tabs, Segmented, Menu, Select", () => {
  it("moves between tabs with arrow keys", async () => {
    const user = userEvent.setup();
    function T() {
      const [v, setV] = useState("a");
      return <Tabs label="x" value={v} onChange={setV} tabs={[{ id: "a", label: "A" }, { id: "b", label: "B", count: 3 }]} />;
    }
    render(<T />);
    screen.getByRole("tab", { name: "A" }).focus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: /B/ })).toHaveAttribute("aria-selected", "true");
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: /B/ }));
  });
  it("Segmented exposes pressed state", async () => {
    const fn = vi.fn();
    render(<Segmented label="v" value="a" onChange={fn} options={[{ value: "a", label: "A" }, { value: "b", label: "B" }]} />);
    expect(screen.getByRole("button", { name: "A" })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(screen.getByRole("button", { name: "B" }));
    expect(fn).toHaveBeenCalledWith("b");
  });
  it("Menu opens, navigates with arrows, selects with Enter and closes on Esc", async () => {
    const user = userEvent.setup();
    const pick = vi.fn();
    render(<Menu label="More" items={[{ id: "1", label: "One", onSelect: () => pick("1") }, { id: "2", label: "Two", onSelect: () => pick("2") }]}
      trigger={({ props }) => <button {...props}>open</button>} />);
    await user.click(screen.getByText("open"));
    expect(screen.getByRole("menu", { name: "More" })).toBeInTheDocument();
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "One" }));
    await user.keyboard("{ArrowDown}{Enter}");
    expect(pick).toHaveBeenCalledWith("2");
    await user.click(screen.getByText("open"));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
  });
  it("Select filters options by typing and picks with Enter", async () => {
    const user = userEvent.setup();
    const fn = vi.fn();
    const options = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta"].map((v) => ({ value: v, label: v }));
    render(<Select label="Greek" value="alpha" options={options} onChange={fn} />);
    await user.click(screen.getByRole("button", { name: /Greek/ }));
    await user.keyboard("zet{Enter}");
    expect(fn).toHaveBeenCalledWith("zeta");
  });
  it("Checkbox and RadioGroup are native controls", async () => {
    const user = userEvent.setup();
    const c = vi.fn();
    const r = vi.fn();
    render(<><Checkbox label="Sure" onChange={c} /><RadioGroup label="pick" value="a" onChange={r} options={[{ value: "a", label: "A" }, { value: "b", label: "B" }]} /></>);
    await user.click(screen.getByRole("checkbox", { name: "Sure" }));
    expect(c).toHaveBeenCalledWith(true);
    await user.click(screen.getByRole("radio", { name: "B" }));
    expect(r).toHaveBeenCalledWith("b");
    expect(screen.getByRole("group", { name: "pick" })).toBeInTheDocument();
  });
  it("Button pending disables and marks busy", () => {
    render(<Button pending>Send</Button>);
    expect(screen.getByRole("button")).toBeDisabled();
    expect(screen.getByRole("button")).toHaveAttribute("aria-busy", "true");
  });
});

describe("Toast region and VirtualList", () => {
  it("announces errors with role=alert inside a polite live region", () => {
    toastStore.set([]);
    render(<ToastRegion />);
    act(() => void toast({ tone: "err", title: "Boom", detail: "why" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Boom");
    expect(screen.getByLabelText("Notifications")).toHaveAttribute("aria-live", "polite");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });
  it("mounts only a window of hundreds of rows", () => {
    const items = Array.from({ length: 500 }, (_, i) => i);
    render(<VirtualList items={items} getKey={(i) => String(i)} estimateSize={() => 30} renderItem={(i) => <div data-testid="r">row {i}</div>} initialCount={25} />);
    expect(screen.getAllByTestId("r").length).toBe(25);
  });
});

describe("Escape: one owner, innermost first", () => {
  it("hands Esc to the most recently registered owner only, then the next", async () => {
    const user = userEvent.setup();
    const calls: string[] = [];
    const a = pushEscape(() => calls.push("outer"));
    const b = pushEscape(() => calls.push("inner"));
    await user.keyboard("{Escape}");
    b();
    await user.keyboard("{Escape}");
    a();
    await user.keyboard("{Escape}"); // nobody owns it: not swallowed, nothing called
    expect(calls).toEqual(["inner", "outer"]);
  });

  it("a Select open inside a Dialog closes alone; the dialog and what was typed survive until the second Esc", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <Dialog open onClose={onClose} title="Host">
        <input aria-label="note" />
        <Select label="Model" value="a" options={[{ value: "a", label: "Alpha" }, { value: "b", label: "Beta" }]} onChange={() => {}} />
      </Dialog>,
    );
    await user.type(screen.getByLabelText("note"), "keep me");
    await user.click(screen.getByRole("button", { name: /Model: Alpha/ }));
    expect(screen.getByRole("listbox", { name: "Model" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByLabelText("note")).toHaveValue("keep me");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: /Model: Alpha/ }));
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("a Popover inside a Dialog likewise closes alone", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <Dialog open onClose={onClose} title="Host">
        <Popover trigger={({ props }) => <button {...props}>more</button>}>{() => <div>panel</div>}</Popover>
      </Dialog>,
    );
    await user.click(screen.getByText("more"));
    expect(screen.getByText("panel")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByText("panel")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("two stacked dialogs: one Esc closes only the top one", async () => {
    const user = userEvent.setup();
    const outer = vi.fn();
    const inner = vi.fn();
    render(<><Dialog open onClose={outer} title="Outer">o</Dialog><Dialog open onClose={inner} title="Inner">i</Dialog></>);
    await user.keyboard("{Escape}");
    expect(inner).toHaveBeenCalledTimes(1);
    expect(outer).not.toHaveBeenCalled();
  });

  it("a persistent dialog ignores Esc and scrim clicks, has no close button, and shields what is beneath it", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const beneath = vi.fn();
    const off = pushEscape(beneath);
    render(<Dialog open persistent onClose={onClose} title="Sign in" footer={<button onClick={onClose}>Cancel sign-in</button>}>x</Dialog>);
    await user.keyboard("{Escape}");
    fireEvent.mouseDown(document.querySelector(".scrim")!);
    expect(onClose).not.toHaveBeenCalled();
    expect(beneath).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Sign in" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Cancel sign-in" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    off();
  });

  it("a normal dialog still closes on a scrim click", () => {
    const onClose = vi.fn();
    render(<Dialog open onClose={onClose} title="T">x</Dialog>);
    fireEvent.mouseDown(document.querySelector(".scrim")!);
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("Select option layout", () => {
  const long = "claude-sonnet-4-5-20250929-with-an-unreasonably-long-suffix-that-cannot-fit";
  const options = [
    { value: long, label: long, mono: true, description: "A long sentence about what this model is good for, which needs more than one line in a narrow menu." },
    { value: "b", label: "Beta" },
  ];
  it("puts the label and the description in separate blocks (two lines), not side by side", async () => {
    const user = userEvent.setup();
    render(<Select label="Model" value="b" options={options} onChange={() => {}} />);
    await user.click(screen.getByRole("button", { name: /Model: Beta/ }));
    const first = screen.getAllByRole("option")[0];
    const text = first.querySelector(".combo-t")!;
    expect(Array.from(text.children).map((c) => c.className)).toEqual(["combo-l mono", "d"]);
    expect(text.querySelector(".combo-l")!.textContent).toBe(long);
    expect(text.querySelector(".combo-l")).toHaveAttribute("title", long); // truncated visually, complete on hover
    expect(text.querySelector(".d")!.textContent).toMatch(/narrow menu\.$/);
    // an option without a description has just the label line
    expect(screen.getAllByRole("option")[1].querySelectorAll(".combo-t > *").length).toBe(1);
  });
  it("does not size the list from inline styles (the stylesheet owns width) and marks the upward variant by class", async () => {
    const user = userEvent.setup();
    render(<Select compact up label="Model" value="b" options={options} onChange={() => {}} />);
    await user.click(screen.getByRole("button", { name: /Model: Beta/ }));
    const list = document.querySelector(".combo-list")!;
    expect(list.getAttribute("style")).toBeNull();
    expect(list.className).toContain("up");
  });
});

describe("Select opens on the side with room", () => {
  const rect = (top: number, bottom: number) => ({ top, bottom, left: 0, right: 100, width: 100, height: bottom - top, x: 0, y: top, toJSON: () => ({}) }) as DOMRect;
  async function openAt(top: number, up?: boolean) {
    const rects = vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      return this.classList.contains("combo") ? rect(top, top + 30) : rect(0, 700); // every clipping ancestor spans 0..700
    });
    const height = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(200);
    const view = render(<Select up={up} label="Model" value="a" options={[{ value: "a", label: "A" }]} onChange={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: /Model/ }));
    const cls = document.querySelector(".combo-list")!.className;
    view.unmount();
    rects.mockRestore();
    height.mockRestore();
    return cls;
  }
  it("a menu near the bottom of its scroller flips upwards, one with room stays below; a composer picker with no room above flips down", async () => {
    expect(await openAt(300)).not.toContain("up");
    expect(await openAt(600)).toContain("up");
    expect(await openAt(300, true)).toContain("up");
    expect(await openAt(100, true)).not.toContain("up");
  });
});

describe("Clipboard and CopyButton", () => {
  const original = Object.getOwnPropertyDescriptor(navigator, "clipboard");
  const exec = Object.getOwnPropertyDescriptor(document, "execCommand");
  afterEach(() => {
    if (original) Object.defineProperty(navigator, "clipboard", original);
    else delete (navigator as { clipboard?: unknown }).clipboard;
    if (exec) Object.defineProperty(document, "execCommand", exec);
    else delete (document as { execCommand?: unknown }).execCommand;
    toastStore.set([]);
  });
  const setClipboard = (writeText: (t: string) => Promise<void>) => Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  const setExec = (fn: (c: string) => boolean) => Object.defineProperty(document, "execCommand", { configurable: true, value: fn });

  it("copyText resolves only after the write, with the exact text", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    setClipboard(write);
    await expect(copyText("a\n  b `c`")).resolves.toBeUndefined();
    expect(write).toHaveBeenCalledWith("a\n  b `c`");
  });
  it("falls back to the legacy copy command when the async API refuses, and rejects when both fail", async () => {
    setClipboard(() => Promise.reject(new Error("NotAllowedError")));
    const exec = vi.fn().mockReturnValue(true);
    setExec(exec);
    await expect(copyText("x")).resolves.toBeUndefined();
    expect(exec).toHaveBeenCalledWith("copy");
    setExec(() => false);
    await expect(copyText("x")).rejects.toThrow(/NotAllowedError/);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
    await expect(copyText("x")).rejects.toThrow(/not available/);
  });

  it("shows Copied only after the write resolved", async () => {
    const user = userEvent.setup();
    let done!: () => void;
    setClipboard(() => new Promise<void>((r) => (done = r)));
    render(<CopyButton text="hello" label="Copy thing" />);
    await user.click(screen.getByRole("button", { name: "Copy thing" }));
    expect(screen.queryByRole("button", { name: "Copied" })).toBeNull(); // write still pending: nothing claimed
    await act(async () => done());
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Copied");
  });
  it("shows a visible failure, and a toast with the reason, when nothing could be copied", async () => {
    const user = userEvent.setup();
    setClipboard(() => Promise.reject(new Error("denied by the webview")));
    setExec(() => false);
    toastStore.set([]);
    render(<CopyButton text="hello" label="Copy thing" />);
    await user.click(screen.getByRole("button", { name: "Copy thing" }));
    const failed = await screen.findByRole("button", { name: "Copy failed" });
    expect(failed).toHaveAttribute("data-state", "failed");
    expect(screen.queryByRole("button", { name: "Copied" })).toBeNull();
    expect(toastStore.get()[0]).toMatchObject({ tone: "err", title: "Could not copy", detail: expect.stringContaining("denied by the webview") });
  });
});
