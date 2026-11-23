import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { hold, quietBackend, renderApp, sessionByName } from "../helpers";

type Ref = ReturnType<typeof sessionByName>;

async function open(name: string, backend: FixtureBackend = quietBackend()) {
  const s = sessionByName(backend, name);
  const r = await renderApp(`/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name });
  return { ...r, s };
}
const phase = (b: FixtureBackend, s: Ref) => b.snapshot(s.host, s.id).phase.state;
const requests = (b: FixtureBackend, method: string) => b.requests.filter((r) => r.method === method);
const commands = (b: FixtureBackend) => requests(b, "session.command").map((r) => (r.params as { command: { command: string } }).command.command);
const stop = (b: FixtureBackend, s: Ref) => act(async () => void (await b.request("sessions.stop", { id: s.id }, { host: s.host })));

describe("session header: Interrupt keeps the agent running, End session ends the process and keeps the session", () => {
  it("a running session offers Interrupt and End session, and no Stop and no Continue", async () => {
    await open("csv-schema-worker");
    expect(screen.getByRole("button", { name: "Interrupt" })).toHaveAttribute("title", "Cancel the current turn; the agent keeps running");
    expect(screen.getByRole("button", { name: "End session" })).toHaveAttribute("title", "Shut the agent down. The session is kept and can be continued.");
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("Interrupt is enabled only while a turn runs, shows pending until the host answers, and the agent keeps running", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s } = await open("csv-schema-worker", backend); // working
    const h = hold(backend, "session.command");
    const btn = screen.getByRole("button", { name: "Interrupt" });
    expect(btn).toBeEnabled();
    await user.click(btn);
    await waitFor(() => expect(screen.getByRole("button", { name: "Interrupt" })).toHaveAttribute("aria-busy", "true"));
    expect(screen.getByRole("button", { name: "Interrupt" })).toBeDisabled();
    expect(phase(backend, s)).toBe("working"); // nothing is shown that the host has not confirmed
    h.release();
    await waitFor(() => expect(phase(backend, s)).toBe("idle"));
    expect(screen.getByRole("button", { name: "Interrupt" })).toBeDisabled(); // no turn left to interrupt
    expect(screen.getByRole("button", { name: "End session" })).toBeEnabled(); // and the agent is still running
    expect(commands(backend)).toEqual(["interrupt"]);
  });

  it("End session shuts the process down: the session is Ended, kept, and offers Continue and Delete instead", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s } = await open("docs-search-index", backend);
    await user.click(screen.getByRole("button", { name: "End session" }));
    await waitFor(() => expect(phase(backend, s)).toBe("exited"));
    expect(await screen.findByRole("button", { name: "Continue" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "End session" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Interrupt" })).toBeNull();
    expect(document.querySelector('.sd-title [data-status="ended"]')).toHaveTextContent("Ended");
    expect(requests(backend, "sessions.stop")).toHaveLength(1);
  });

  it("Continue calls sessions.continue, shows pending, and the session starts again with its transcript", async () => {
    const user = userEvent.setup();
    const backend = quietBackend({ speed: 1e6 });
    const { s } = await open("docs-search-index", backend);
    await stop(backend, s);
    const h = hold(backend, "sessions.continue");
    await user.click(await screen.findByRole("button", { name: "Continue" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Continue" })).toHaveAttribute("aria-busy", "true"));
    h.release();
    await waitFor(() => expect(phase(backend, s)).toBe("starting"));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Continue" })).toBeNull());
    expect(document.querySelector(".sd-title .st")).toHaveTextContent("Starting"); // the phase the daemon reports, in its word
    expect(screen.getByRole("button", { name: "End session" })).toBeInTheDocument();
    expect(requests(backend, "sessions.continue")).toHaveLength(1);
    expect(requests(backend, "sessions.launch")).toHaveLength(0);
    backend.dispose();
  });

  it("a failed Continue shows the daemon's reason", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s } = await open("docs-search-index", backend);
    await stop(backend, s);
    backend.failNextContinue(s.host, s.id, "the agent binary was not found");
    await user.click(await screen.findByRole("button", { name: "Continue" }));
    expect(await screen.findByText("Could not continue the session")).toBeInTheDocument();
    expect(screen.getByText(/the agent binary was not found/)).toBeInTheDocument();
    expect(phase(backend, s)).toBe("exited");
  });

  it("an ended session the daemon cannot continue: Continue is disabled with the reason, and so is the message box, in the same words", async () => {
    const backend = quietBackend();
    const { s } = await open("docs-search-index", backend);
    await stop(backend, s);
    act(() => backend.patchSession(s.host, s.id, { capabilities: { ...backend.snapshot(s.host, s.id).capabilities, resume: false } }));
    const why = "This agent cannot pick a conversation back up.";
    expect(await screen.findByRole("button", { name: "Continue" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Continue" })).toHaveAttribute("title", why);
    expect(screen.getByTestId("continue-why")).toHaveTextContent(why);
    const box = screen.getByRole("textbox", { name: "Message" });
    expect(box).toBeDisabled();
    expect(box).toHaveAttribute("placeholder", why);
    expect(screen.getByTestId("composer-ended")).toHaveTextContent(`Cannot send: ${why}`);
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });

  it("while the daemon is still rebuilding the session, Continue says so, disabled", async () => {
    const backend = quietBackend();
    const { s } = await open("docs-search-index", backend);
    await stop(backend, s);
    act(() => backend.patchSession(s.host, s.id, { loaded: false }));
    expect(await screen.findByRole("button", { name: "Continue" })).toBeDisabled();
    expect(screen.getByTestId("continue-why")).toHaveTextContent("The session is still loading; wait for it to finish.");
  });
});

describe("a prompt to an ended session", () => {
  it("is the ordinary prompt: the daemon continues the session and delivers it, and the page shows only what the daemon reports", async () => {
    const user = userEvent.setup();
    const backend = quietBackend({ speed: 1e6 });
    const { s } = await open("docs-search-index", backend);
    await stop(backend, s);
    const box = screen.getByRole("textbox", { name: "Message" });
    await waitFor(() => expect(box).toHaveAttribute("placeholder", "This session has ended; sending a message continues it"));
    expect(box).toBeEnabled();
    const h = hold(backend, "session.command");
    await user.type(box, "pick up where you left off{Enter}");
    // Not sent as far as the app knows: the text stays until the host has taken it, and nothing says "sent".
    await waitFor(() => expect(h.pending()).toBe(1));
    expect(box).toHaveValue("pick up where you left off");
    expect(box).toHaveAttribute("readonly");
    expect(screen.getByRole("button", { name: "Send" })).toHaveAttribute("aria-busy", "true");
    expect(screen.queryByText("pick up where you left off", { selector: ".msg-user *" })).toBeNull();
    h.release();
    await waitFor(() => expect(box).toHaveValue(""));
    // The daemon continued the session (starting) and journaled the prompt behind it, queued until the agent is ready.
    expect(phase(backend, s)).toBe("starting");
    const bubble = (await screen.findByText("pick up where you left off")).closest(".msg-user") as HTMLElement;
    expect(bubble).toHaveAttribute("data-delivery", "queued");
    expect(commands(backend)).toEqual(["prompt"]); // one command; no separate continue from the app
    expect(requests(backend, "sessions.continue")).toHaveLength(0);
    backend.dispose();
  });

  it("when the daemon cannot continue it, the message and the reason stay in the box", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s } = await open("docs-search-index", backend);
    await stop(backend, s);
    backend.failNextContinue(s.host, s.id, "the agent binary was not found");
    const box = screen.getByRole("textbox", { name: "Message" });
    await user.type(box, "are you there{Enter}");
    const note = await screen.findByTestId("composer-failure");
    expect(note).toHaveTextContent(/the session had ended and could not be continued, so nothing was sent: the agent binary was not found/);
    expect(note).toHaveTextContent("Your message is still here.");
    expect(box).toHaveValue("are you there");
    expect(box).not.toHaveAttribute("readonly");
    expect(screen.queryByText("Could not send your message")).toBeNull(); // said once, where the user is looking
    expect(screen.queryByText("are you there", { selector: ".msg-user *" })).toBeNull();
    // Editing the message dismisses the note; sending again works (the failure was one-off).
    await user.type(box, "?");
    expect(screen.queryByTestId("composer-failure")).toBeNull();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(box).toHaveValue(""));
  });

  it("the placeholder of a running session is the plain one", async () => {
    await open("docs-search-index");
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveAttribute("placeholder", "Message the agent");
  });
});

describe("Esc on a session page, innermost first", () => {
  const esc = (user: ReturnType<typeof userEvent.setup>) => user.keyboard("{Escape}");

  it("a turn is running: Esc interrupts it, shows the button's pending state, and does not leave the page", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s, history } = await open("csv-schema-worker", backend); // working
    const h = hold(backend, "session.command");
    await esc(user);
    await waitFor(() => expect(h.pending()).toBe(1));
    expect(screen.getByRole("button", { name: "Interrupt" })).toHaveAttribute("aria-busy", "true");
    // The same key again while it is in flight is swallowed: nothing more is sent, and the page does not go back.
    await esc(user);
    expect(h.pending()).toBe(1);
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`);
    h.release();
    await waitFor(() => expect(phase(backend, s)).toBe("idle"));
    expect(commands(backend)).toEqual(["interrupt"]);
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`);
  });

  it("a turn is running and the message box has the focus: Esc still interrupts, once, and does not blur-and-leave", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s, history } = await open("csv-schema-worker", backend);
    const box = screen.getByRole("textbox", { name: "Message" });
    await user.click(box);
    await user.type(box, "draft");
    await esc(user);
    await waitFor(() => expect(commands(backend)).toEqual(["interrupt"]));
    expect(box).toHaveValue("draft");
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`);
  });

  it("the session waits for your decision: Esc never interrupts (that would decline what it asks); it goes back to the Inbox", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { history } = await open("migrate-auth-middleware", backend); // needs_you
    await esc(user);
    await waitFor(() => expect(history.store.get()).toBe("/inbox"));
    expect(commands(backend)).toEqual([]);
  });

  it("nothing runs: with the message box focused Esc leaves it (and does not navigate); the next Esc goes back to the Inbox", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s, history } = await open("docs-search-index", backend); // idle
    const box = screen.getByRole("textbox", { name: "Message" });
    await user.click(box);
    expect(document.activeElement).toBe(box);
    await esc(user);
    expect(document.activeElement).not.toBe(box);
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`);
    await esc(user);
    await waitFor(() => expect(history.store.get()).toBe("/inbox"));
    expect(commands(backend)).toEqual([]);
  });

  it("an open menu takes the key first: Esc closes the model menu of a running session and interrupts nothing", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s, history } = await open("csv-schema-worker", backend);
    await user.click(screen.getByRole("button", { name: /^Model: / }));
    expect(screen.getByRole("listbox", { name: "Model" })).toBeInTheDocument();
    await esc(user);
    expect(screen.queryByRole("listbox", { name: "Model" })).toBeNull();
    expect(commands(backend)).toEqual([]);
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`);
  });

  it("the slash menu takes the key first: Esc closes it, keeps the text and the focus, interrupts nothing", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s, history } = await open("csv-schema-worker", backend);
    const box = screen.getByRole("textbox", { name: "Message" });
    await user.type(box, "/i");
    expect(screen.getByRole("listbox", { name: "Slash commands" })).toBeInTheDocument();
    await esc(user);
    expect(screen.queryByRole("listbox", { name: "Slash commands" })).toBeNull();
    expect(box).toHaveValue("/i");
    expect(document.activeElement).toBe(box);
    expect(commands(backend)).toEqual([]);
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`);
    // Typing again brings the menu back; then the key is the page's again.
    await user.type(box, "n");
    expect(screen.getByRole("listbox", { name: "Slash commands" })).toBeInTheDocument();
  });

  it("the inline rename editor takes the key first", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s, history } = await open("csv-schema-worker", backend);
    await user.click(screen.getByRole("button", { name: "Rename session" }));
    expect(screen.getByRole("textbox", { name: "Session name" })).toBeInTheDocument();
    await esc(user);
    expect(screen.queryByRole("textbox", { name: "Session name" })).toBeNull();
    expect(commands(backend)).toEqual([]);
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`);
  });

  it("in the terminal Esc is the shell's: a running turn is not interrupted and nothing blurs or navigates", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s, history } = await open("csv-schema-worker", backend);
    const box = document.createElement("div"); // what the Terminal tab mounts xterm.js into
    box.className = "xterm-box";
    const helper = document.createElement("textarea");
    box.append(helper);
    document.body.append(box);
    helper.focus();
    await esc(user);
    expect(commands(backend)).toEqual([]);
    expect(document.activeElement).toBe(helper);
    expect(history.store.get()).toBe(`/session/${s.host}/${s.id}`);
    box.remove();
  });

  it("an ended session: Esc goes back to the Inbox (there is nothing to interrupt)", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const { s, history } = await open("docs-search-index", backend);
    await stop(backend, s);
    await screen.findByRole("button", { name: "Continue" });
    await esc(user);
    await waitFor(() => expect(history.store.get()).toBe("/inbox"));
  });

  it("the shortcut sheet says the rule", async () => {
    const user = userEvent.setup();
    await open("docs-search-index");
    await user.keyboard("?");
    const help = await screen.findByRole("dialog", { name: "Keyboard shortcuts" });
    expect(within(help).getByText("Close the open menu, dialog, preview or editor (before anything else)")).toBeInTheDocument();
    expect(within(help).getByText("Interrupt the turn that is running")).toBeInTheDocument();
    expect(within(help).getByText("Leave the text field you are typing in")).toBeInTheDocument();
    expect(within(help).getByText("Back to the Inbox (when no turn is running)")).toBeInTheDocument();
  });
});

