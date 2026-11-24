import { act, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { VirtualList, type VirtualListHandle } from "../../src/ui";

/**
 * A row whose body is NOT memoised, so it re-renders whenever its React element is recreated — the way an
 * unmemoised markdown row would. It records each render under its id, so a test can count how often a mounted
 * row's body renders across a run of scroll events.
 */
function countingList(items: Row[], renders: Record<string, number>, props: Partial<React.ComponentProps<typeof VirtualList<Row>>> = {}) {
  const Body = ({ id }: { id: string }) => {
    renders[id] = (renders[id] ?? 0) + 1;
    return <div data-testid="row">{id}</div>;
  };
  return list(items, { renderItem: (r) => <Body id={r.id} />, ...props });
}

// Layout is mocked: every `.vlist` is VIEW pixels high and every row is ROW pixels (jsdom has no layout).
const VIEW = 400;
const ROW = 40;
const OVERSCAN = 6;

type Row = { id: string };
/** Rows numbered by their position in the whole list, so a prepend keeps every id. */
const rows = (from: number, to: number): Row[] => Array.from({ length: to - from }, (_, i) => ({ id: `r${from + i}` }));
const box = () => document.querySelector(".vlist") as HTMLElement;
const mounted = () => screen.queryAllByTestId("row").map((e) => e.textContent ?? "");
const scrollBox = (top: number) => {
  const el = box();
  act(() => {
    el.scrollTop = top;
    fireEvent.scroll(el);
  });
};

function list(items: Row[], props: Partial<React.ComponentProps<typeof VirtualList<Row>>> = {}, ref?: React.Ref<VirtualListHandle>) {
  return (
    <VirtualList<Row>
      ref={ref}
      items={items}
      getKey={(r) => r.id}
      estimateSize={() => ROW}
      renderItem={(r) => <div data-testid="row">{r.id}</div>}
      className="vl"
      {...props}
    />
  );
}

/** A ResizeObserver that only reports elements it was asked to observe, so tests can check what is measured. */
class FakeRO {
  static list: FakeRO[] = [];
  observed = new Set<Element>();
  constructor(public cb: ResizeObserverCallback) {
    FakeRO.list.push(this);
  }
  observe(el: Element) {
    this.observed.add(el);
  }
  unobserve(el: Element) {
    this.observed.delete(el);
  }
  disconnect() {
    this.observed.clear();
  }
}

const realRO = globalThis.ResizeObserver;

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains("vlist") ? VIEW : 0;
    },
  });
  FakeRO.list = [];
  globalThis.ResizeObserver = FakeRO as unknown as typeof ResizeObserver;
});

afterEach(() => {
  delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight;
  globalThis.ResizeObserver = realRO;
});

describe("VirtualList windowing", () => {
  it("with 5000 rows, mounts only the viewport plus overscan, at the top and far down", () => {
    render(list(rows(0, 5000)));
    const bound = Math.ceil(VIEW / ROW) + 2 * OVERSCAN + 2;
    expect(mounted().length).toBeGreaterThan(0);
    expect(mounted().length).toBeLessThanOrEqual(bound);
    scrollBox(2500 * ROW);
    expect(mounted()).toContain("r2500");
    expect(mounted().length).toBeLessThanOrEqual(bound);
  });

  it("measures the rows mounted first, and a row that grew moves the rows below it", () => {
    render(list(rows(0, 200), { initialCount: 10 }));
    const ro = FakeRO.list[0];
    const first = screen.getByText("r0").parentElement as HTMLElement;
    // Rows mounted by the first render are observed too, not only the ones mounted after the observer exists.
    expect(ro.observed.has(first)).toBe(true);
    act(() => ro.cb([{ target: first, borderBoxSize: [{ blockSize: ROW + 60 }] } as unknown as ResizeObserverEntry], ro as unknown as ResizeObserver));
    // Scroll to row 40: the window starts at row 32, and the spacer above it holds row 0 at its measured 100px.
    scrollBox(40 * ROW);
    const topSpacer = parseFloat(box().querySelector<HTMLElement>('[aria-hidden="true"]')!.style.height);
    expect(topSpacer).toBe(32 * ROW + 60);
  });
});

