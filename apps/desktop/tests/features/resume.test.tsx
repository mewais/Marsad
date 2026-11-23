import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { closeDialogs } from "../../src/shell/dialogs";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { quietBackend, renderApp, sessionByName } from "../helpers";

type User = ReturnType<typeof userEvent.setup>;

// The open dialogs are app-wide state: a test that leaves one open must not open it in the next.
afterEach(() => closeDialogs());
const requests = (b: FixtureBackend, method: string) => b.requests.filter((r) => r.method === method);

async function openDialog(user: User, backend: FixtureBackend = quietBackend()) {
  const r = await renderApp("/inbox", backend);
  await user.keyboard("n");
  const dlg = await screen.findByRole("dialog", { name: "New session" });
  await waitFor(() => expect(within(dlg).getByLabelText("Project folder")).toHaveValue("~/src/ledgerline"));
  await waitFor(() => expect(within(dlg).getByRole("radio", { name: /Claude Code/ })).toHaveAttribute("aria-checked", "true"));
  return { ...r, dlg };
}

async function pickConversation(user: User, dlg: HTMLElement, title: RegExp) {
  await user.click(within(dlg).getByRole("button", { name: "Continue an existing one" }));
  const list = await within(dlg).findByRole("listbox", { name: "Sessions to continue" });
  await waitFor(() => expect(within(list).getAllByRole("option").length).toBeGreaterThan(3));
  await user.click(within(list).getByRole("option", { name: title }));
}

