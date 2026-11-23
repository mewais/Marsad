import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { countLines, parseDiff, splitRows } from "../../src/features/session/tabs/diff";
import { quietBackend, renderApp, sessionByName } from "../helpers";

const DIFF = `diff --git a/a.ts b/a.ts
--- a/a.ts
+++ b/a.ts
@@ -1,4 +1,5 @@ head
 one
-two
-three
+TWO
+THREE
+FOUR
 four
`;

async function open(name: string, tab: string) {
  const backend = quietBackend();
  const s = sessionByName(backend, name);
  const r = await renderApp(`/session/${s.host}/${s.id}?tab=${tab}`, backend);
  await screen.findByRole("heading", { name });
  return { ...r, s };
}

describe("diff rendering", () => {
  it("parses hunks with old/new line numbers", () => {
    const [h] = parseDiff(DIFF);
    expect(h.lines.map((l) => [l.kind, l.old, l.new])).toEqual([["ctx", 1, 1], ["del", 2, undefined], ["del", 3, undefined], ["add", undefined, 2], ["add", undefined, 3], ["add", undefined, 4], ["ctx", 4, 5]]);
    expect(countLines([h])).toEqual({ added: 3, removed: 2 });
  });
  it("pairs deletions with additions side by side", () => {
    const rows = splitRows(parseDiff(DIFF)[0]);
    expect(rows.map((r) => [r.left?.text ?? null, r.right?.text ?? null])).toEqual([["one", "one"], ["two", "TWO"], ["three", "THREE"], [null, "FOUR"], ["four", "four"]]);
  });
});

describe("session tabs", () => {
  it("changes: lists files, shows unified and split diffs, says when a diff was cut", async () => {
    const user = userEvent.setup();
    await open("migrate-auth-middleware", "changes");
    const files = await screen.findByRole("listbox", { name: "Changed files" });
    expect(within(files).getAllByRole("option")).toHaveLength(8);
    expect(await screen.findByText("import { authMiddleware } from '@ledgerline/auth'")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Split" }));
    expect(document.querySelector(".diff.split .sl")).not.toBeNull();
    await user.click(within(files).getByRole("option", { name: /pnpm-lock.yaml/ }));
    expect(await screen.findByText(/longer than Marsad shows at once/)).toBeInTheDocument();
    await user.click(within(files).getByRole("option", { name: /auth-flow.png/ }));
    expect(await screen.findByText(/Binary file/)).toBeInTheDocument();
  });

  it("changes: diffs are coloured by the file's language, deletions from the old side", async () => {
    const user = userEvent.setup();
    await open("migrate-auth-middleware", "changes");
    await screen.findByText("import { authMiddleware } from '@ledgerline/auth'");
    const table = () => screen.getByRole("table", { name: /^Diff of / });
    await waitFor(() => expect(table().getAttribute("data-lang")).toBe("typescript"));
    const kw = [...table().querySelectorAll(".ln.a .hljs-keyword, .ln.d .hljs-keyword, .ln.u .hljs-keyword")].map((e) => e.textContent);
    expect(kw).toContain("import");
    expect(table().querySelector(".ln.d .hljs-string, .ln.d .hljs-keyword")).not.toBeNull();
    // Side by side keeps the colours.
    await user.click(screen.getByRole("button", { name: "Split" }));
    expect(table().querySelector(".sl .hljs-keyword")).not.toBeNull();
  });

  it("changes: commit sends exactly the picked files with the message", async () => {
    const user = userEvent.setup();
    const { backend, s } = await open("migrate-auth-middleware", "changes");
    await screen.findByRole("listbox", { name: "Changed files" });
    await user.click(screen.getByRole("checkbox", { name: "Include pnpm-lock.yaml" }));
    await user.type(screen.getByLabelText(/Commit to refactor\/auth-mw/), "refactor(api): auth middleware");
    await user.click(screen.getByRole("button", { name: /Commit 7 files/ }));
    await waitFor(() => expect(backend.requests.find((r) => r.method === "git.commit")).toBeDefined());
    const req = backend.requests.find((r) => r.method === "git.commit")!;
    expect(req.params).toMatchObject({ target: { session: s.id }, message: "refactor(api): auth middleware" });
    expect((req.params as { files: string[] }).files).not.toContain("pnpm-lock.yaml");
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(1));
  });

  it("changes: discard asks in an in-app dialog first", async () => {
    const user = userEvent.setup();
    const { backend } = await open("migrate-auth-middleware", "changes");
    await screen.findByRole("listbox", { name: "Changed files" });
    await user.click(screen.getByRole("button", { name: "Discard…" }));
    const dlg = screen.getByRole("dialog", { name: /Discard changes to 8 files/ });
    expect(document.activeElement).toBe(within(dlg).getByRole("button", { name: "Cancel" }));
    await user.click(within(dlg).getByRole("button", { name: "Cancel" }));
    expect(backend.requests.some((r) => r.method === "git.discard")).toBe(false);
    await user.click(screen.getByRole("button", { name: "Discard…" }));
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(backend.requests.some((r) => r.method === "git.discard")).toBe(true));
    expect(await screen.findByText("Working tree is clean")).toBeInTheDocument();
  });

  it("config: effective items with provenance from the hub", async () => {
    await open("migrate-auth-middleware", "config");
    expect(await screen.findByText("turned off at Host")).toBeInTheDocument();
    expect(screen.getAllByText("from Claude Code config").length).toBeGreaterThan(2);
    expect(screen.getByText("added at Global")).toBeInTheDocument();
  });
});