describe("VirtualList prepend anchoring", () => {
  it("keeps the first visible row at the same screen offset when rows are prepended above it", () => {
    const { rerender } = render(list(rows(20, 120), { keepAnchor: true }));
    scrollBox(15 * ROW); // r35 is the first visible row, exactly at the top of the viewport
    expect(screen.getByText("r35")).toBeInTheDocument();
    rerender(list(rows(0, 120), { keepAnchor: true }));
    // r35 moved down by the 20 prepended rows; the reader's scroll moved with it, so r35 is still at the top.
    expect(box().scrollTop).toBe(35 * ROW);
    expect(screen.getByText("r35")).toBeInTheDocument();
  });

  it("without keepAnchor a prepend leaves the scroll position alone (Inbox and diagnostics behave as before)", () => {
    const { rerender } = render(list(rows(20, 120)));
    scrollBox(15 * ROW);
    rerender(list(rows(0, 120)));
    expect(box().scrollTop).toBe(15 * ROW);
  });
});

describe("VirtualList stick to end", () => {
  it("starts at the end, follows appends, and releases when the reader scrolls up", () => {
    const { rerender } = render(list(rows(0, 100), { stickToEnd: true, anchor: "end" }));
    expect(box().scrollTop).toBe(100 * ROW - VIEW);
    rerender(list(rows(0, 110), { stickToEnd: true, anchor: "end" }));
    expect(box().scrollTop).toBe(110 * ROW - VIEW);
    expect(mounted()).toContain("r109");
    scrollBox(1000);
    rerender(list(rows(0, 120), { stickToEnd: true, anchor: "end" }));
    expect(box().scrollTop).toBe(1000);
  });
});

describe("VirtualList onReachStart", () => {
  it("fires once per reach of the start, and again on the next reach", () => {
    const onReachStart = vi.fn();
    render(list(rows(0, 100), { onReachStart }));
    expect(onReachStart).toHaveBeenCalledTimes(1); // mounted at the top is a reach
    scrollBox(0);
    scrollBox(0);
    expect(onReachStart).toHaveBeenCalledTimes(1);
    scrollBox(500);
    expect(onReachStart).toHaveBeenCalledTimes(1);
    scrollBox(0);
    expect(onReachStart).toHaveBeenCalledTimes(2);
  });

  it("does not fire while the reader is away from the start", () => {
    const onReachStart = vi.fn();
    render(list(rows(0, 100), { onReachStart, stickToEnd: true, anchor: "end" }));
    expect(onReachStart).not.toHaveBeenCalled();
    scrollBox(2000);
    expect(onReachStart).not.toHaveBeenCalled();
  });
});

describe("VirtualList scroll cost", () => {
  it("does not re-render a mounted row's body on scroll events that do not change its position", () => {
    const renders: Record<string, number> = {};
    render(countingList(rows(0, 200), renders, { initialCount: 20 }));
    // r5 sits in the mounted window at the top and stays there across these nudges (all within the first row's height).
    expect(screen.getByText("r5")).toBeInTheDocument();
    const before = renders["r5"];
    expect(before).toBeGreaterThan(0);
    // Several scroll events that keep the same rows mounted. Each fires a scroll re-render of the list; a cheap list
    // must not re-render the bodies of rows whose position and visibility did not change.
    for (const top of [5, 12, 20, 28, 35, 10, 3]) scrollBox(top);
    expect(screen.getByText("r5")).toBeInTheDocument();
    expect(renders["r5"]).toBe(before);
  });
});

describe("VirtualList handle", () => {
  it("scrollToKey brings a far row into view, mounting it", () => {
    const ref = createRef<VirtualListHandle>();
    render(list(rows(0, 5000), {}, ref));
    act(() => ref.current!.scrollToKey("r4000"));
    expect(mounted()).toContain("r4000");
    expect(box().scrollTop).toBe(4001 * ROW - VIEW);
  });

  it("scrollToEnd shows the last row and follows the list from there", () => {
    const ref = createRef<VirtualListHandle>();
    render(list(rows(0, 300), { stickToEnd: true, anchor: "end" }, ref));
    scrollBox(0);
    act(() => ref.current!.scrollToEnd());
    expect(mounted()).toContain("r299");
    expect(box().scrollTop).toBe(300 * ROW - VIEW);
  });

  it("scrollToKey with highlight marks the row for a flash", () => {
    const ref = createRef<VirtualListHandle>();
    render(list(rows(0, 500), {}, ref));
    act(() => ref.current!.scrollToKey("r300", { highlight: true }));
    expect(screen.getByText("r300").parentElement).toHaveAttribute("data-flash");
    const first = screen.getByText("r300").parentElement!.getAttribute("data-flash");
    expect(first).not.toBeNull();
    // The same row flashed again changes the flash, so its animation starts over.
    act(() => ref.current!.scrollToKey("r300", { highlight: true }));
    expect(screen.getByText("r300").parentElement!.getAttribute("data-flash")).not.toBe(first);
  });
});