describe("Inbox: ended sessions are listed apart from idle ones", () => {
  const rowOf = (b: FixtureBackend, name: string) => document.querySelector(`[data-session="${sessionByName(b, name).key}"]`) as HTMLElement | null;

  it("an ended session leaves Idle for its own group, after it, with Continue on its row; the summary and the filter count it", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    const idleBefore = Number(/(\d+) idle/.exec(document.body.textContent!)?.[1]);
    expect(rowOf(backend, "docs-search-index")).toHaveAttribute("data-status", "idle");
    expect(document.body.textContent).toMatch(/0 ended/);
    await stop(backend, sessionByName(backend, "docs-search-index"));
    await waitFor(() => expect(rowOf(backend, "docs-search-index")).toHaveAttribute("data-status", "ended"));
    const list = screen.getByRole("list", { name: "Sessions" });
    const groups = [...list.querySelectorAll(".grp")].map((g) => g.textContent);
    expect(groups.findIndex((t) => /^Idle/.test(t!))).toBeGreaterThanOrEqual(0);
    expect(groups.findIndex((t) => /^Ended/.test(t!))).toBeGreaterThan(groups.findIndex((t) => /^Idle/.test(t!)));
    expect(document.body.textContent).toMatch(new RegExp(`${idleBefore - 1} idle`));
    expect(document.body.textContent).toMatch(/1 ended/);
    const row = rowOf(backend, "docs-search-index")!;
    expect(within(row).getByRole("button", { name: "Continue" })).toBeEnabled();
    expect(within(row).getByText("Ended")).toBeInTheDocument();
    // Continue from the row.
    await user.click(within(row).getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(requests(backend, "sessions.continue")).toHaveLength(1));
    // The Status filter offers it.
    await user.click(screen.getByRole("button", { name: /^Status: / }));
    expect(screen.getByRole("option", { name: "Ended" })).toBeInTheDocument();
  });

  it("an ended session that finished work you have not seen stays under Finished until seen, with Continue there; then it is Ended", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    expect(rowOf(backend, "tile-cache-eviction")).toHaveAttribute("data-status", "done");
    const s = sessionByName(backend, "tile-cache-eviction");
    await stop(backend, s);
    await waitFor(() => expect(phase(backend, s)).toBe("exited"));
    const row = () => rowOf(backend, "tile-cache-eviction")!;
    expect(row()).toHaveAttribute("data-status", "done");
    expect(within(row()).getByRole("button", { name: "Continue" })).toBeEnabled();
    await user.click(within(row()).getByRole("button", { name: /Mark seen/ }));
    await waitFor(() => expect(row()).toHaveAttribute("data-status", "ended"));
    expect(within(row()).getByRole("button", { name: "Continue" })).toBeEnabled();
    expect(within(row()).queryByRole("button", { name: /Mark seen/ })).toBeNull();
  });

  it("the host's session list and the workstream node say Ended too", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, "docs-search-index"); // on x1, in no workstream
    const { history } = await renderApp("/inbox", backend);
    await stop(backend, s);
    act(() => history.push(`/hosts?h=${s.host}`));
    const here = (await screen.findByText("Sessions here")).parentElement as HTMLElement;
    const line = await within(here).findByRole("button", { name: /docs-search-index/ });
    expect(line).toHaveTextContent("Ended");
    expect(line).not.toHaveTextContent("Idle");
    // A workstream node whose session ended.
    const n = sessionByName(backend, "csv-schema-worker");
    await stop(backend, n);
    act(() => history.push("/workstreams/ws-invoice"));
    await screen.findByTestId("graph-canvas", {}, { timeout: 4000 });
    await waitFor(() => expect(document.querySelector('[data-node="csv"]')).toHaveAttribute("aria-label", expect.stringMatching(/: Ended$/)));
    expect(document.querySelector('[data-node="csv"] .st')).toHaveClass("ended");
  });

  it("a failed session's row says Continue (it continues the session; it re-runs nothing)", async () => {
    await renderApp("/inbox");
    expect(screen.queryByRole("button", { name: "Retry last step" })).toBeNull();
    const row = document.querySelector('[data-status="failed"]') as HTMLElement;
    expect(within(row).getByRole("button", { name: "Continue" })).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: "Open log" })).toBeInTheDocument();
  });

  it("the palette says which sessions are ended", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    await stop(backend, sessionByName(backend, "docs-search-index"));
    await user.keyboard("{Control>}k{/Control}");
    const pal = screen.getByRole("dialog", { name: "Command palette" });
    await user.type(within(pal).getByRole("combobox"), "docs-search");
    expect(await within(pal).findByRole("option", { name: /docs-search-index.*Ended/ })).toBeInTheDocument();
  });
});

