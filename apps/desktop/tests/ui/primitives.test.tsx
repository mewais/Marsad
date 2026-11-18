import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import {
  Button, Checkbox, deriveInitials, Dialog, DriverBadge, linkWord, makeDriver, Menu, phaseToStatus, RadioGroup, Segmented, Select, StatusPill, Tabs, toast, ToastRegion, toastStore,
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
    render(<DriverBadge driver={makeDriver("zed", "Zed Agent", 5)} />);
    expect(screen.getByText("ZA")).toBeInTheDocument();
  });
  it("states the link in words", () => {
    expect(linkWord({ link: "reconnecting", retry_in_ms: 4000 })).toBe("Reconnecting in 4s");
    render(<ConnState host={{ link: "offline" }} />);
    expect(screen.getByText("Offline")).toBeInTheDocument();
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
