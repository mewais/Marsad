/**
 * GateOptions — the shared stacked-row renderer for gate answer choices.
 * (Elicitation choices, permission/ACP suggestions, and question options all
 * render through it now — one implementation, one layout.)
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { GateOptions, type GateOption } from "../src/components/GateOptions";

function opt(over: Partial<GateOption> = {}): GateOption {
  return { id: "o1", label: "Option one", onSelect: () => {}, ...over };
}

describe("GateOptions", () => {
  it("renders null when there are no options", () => {
    const { container } = render(<GateOptions options={[]} />);
    expect(container.firstChild).toBeNull();
  });

  it("renders one button per option with its label and testid", () => {
    render(
      <GateOptions
        testid="my-choices"
        options={[
          opt({ id: "a", testid: "opt-a", label: "Allow once" }),
          opt({ id: "b", testid: "opt-b", label: "Always allow" }),
        ]}
      />
    );
    expect(screen.getByTestId("my-choices")).toBeTruthy();
    expect(screen.getByTestId("opt-a").textContent).toContain("Allow once");
    expect(screen.getByTestId("opt-b").textContent).toContain("Always allow");
  });

  it("renders the optional description line", () => {
    render(<GateOptions options={[opt({ testid: "o", description: "the long form" })]} />);
    expect(screen.getByTestId("o").textContent).toContain("the long form");
  });

  it("fires onSelect when clicked", () => {
    const onSelect = vi.fn();
    render(<GateOptions options={[opt({ testid: "o", onSelect })]} />);
    fireEvent.click(screen.getByTestId("o"));
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("disables every option (and blocks clicks) when disabled", () => {
    const onSelect = vi.fn();
    render(<GateOptions disabled options={[opt({ testid: "o", onSelect })]} />);
    const btn = screen.getByTestId("o") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    fireEvent.click(btn);
    expect(onSelect).not.toHaveBeenCalled();
  });
});