describe("open in your own terminal", () => {
  it("shows the command the daemon gave, quoted for the shell, and its folder, each with a copy button", async () => {
    const user = userEvent.setup();
    const written: string[] = [];
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (t: string) => void written.push(t) } });
    const backend = quietBackend();
    const { s } = await open("migrate-auth-middleware", backend);
    act(() => backend.patchSession(s.host, s.id, { cwd: "/home/me/my project" }));
    const side = await screen.findByTestId("side-own-terminal");
    expect(within(side).getByRole("heading", { name: "Open in your own terminal" })).toBeInTheDocument();
    expect(within(side).getByTestId("own-command")).toHaveTextContent("claude --resume native-7");
    expect(within(side).getByTestId("own-folder")).toHaveTextContent("/home/me/my project");
    expect(side).toHaveTextContent("This opens the same conversation in the agent’s own program. The agent’s own session list may not show conversations started by other tools.");
    await user.click(within(side).getByRole("button", { name: "Copy the command" }));
    await user.click(within(side).getByRole("button", { name: "Copy the folder" }));
    expect(written).toEqual(["claude --resume native-7", "/home/me/my project"]);
  });

  it("shows nothing when the daemon has no command for the conversation", async () => {
    await open("docs-search-index"); // an agent with no command of its own
    expect(screen.queryByTestId("side-own-terminal")).toBeNull();
    expect(screen.queryByText(/own terminal/i)).toBeNull();
  });
});
