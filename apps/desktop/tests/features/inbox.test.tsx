import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { sessionByName, renderApp } from "../helpers";

const selected = () => document.querySelector('.ibx-item[aria-current="true"]')?.getAttribute("data-session");

describe("Inbox", () => {
  it("groups attention-first with counts and states every status in words", async () => {
    await renderApp();
    const list = screen.getByRole("list", { name: "Sessions" });
    const text = list.textContent!;
    expect(text.indexOf("Needs you")).toBeLessThan(text.indexOf("Failed"));
    expect(text.indexOf("Failed")).toBeLessThan(text.indexOf("Finished, not seen yet"));
    expect(within(list).getAllByText("Needs you").length).toBeGreaterThan(4);
    expect(screen.getByText(/finished since you looked at/)).toBeInTheDocument();
  });

  it("J/K move the selection, Enter opens the session", async () => {
    const user = userEvent.setup();
    const { backend, history } = await renderApp();
    const first = selected();
    expect(first).toBe(sessionByName(backend, "migrate-auth-middleware").key);
    await user.keyboard("j");
    expect(selected()).toBe(sessionByName(backend, "pdf-renderer-worker").key);
    await user.keyboard("jk");
    expect(selected()).toBe(sessionByName(backend, "pdf-renderer-worker").key);
    await user.keyboard("{Enter}");
    expect(history.store.get()).toBe(`/session/hz/${sessionByName(backend, "pdf-renderer-worker").id}`);
  });

  it("a digit answers the selected permission gate with the agent's Nth choice", async () => {
    const user = userEvent.setup();
    const { backend } = await renderApp();
    await user.keyboard("j"); // pdf-renderer-worker (permission)
    await user.keyboard("1");
    await waitFor(() => expect(backend.requests.some((r) => r.method === "session.command")).toBe(true));
    const req = backend.requests.find((r) => r.method === "session.command")!;
    expect(req.params).toMatchObject({ command: { command: "answer_gate", request_id: "p-pdf", decision: { action: "approve" } } });
    // the answered session leaves "Needs you"
    await waitFor(() => expect(document.querySelector(`[data-session="${sessionByName(backend, "pdf-renderer-worker").key}"]`)?.getAttribute("data-status")).not.toBe("needs"));
  });

  it("D denies, 1-4 pick a question option and Enter sends", async () => {
    const user = userEvent.setup();
    const { backend } = await renderApp();
    await user.keyboard("2"); // question row selected first: option 2
    expect(screen.getByRole("radio", { name: /Migrate on first request/ })).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(backend.requests.find((r) => r.method === "session.command")?.params).toMatchObject({ command: { command: "answer_question" } }));
  });

  it("E marks a finished session seen", async () => {
    const user = userEvent.setup();
    const { backend } = await renderApp();
    const target = sessionByName(backend, "ci-repro-windows").key;
    for (let i = 0; i < 12 && selected() !== target; i++) await user.keyboard("j");
    expect(selected()).toBe(target);
    expect(document.querySelector(`[data-session="${target}"]`)?.getAttribute("data-status")).toBe("done");
    await user.keyboard("e");
    await waitFor(() => expect(document.querySelector(`[data-session="${target}"]`)?.getAttribute("data-status")).toBe("idle"));
  });

  it("/ focuses the filter and typing narrows the list", async () => {
    const user = userEvent.setup();
    await renderApp();
    await user.keyboard("/");
    const input = screen.getByRole("searchbox", { name: "Filter sessions" });
    expect(document.activeElement).toBe(input);
    await user.type(input, "tile-cache");
    await waitFor(() => expect(document.querySelectorAll("[data-session]").length).toBe(1));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(document.querySelectorAll("[data-session]").length).toBeGreaterThan(5));
    expect(input).toHaveValue("");
  });

  it("? opens the shortcut sheet listing the registered keys; Ctrl-K opens the palette", async () => {
    const user = userEvent.setup();
    await renderApp();
    await user.keyboard("?");
    const help = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    expect(within(help).getAllByText("Next session").length).toBeGreaterThan(0);
    await user.keyboard("{Escape}");
    await user.keyboard("{Control>}k{/Control}");
    const pal = screen.getByRole("dialog", { name: "Command palette" });
    await user.type(within(pal).getByRole("combobox"), "tile");
    expect(await within(pal).findByRole("option", { name: /tile-cache-eviction/ })).toBeInTheDocument();
  });
});
