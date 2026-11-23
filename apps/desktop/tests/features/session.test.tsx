import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { hold, renderApp, sessionByName, quietBackend } from "../helpers";

async function open(name: string, backend = quietBackend()) {
  const s = sessionByName(backend, name);
  const r = await renderApp(`/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name });
  return { ...r, s };
}

describe("Session view", () => {
  it("renders markdown, collapsed thinking, tool calls, subagent and inline gates", async () => {
    const user = userEvent.setup();
    await open("migrate-auth-middleware");
    expect(await screen.findByText(/find every place that reads/)).toBeInTheDocument();
    expect(document.querySelector(".msg-ai code")?.textContent).toBe("req.session"); // inline `code` rendered
    const think = screen.getByRole("button", { name: /Thinking/ });
    expect(think).toHaveAttribute("aria-expanded", "false");
    await user.click(think);
    expect(screen.getByText(/I need to find every place/)).toBeInTheDocument();
    expect(screen.getByTestId("subagent")).toHaveTextContent("Subagent · Explore");
    const grep = screen.getByRole("button", { name: /Grep/ });
    await user.click(grep);
    expect(document.querySelector(".tool-b")?.textContent).toContain("routes/login.ts:22");
    // The Edit and the failing Bash that follow the subagent are one run of tool calls: open it first.
    const run = screen.getByRole("button", { name: /2 tool calls.*1 failed/ });
    expect(run).toHaveAttribute("aria-expanded", "false");
    await user.click(run);
    await user.click(screen.getByRole("button", { name: /Edit apps\/api\/src\/server.ts/ }));
    expect(screen.getByTestId("tool-diff")).toBeInTheDocument();
    expect(within(document.querySelector(".run-b") as HTMLElement).getByRole("button", { name: /Bash.*failed/ })).toBeInTheDocument();
    expect(screen.getByText(/Question from Claude Code/)).toBeInTheDocument();
    expect(screen.getByText(/Permission needed · Bash/)).toBeInTheDocument();
    expect(screen.getByText("Waiting for you · 1 permission, 1 question")).toBeInTheDocument();
    expect(screen.getByText(/Plan · 3 of 6/i)).toBeInTheDocument();
  });

  it("terminal tab is honest when the host cannot give a terminal", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    hold(backend, "pty.list").failAll({ code: "unsupported", message: "Terminals need the Marsad helper on a real host." });
    await open("docs-search-index", backend);
    await user.click(screen.getByRole("tab", { name: "Terminal" }));
    expect(await screen.findByText("Terminal not available")).toBeInTheDocument();
    expect(screen.getByText(/need the Marsad helper/)).toBeInTheDocument();
  });

  it("composer: slash menu comes from the snapshot, mode and model pickers too, and sending fires a prompt", async () => {
    const user = userEvent.setup();
    const { backend, s } = await open("docs-search-index");
    const box = screen.getByRole("textbox", { name: "Message" });
    await user.type(box, "/");
    const list = screen.getByRole("listbox", { name: "Slash commands" });
    expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual(expect.arrayContaining([expect.stringContaining("/init")]));
    await user.clear(box);
    await user.click(screen.getByRole("button", { name: /Mode: Build/ }));
    expect(screen.getByRole("option", { name: /Plan/ })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await user.type(box, "add a sitemap{Enter}");
    await waitFor(() => expect(backend.requests.some((r) => (r.params as { command?: { text?: string } })?.command?.text === "add a sitemap")).toBe(true));
    expect(await screen.findByText("add a sitemap")).toBeInTheDocument();
    expect(box).toHaveValue("");
    // the scripted agent replies and the session finishes
    expect(await screen.findByText(/Nothing else needs your attention/, undefined, { timeout: 3000 })).toBeInTheDocument();
    void s;
  });
});