describe("new session: a conversation that belongs to a Marsad session is that session's", () => {
  it("continuing an ended one continues THAT session (its transcript stays) and opens it; it starts no second session", async () => {
    const user = userEvent.setup();
    const backend = quietBackend({ speed: 1e6 });
    const s = sessionByName(backend, "release-notes-1-4"); // carries native-14
    await backend.request("sessions.stop", { id: s.id }, { host: s.host });
    const { dlg, history } = await openDialog(user, backend);
    await pickConversation(user, dlg, /Spike: Postgres/);
    const note = within(dlg).getByTestId("ns-owner");
    expect(note).toHaveTextContent("This conversation belongs to your session “release-notes-1-4”. Continue picks that session up, with its earlier transcript.");
    // It keeps its own name.
    expect(within(dlg).getByLabelText("Name")).toBeDisabled();
    expect(within(dlg).getByLabelText("Name")).toHaveValue("release-notes-1-4");
    await user.click(within(dlg).getByRole("button", { name: /Continue session/ }));
    await waitFor(() => expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`));
    expect(requests(backend, "sessions.continue").map((r) => r.params)).toEqual([{ id: s.id }]);
    expect(requests(backend, "sessions.launch")).toHaveLength(0);
    expect(backend.sessionIds(s.host).filter((id) => backend.snapshot(s.host, id).native_session_id === "native-14")).toEqual([s.id]);
    expect(backend.snapshot(s.host, s.id).generation).toBe(2);
    // The earlier transcript is there.
    await waitFor(() => expect(document.querySelector(".msg-ai")).not.toBeNull());
    backend.dispose();
  });

  it("with a message typed, the message is the ordinary prompt: the daemon continues the session and delivers it", async () => {
    const user = userEvent.setup();
    const backend = quietBackend({ speed: 1e6 });
    const s = sessionByName(backend, "release-notes-1-4");
    await backend.request("sessions.stop", { id: s.id }, { host: s.host });
    const { dlg, history } = await openDialog(user, backend);
    await pickConversation(user, dlg, /Spike: Postgres/);
    await user.type(within(dlg).getByLabelText(/Message to send on resume/), "carry on with the migration");
    await user.click(within(dlg).getByRole("button", { name: /Continue session/ }));
    await waitFor(() => expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`));
    expect(requests(backend, "session.command").map((r) => r.params)).toMatchObject([{ id: s.id, command: { command: "prompt", text: "carry on with the migration" } }]);
    expect(requests(backend, "sessions.continue")).toHaveLength(0);
    expect(requests(backend, "sessions.launch")).toHaveLength(0);
    backend.dispose();
  });

  it("a conversation that is running in a session says so and opens that session instead of starting a second one", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const s = sessionByName(backend, "migrate-auth-middleware"); // carries native-7, needs you
    const { dlg, history } = await openDialog(user, backend);
    await pickConversation(user, dlg, /Refactor invoice totals/);
    const note = within(dlg).getByTestId("ns-owner");
    expect(note).toHaveTextContent("This conversation is running in your session “migrate-auth-middleware”. Open it instead of starting a second one.");
    expect(within(dlg).queryByRole("button", { name: /Continue session|Start session/ })).toBeNull();
    await user.click(within(dlg).getByRole("button", { name: /^Open session/ }));
    await waitFor(() => expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`));
    expect(requests(backend, "sessions.launch")).toHaveLength(0);
    expect(requests(backend, "sessions.continue")).toHaveLength(0);
  });

  it("a conversation that exists only in the agent's own storage still launches a new session that resumes it", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { dlg } = await openDialog(user, backend);
    await pickConversation(user, dlg, /Fix Stripe webhook/);
    expect(within(dlg).queryByTestId("ns-owner")).toBeNull();
    await user.click(within(dlg).getByRole("button", { name: /Continue session/ }));
    await waitFor(() => expect(requests(backend, "sessions.launch")).toHaveLength(1));
    expect(requests(backend, "sessions.launch")[0].params).toMatchObject({ resume: { kind: "resume", native_session_id: "c-term-1" } });
    expect(requests(backend, "sessions.continue")).toHaveLength(0);
  });

  it("an ended session of that conversation that cannot be continued says why and cannot be started", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const s = sessionByName(backend, "release-notes-1-4");
    await backend.request("sessions.stop", { id: s.id }, { host: s.host });
    act(() => backend.patchSession(s.host, s.id, { capabilities: { ...backend.snapshot(s.host, s.id).capabilities, resume: false } }));
    const { dlg } = await openDialog(user, backend);
    await pickConversation(user, dlg, /Spike: Postgres/);
    const why = "This agent cannot pick a conversation back up.";
    expect(within(dlg).getByTestId("ns-owner")).toHaveTextContent(why);
    expect(within(dlg).getByRole("button", { name: /Continue session/ })).toBeDisabled();
    expect(dlg).toHaveTextContent(why);
  });
});

describe("new session: name", () => {
  it("a name typed in the dialog is the session's name at creation, and the session shows it at once", async () => {
    const user = userEvent.setup();
    const backend = quietBackend({ speed: 1e6 });
    const { dlg, history } = await openDialog(user, backend);
    await user.type(within(dlg).getByLabelText("Name"), "  Health check  ");
    await user.type(within(dlg).getByLabelText(/First message/), "Add a health check");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    await waitFor(() => expect(history.store.get()).toMatch(/^\/session\/x1\//));
    expect(requests(backend, "sessions.launch")[0].params).toMatchObject({ name: "Health check", first_prompt: "Add a health check" });
    expect(await screen.findByRole("heading", { name: "Health check" })).toBeInTheDocument();
    backend.dispose();
  });

  it("without a name the request carries none (the first message names the session, as before)", async () => {
    const user = userEvent.setup();
    const backend = quietBackend({ speed: 1e6 });
    const { dlg, history } = await openDialog(user, backend);
    await user.type(within(dlg).getByLabelText(/First message/), "Add a health check");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    await waitFor(() => expect(history.store.get()).toMatch(/^\/session\/x1\//));
    expect(requests(backend, "sessions.launch")[0].params).toMatchObject({ name: null });
    expect(await screen.findByRole("heading", { name: "Add a health check" })).toBeInTheDocument();
    backend.dispose();
  });

  it("in a workstream design the name is the node's name: one name, typed once", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { history } = await renderApp("/inbox", backend);
    act(() => history.push("/workstreams/ws-docs"));
    await screen.findByTestId("graph-canvas", {}, { timeout: 4000 });
    await user.click((await screen.findAllByRole("button", { name: /^Add agent/ }))[0]);
    const dlg = await screen.findByRole("dialog", { name: /Add agent to Docs search revamp/ });
    await waitFor(() => expect(within(dlg).getByRole("radio", { name: /Claude Code/ })).toHaveAttribute("aria-checked", "true"));
    await waitFor(() => expect(within(dlg).getByLabelText("Project folder")).not.toHaveValue(""));
    expect(within(dlg).getByText(/The agent's name in this workstream/)).toBeInTheDocument();
    await user.type(within(dlg).getByLabelText("Name"), "Index Builder");
    await user.type(within(dlg).getByLabelText(/First message/), "Build the index from the docs");
    await user.click(within(dlg).getByRole("button", { name: "Add to design" }));
    await waitFor(() => expect(requests(backend, "workstreams.add_node")).toHaveLength(1));
    expect(requests(backend, "workstreams.add_node")[0].params).toMatchObject({ workstream: "ws-docs", node: { name: "Index Builder", id: "index-builder" } });
  });
});

describe("new session: a folder the host refuses", () => {
  it("shows the host's message next to the folder field; the dialog stays open with everything typed, and no toast says it twice", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { dlg, history } = await openDialog(user, backend);
    const folder = within(dlg).getByLabelText("Project folder");
    await user.clear(folder);
    await user.type(folder, "src/project");
    await user.type(within(dlg).getByLabelText("Name"), "Typed name");
    await user.type(within(dlg).getByLabelText(/First message/), "Typed message");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    const err = await within(dlg).findByTestId("ns-cwd-error");
    expect(err).toHaveTextContent(/^working directory "src\/project" is not an absolute path \(a leading ~ stands for the home directory of the user the host runs as; ~user and relative paths are not accepted\)$/);
    expect(folder).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("dialog", { name: "New session" })).toBeInTheDocument();
    expect(within(dlg).getByLabelText("Name")).toHaveValue("Typed name");
    expect(within(dlg).getByLabelText(/First message/)).toHaveValue("Typed message");
    expect(screen.queryByText("Could not start the session")).toBeNull();
    expect(history.store.get()).toBe("/inbox");
    // Editing the folder takes the message away; a folder that does not exist is refused the same way, naming the path as typed.
    await user.type(folder, "x");
    expect(within(dlg).queryByTestId("ns-cwd-error")).toBeNull();
    await user.clear(folder);
    await user.type(folder, "~/src/nope");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    expect(await within(dlg).findByTestId("ns-cwd-error")).toHaveTextContent('working directory "~/src/nope" does not exist (looked for "/home/dev/src/nope")');
    // A real one starts the session.
    await user.clear(folder);
    await user.type(folder, "~/src/ledgerline");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    await waitFor(() => expect(history.store.get()).toMatch(/^\/session\/x1\//));
    expect(requests(backend, "sessions.launch")).toHaveLength(3);
  });

  it("a refusal for another reason is still a toast, not a message on the folder", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { dlg } = await openDialog(user, backend);
    backend.request = ((m: string, p?: unknown, o?: object) => (m === "sessions.launch" ? Promise.reject({ code: "spawn_failed", message: "could not start the session: no such binary" }) : Object.getPrototypeOf(backend).request.call(backend, m, p, o))) as typeof backend.request;
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    expect(await screen.findByText("Could not start the session")).toBeInTheDocument();
    expect(within(dlg).queryByTestId("ns-cwd-error")).toBeNull();
  });
});
