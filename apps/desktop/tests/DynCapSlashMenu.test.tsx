/**
 * U2 — Composer slash menu consumes ONLY the session's live slash_commands.
 *
 * Contract pinned (docs/specs/dynamic-capabilities.md §1.3, "the consumption
 * contract"):
 *   "The Composer/Activity slash menu's ONLY source is `sv.slash_commands`
 *   (`Composer.tsx:72`, `Activity.tsx:553-555` today pass it but fall back
 *   to the catalog). Empty list (pre-init, or a driver that reports none) →
 *   empty menu with a 'waiting for the session to report commands' hint.
 *   The empty state is first-class UX, never backfilled from a catalog....
 *   Deleted: `SLASH_CATALOG` + the fallback arm of `filterSlash`
 *   (`api.ts:581-615`)."
 *
 * Today `bridge/api.ts`'s `filterSlash()` falls back to the static
 * `SLASH_CATALOG` (11 hardcoded commands) whenever `sessionSlash` is
 * empty/undefined — this suite pins the honest-empty-state contract and is
 * RED until that fallback arm (and `SLASH_CATALOG` itself) are deleted.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Composer } from "../src/components/Composer";
import { filterSlash } from "../src/bridge/api";

describe("api.filterSlash — no catalog fallback (U2 kill-list)", () => {
  it("an explicitly empty session slash list returns [] — never the 11-entry SLASH_CATALOG", () => {
    expect(filterSlash("/", [])).toEqual([]);
    expect(filterSlash("", [])).toEqual([]);
  });

  it("an absent (undefined) session slash list ALSO returns [] — pre-init is an honest empty state, not a catalog", () => {
    expect(filterSlash("/", undefined)).toEqual([]);
  });

  it("a non-empty session slash list is filtered by prefix and returns ONLY session-provided entries", () => {
    const sessionSlash = [
      { name: "/foo", description: "does foo", argument_hint: "" },
      { name: "/bar", description: "does bar", argument_hint: "" },
    ];
    const result = filterSlash("/foo", sessionSlash);
    expect(result).toEqual([{ name: "/foo", desc: "does foo" }]);
    // None of the static catalog's names leak in even as extra entries.
    expect(result.find((e) => e.name === "/clear")).toBeUndefined();
    expect(result.find((e) => e.name === "/model")).toBeUndefined();
  });
});

describe("Composer slash popover — empty state is a hint, not a fallback catalog", () => {
  it("typing '/' with no session slash_commands shows a 'waiting for the session to report commands' hint, not 11 preset entries", () => {
    render(
      <Composer sessionId="s-empty" onSend={vi.fn()} onInterrupt={vi.fn()} slashCommands={[]} />
    );
    const ta = screen.getByPlaceholderText(/Send a prompt/i);
    fireEvent.change(ta, { target: { value: "/" } });

    // Honest empty state: zero command rows are rendered.
    const rows = document.querySelectorAll('[data-testid^="slash-entry-"]');
    expect(rows.length).toBe(0);
    // The static catalog's commands must not appear anywhere in the popover.
    expect(screen.queryByText("/clear")).toBeNull();
    expect(screen.queryByText("/model")).toBeNull();
    expect(screen.queryByText(/waiting for the session to report commands/i)).toBeTruthy();
  });

  it("typing '/' before slashCommands is even passed (undefined, pre-init) also shows the hint, never the catalog", () => {
    render(<Composer sessionId="s-preinit" onSend={vi.fn()} onInterrupt={vi.fn()} />);
    const ta = screen.getByPlaceholderText(/Send a prompt/i);
    fireEvent.change(ta, { target: { value: "/" } });

    expect(screen.queryByText("/clear")).toBeNull();
    expect(screen.queryByText(/waiting for the session to report commands/i)).toBeTruthy();
  });
});
