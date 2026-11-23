import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { closeDialogs } from "../../src/shell/dialogs";
import { nothingToContinue, workingDirectoryRefusal } from "../../src/bridge/api";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { renderApp } from "../helpers";

const MESSAGE = "there is no earlier conversation to continue in /home/dev/src/scratch";
/** How the hub relays a host's refusal of a launch: wrapped as an `upstream` error, the host's code and message in `cause`. */
const upstream = (code: string, message: string) => ({ code: "upstream", message: `sessions.create on x1: ${message}`, data: { marsad_error: "upstream", message: `sessions.create on x1: ${message}`, cause: { code, message, data: null } } });

describe("the host's answer to 'continue the most recent conversation' of a folder that has none", () => {
  it("is read from the upstream cause, as the refusal of a bad folder is, and from a bare error too", () => {
    expect(nothingToContinue(upstream("nothing_to_continue", MESSAGE))).toBe(MESSAGE);
    expect(nothingToContinue({ code: "nothing_to_continue", message: MESSAGE })).toBe(MESSAGE);
    expect(nothingToContinue(upstream("bad_working_directory", "x"))).toBeNull();
    expect(workingDirectoryRefusal(upstream("nothing_to_continue", MESSAGE))).toBeNull();
    expect(nothingToContinue(new Error("nope"))).toBeNull();
    expect(nothingToContinue(undefined)).toBeNull();
  });
});

// The dialog's open state lives outside the app: a test that leaves it open would open the next test's app with it.
afterEach(() => closeDialogs());

async function openDialog(folder = "~/src/scratch") {
  const user = userEvent.setup();
  const r = await renderApp();
  await user.keyboard("n");
  const dlg = await screen.findByRole("dialog", { name: "New session" });
  await waitFor(() => expect(within(dlg).getByLabelText("Project folder")).toHaveValue("~/src/ledgerline"));
  const field = within(dlg).getByLabelText("Project folder");
  await user.clear(field);
  await user.type(field, folder);
  await waitFor(() => expect(within(dlg).getByRole("radio", { name: /Claude Code/ })).toHaveAttribute("aria-checked", "true"));
  return { user, dlg, ...r };
}
const launches = (b: FixtureBackend) => b.requests.filter((r) => r.method === "sessions.launch");

describe("new session: Continue most recent in a folder with no conversation", () => {
  it("keeps the dialog open with everything typed and says, next to the choice, that there is nothing to continue (the daemon's words)", async () => {
    const { user, dlg, backend } = await openDialog();
    const before = backend.sessionIds("x1").length;
    await user.type(within(dlg).getByLabelText(/First message/), "Pick up where we left off");
    await user.click(within(dlg).getByRole("button", { name: "Continue an existing one" }));
    await waitFor(() => expect(within(dlg).getByRole("button", { name: "Selected" })).toBeInTheDocument());
    await user.type(within(dlg).getByLabelText("Name"), "scratchpad");
    await user.click(within(dlg).getByRole("button", { name: /Continue session/ }));
    const note = await within(dlg).findByTestId("ns-nothing");
    expect(note).toHaveTextContent(MESSAGE);
    expect(note).toHaveAttribute("role", "alert");
    // The request went out as "latest"; the host refused, so nothing was created and the dialog is as it was.
    expect(launches(backend)).toHaveLength(1);
    expect(launches(backend)[0].params).toMatchObject({ resume: { kind: "latest" } });
    expect(backend.sessionIds("x1")).toHaveLength(before);
    expect(screen.getByRole("dialog", { name: "New session" })).toBeInTheDocument();
    expect(within(dlg).getByLabelText(/Message to send on resume/)).toHaveValue("Pick up where we left off");
    expect(within(dlg).getByLabelText("Name")).toHaveValue("scratchpad");
    expect(within(dlg).getByLabelText("Project folder")).toHaveValue("~/src/scratch");
    // It sits with the resume choice, not as a toast or next to the folder field.
    expect(note.closest(".col")).toBe(within(dlg).getByRole("button", { name: "Selected" }).closest(".col"));
    expect(note.compareDocumentPosition(within(dlg).getByLabelText(/Message to send on resume/)) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("offers a new conversation as the user's own choice, never starts one by itself", async () => {
    const { user, dlg, backend, history } = await openDialog();
    await user.click(within(dlg).getByRole("button", { name: "Continue an existing one" }));
    await user.type(within(dlg).getByLabelText(/Message to send on resume/), "Start the notes");
    await user.click(within(dlg).getByRole("button", { name: /Continue session/ }));
    const note = await within(dlg).findByTestId("ns-nothing");
    expect(launches(backend)).toHaveLength(1);
    await user.click(within(note).getByRole("button", { name: "Start a new conversation instead" }));
    // Still nothing started: the choice is only switched, and what was typed is kept.
    expect(launches(backend)).toHaveLength(1);
    expect(screen.queryByTestId("ns-nothing")).toBeNull();
    expect(within(dlg).getByLabelText(/First message/)).toHaveValue("Start the notes");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    await waitFor(() => expect(launches(backend)).toHaveLength(2));
    expect(launches(backend)[1].params).toMatchObject({ resume: { kind: "new" }, first_prompt: "Start the notes" });
    await waitFor(() => expect(history.store.get()).toMatch(/^\/session\/x1\//));
  });

  it("stops saying it once the folder changes, and a folder that has conversations continues its most recent", async () => {
    const { user, dlg, backend, history } = await openDialog();
    await user.click(within(dlg).getByRole("button", { name: "Continue an existing one" }));
    await user.click(within(dlg).getByRole("button", { name: /Continue session/ }));
    await within(dlg).findByTestId("ns-nothing");
    console.log("DBG", [...dlg.querySelectorAll("[role=radio]")].map((x) => x.textContent + ":" + x.getAttribute("aria-checked") + (x.hasAttribute("disabled") ? "(dis)" : "")).join(" | "), "| cwd=", (within(dlg).getByLabelText("Project folder") as HTMLInputElement).value);
  const field = within(dlg).getByLabelText("Project folder");
    await user.clear(field);
    await user.type(field, "~/src/ledgerline");
    await waitFor(() => expect(screen.queryByTestId("ns-nothing")).toBeNull());
    await user.click(within(dlg).getByRole("button", { name: /Continue session/ }));
    await waitFor(() => expect(launches(backend)).toHaveLength(2));
    await waitFor(() => expect(history.store.get()).toMatch(/^\/session\/x1\//));
    // The host resolved "most recent" to a conversation of the folder, and it comes with its earlier messages.
    const id = history.store.get().split("/").pop()!;
    expect(backend.snapshot("x1", id).native_session_id).toBe("native-7");
    expect(backend.snapshot("x1", id).earlier.state.kind).not.toBe("none");
    // And the session it opened shows that conversation above its own.
    const earlier = await screen.findByTestId("earlier-part");
    expect((await within(earlier).findAllByText(/^Round \d+: /)).length).toBeGreaterThan(0);
    await waitFor(() => expect(backend.snapshot("x1", id).earlier.state).toEqual({ kind: "loaded", complete: true }));
    expect(screen.getByTestId("earlier-divider")).toBeInTheDocument();
  });
});
