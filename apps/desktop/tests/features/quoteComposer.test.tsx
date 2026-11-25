import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { clearDraft } from "../../src/features/session/draft";
import { quietBackend, renderApp, sessionByName } from "../helpers";

describe("quote in reply lands in the open composer", () => {
  it("the row menu's Quote in reply puts the row's text, quoted, into the composer and focuses it", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const s = sessionByName(backend, "docs-search-index");
    const key = `${s.host}/${s.id}`;
    clearDraft(key);
    await renderApp(`/session/${s.host}/${s.id}`, backend);
    await screen.findByRole("heading", { name: "docs-search-index" });
    const box = (await screen.findByRole("textbox", { name: "Message" })) as HTMLTextAreaElement;
    await user.type(box, "my reply");

    const row = (document.querySelector(".msg-user") ?? document.querySelector(".msg-ai")) as HTMLElement;
    expect(row).toBeTruthy();
    const rowText = row.textContent ?? "";
    const menuBtn = within(row.closest(".row-wrap") as HTMLElement).getByRole("button", { name: "Message actions" });
    menuBtn.focus();
    await user.keyboard("{Enter}");
    await user.click(within(screen.getByRole("menu", { name: "Message actions" })).getByRole("menuitem", { name: "Quote in reply" }));

    await waitFor(() => expect(box.value).toMatch(/^my reply\n\n> /));
    const quoted = box.value.split("\n").filter((l) => l.startsWith("> ")).map((l) => l.slice(2));
    expect(quoted.length).toBeGreaterThan(0);
    expect(rowText.replace(/\s+/g, " ")).toContain(quoted[0].replace(/\s+/g, " ").slice(0, 20));
    expect(document.activeElement).toBe(box);
  });
});
