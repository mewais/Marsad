import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { quietBackend, renderApp, sessionByName } from "../helpers";

/** The refusal as the reader sees it (the words are the product's, not the implementation's). */
const COMMAND_WITH_FILES = "Commands can't take attachments. Remove the files, or remove the /.";

// docs-search-index is an opencode session: it reports the /init command.
async function openComposer(backend = quietBackend()) {
  const s = sessionByName(backend, "docs-search-index");
  await renderApp(`/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name: "docs-search-index" });
  return { backend, box: (await screen.findByRole("textbox", { name: "Message" })) as HTMLTextAreaElement };
}

/** Attaches one PNG the way a paste does. */
async function attachPng(box: HTMLElement, name = "shot.png") {
  const png = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], name, { type: "image/png" });
  fireEvent.paste(box, { clipboardData: { files: [png], types: ["Files"] } });
  return screen.findAllByTestId("pending-attachment");
}

/** The commands the composer sent to the session, in order. */
const sent = (backend: FixtureBackend) =>
  backend.requests.filter((r) => r.method === "session.command").map((r) => (r.params as { command: { command: string; text?: string; name?: string; args?: string; attachments?: unknown[] } }).command);

describe("composer: a reported slash command takes no attachments", () => {
  it("a command with attachments is refused: the message says why, and the text and the files stay", async () => {
    const user = userEvent.setup();
    const { backend, box } = await openComposer();
    await user.type(box, "/init");
    expect(await attachPng(box)).toHaveLength(1);
    expect(await screen.findByText(COMMAND_WITH_FILES)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await user.keyboard("{Enter}");
    expect(sent(backend)).toEqual([]);
    expect(box).toHaveValue("/init");
    expect(screen.getAllByTestId("pending-attachment")).toHaveLength(1);
  });

  it("removing the files lets the same command run as a command, and the message goes", async () => {
    const user = userEvent.setup();
    const { backend, box } = await openComposer();
    await user.type(box, "/init");
    await attachPng(box);
    await screen.findByText(COMMAND_WITH_FILES);
    await user.click(screen.getByRole("button", { name: "Remove shot.png" }));
    expect(screen.queryByText(COMMAND_WITH_FILES)).toBeNull();
    // The remove button had the focus and is gone: the reader goes back to the message.
    await user.click(box);
    await user.keyboard("{End}{Enter}");
    await waitFor(() => expect(sent(backend)).toEqual([expect.objectContaining({ command: "run_slash", name: "init", args: "" })]));
  });

  it("removing the slash instead lets the same files go as a prompt", async () => {
    const user = userEvent.setup();
    const { backend, box } = await openComposer();
    await user.type(box, "/init");
    await attachPng(box);
    await screen.findByText(COMMAND_WITH_FILES);
    await user.keyboard("{Home}{Delete}");
    expect(box).toHaveValue("init");
    expect(screen.queryByText(COMMAND_WITH_FILES)).toBeNull();
    expect(sent(backend)).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(sent(backend)).toEqual([expect.objectContaining({ command: "prompt", text: "init" })]));
  });
});

describe("composer: an unknown /foo is an ordinary prompt", () => {
  it("with attachments it is sent as the prompt with its files, and no refusal is shown", async () => {
    const user = userEvent.setup();
    const { backend, box } = await openComposer();
    await user.type(box, "/foo bar");
    await attachPng(box);
    expect(screen.queryByText(COMMAND_WITH_FILES)).toBeNull();
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(sent(backend)).toEqual([expect.objectContaining({ command: "prompt", text: "/foo bar" })]));
    expect(sent(backend)[0].attachments).toHaveLength(1);
  });

  it("without attachments it is sent as the prompt text, as before", async () => {
    const user = userEvent.setup();
    const { backend, box } = await openComposer();
    await user.type(box, "/foo");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(sent(backend)).toEqual([expect.objectContaining({ command: "prompt", text: "/foo" })]));
  });
});
