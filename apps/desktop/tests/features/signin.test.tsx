import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { AuthPrompt, LinkState } from "../../src/bridge/wire";
import { linkView, NO_SCREEN, reconcileSignInScreen, signInRepeatWords, type SignInQuestion, type SignInScreen } from "../../src/state";
import { hold, mountApp, quietBackend, renderApp } from "../helpers";

const strip = () => screen.queryByTestId("signin-strip");
const dialog = (host: string) => screen.queryByRole("dialog", { name: `Sign in to ${host}` });
const statusBar = () => screen.getByRole("contentinfo", { name: "Status" });
const authAnswers = (b: ReturnType<typeof quietBackend>) => b.requests.filter((r) => r.method === "hosts.auth_answer").map((r) => r.params as { host: string; id: number; answer: string });
const reconnects = (b: ReturnType<typeof quietBackend>) => b.requests.filter((r) => r.method === "hosts.reconnect").map((r) => r.params);

/**
 * The app on `path`. `second`: the fixture's connected host hetzner is an interactive one too (what the hub learns from a
 * connect that asked) and has just dropped, so the hub holds it for the user like gpu-box.
 */
async function app(path = "/inbox", { second = false } = {}) {
  const backend = quietBackend();
  if (second) backend.markInteractive("hz");
  const r = await renderApp("/inbox", backend);
  if (second) act(() => backend.dropLink("hz", "connection reset by peer"));
  if (path !== "/inbox") act(() => r.history.push(path));
  return { ...r, backend };
}

describe("a host that waits for the user to sign in", () => {
  it("is said everywhere it shows, and nothing is asked of the host until the user clicks", async () => {
    const { backend, history } = await app();
    // Inbox: compact one-line note naming disconnected hosts, with Sign in for waiting hosts and a link to Hosts (no error dump).
    const group = await screen.findByTestId("waiting-hosts");
    expect(group).toHaveTextContent(/gpu-box/);
    expect(within(group).getByRole("button", { name: "Sign in" })).toBeEnabled();
    expect(within(group).getByRole("link", { name: "Details on Hosts" })).toHaveAttribute("href", "#/hosts");
    // The strip stays out of the Inbox's way; the bell and the Hosts item count the host, the Inbox item counts sessions.
    expect(strip()).toBeNull();
    const rail = screen.getByRole("navigation", { name: "Main" });
    expect(within(rail).getByLabelText("1 host needs you to sign in")).toBeInTheDocument();
    expect(within(rail).getByLabelText(/^\d+ need you$/).textContent).not.toBe("");
    const needs = Number(within(rail).getByLabelText(/^\d+ need you$/).textContent);
    expect(within(screen.getByRole("button", { name: "Notifications" })).getByText(String(needs + 1))).toBeInTheDocument();
    // The status bar says it in the host's own chip.
    const chip = within(statusBar()).getByText(/gpu-box · needs you to sign in/);
    expect(chip.closest("button")).toHaveAttribute("data-held");
    // Any other page: one strip, with the same click.
    act(() => history.push("/config"));
    const s = await screen.findByTestId("signin-strip");
    expect(s).toHaveTextContent("gpu-box needs you to sign in.");
    expect(s).toHaveTextContent("Marsad will not reconnect it on its own: signing in needs you.");
    expect(within(s).getByRole("button", { name: "Sign in" })).toBeEnabled();
    // The hosts page has its own: the row says it, the detail says why and what happens to the sessions.
    act(() => history.push("/hosts?h=gpu"));
    const panel = await screen.findByRole("complementary", { name: "gpu-box details" });
    const held = within(panel).getByTestId("host-held");
    expect(held).toHaveTextContent("Needs you to sign in");
    expect(held).toHaveTextContent("Marsad will not reconnect it on its own: signing in needs you.");
    expect(held).not.toHaveTextContent(/reconnecting/i);
    expect(strip()).toBeNull();
    // Not one request went out: the hub holds it until the click.
    expect(reconnects(backend)).toEqual([]);
    expect(authAnswers(backend)).toEqual([]);
  });

  it("one click calls hosts.reconnect once: pending while the hub takes it, its refusal shown, then the dialog", async () => {
    const user = userEvent.setup();
    const { backend, history } = await app("/config");
    const h = hold(backend, "hosts.reconnect");
    const click = within(await screen.findByTestId("signin-strip")).getByRole("button", { name: "Sign in" });
    await user.click(click);
    await waitFor(() => expect(h.pending()).toBe(1));
    expect(click).toHaveAttribute("aria-busy", "true");
    await user.click(click);
    expect(h.pending()).toBe(1);
    act(() => h.reject({ code: "internal", message: "the hub could not start the attempt" }));
    expect(await screen.findByText("Could not connect to gpu-box")).toBeInTheDocument();
    expect(screen.getByText("the hub could not start the attempt")).toBeInTheDocument();
    await waitFor(() => expect(click).not.toHaveAttribute("aria-busy"));
    expect(dialog("gpu-box")).toBeNull();
    // Healed: the same click starts the attempt and the host's question follows.
    h.healAll();
    await user.click(within(screen.getByTestId("signin-strip")).getByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(h.pending()).toBe(1));
    act(() => h.release());
    expect(await screen.findByRole("dialog", { name: "Sign in to gpu-box" })).toBeInTheDocument();
    // (The refused request never reached the fixture; the second did, once.)
    expect(reconnects(backend)).toEqual([{ id: "gpu" }]);
    void history;
  });

  it("two hosts: one strip naming both with one action each; a dropped host keeps its sessions listed as last known with the cause", async () => {
    const { backend, history } = await app("/config", { second: true });
    const s = await screen.findByTestId("signin-strip");
    await waitFor(() => expect(s).toHaveTextContent("2 hosts need you to sign in."));
    expect(s).toHaveTextContent("hetzner-ax41, gpu-box");
    expect(within(s).getByRole("button", { name: "Sign in to gpu-box" })).toBeInTheDocument();
    expect(within(s).getByRole("button", { name: "Sign in to hetzner-ax41" })).toBeInTheDocument();
    act(() => history.push("/inbox"));
    const group = await screen.findByTestId("waiting-hosts");
    await waitFor(() => expect(group).toHaveTextContent(/hetzner-ax41/));
    expect(group).not.toHaveTextContent("connection reset by peer");
    expect(within(group).getByRole("button", { name: "Sign in to gpu-box" })).toBeEnabled();
    expect(within(group).getByRole("button", { name: "Sign in to hetzner-ax41" })).toBeEnabled();
    expect(within(group).getByRole("link", { name: "Details on Hosts" })).toHaveAttribute("href", "#/hosts");
    // Its sessions stay listed, as last known.
    expect(document.querySelectorAll('[data-session^="hz/"][data-stale]').length).toBeGreaterThan(0);
    act(() => history.push("/hosts?h=hz"));
    const panel = await screen.findByRole("complementary", { name: "hetzner-ax41 details" });
    expect(within(panel).getByTestId("host-held")).toHaveTextContent("The connection to hetzner-ax41 dropped: connection reset by peer. Marsad will not reconnect it on its own");
    expect(within(panel).getByTestId("held-sessions")).toHaveTextContent(/keep running there and reattach once you sign in/);
    expect(within(panel).queryByRole("button", { name: "Retry now" })).toBeNull();
    expect(reconnects(backend)).toEqual([]);
  });

  it("a host the hub does reconnect by itself is not waiting for the user", async () => {
    const { backend } = await app("/config");
    act(() => backend.dropLink("hz", "connection reset by peer"));
    await waitFor(() => expect(within(statusBar()).getByText(/hetzner-ax41 · reconnecting/)).toBeInTheDocument());
    expect(screen.getByTestId("signin-strip")).toHaveTextContent("gpu-box needs you to sign in.");
    expect(screen.getByTestId("signin-strip")).not.toHaveTextContent("hetzner");
  });

  it("when the user cancels, the host is shown as what it is, without the will-not-retry words, and the strip goes", async () => {
    const user = userEvent.setup();
    const { backend } = await app("/hosts?h=gpu");
    const panel = await screen.findByRole("complementary", { name: "gpu-box details" });
    await user.click(within(panel).getByRole("button", { name: "Sign in" }));
    await user.click(await screen.findByRole("button", { name: "Cancel sign-in" }));
    await waitFor(() => expect(dialog("gpu-box")).toBeNull());
    await waitFor(() => expect(panel.querySelector(".gate")).toHaveTextContent("Sign-in did not succeed"));
    const card = panel.querySelector(".gate")!;
    expect(card).toHaveTextContent("Sign-in was cancelled.");
    expect(card).not.toHaveTextContent(/will not reconnect/);
    expect(within(panel).queryByTestId("host-held")).toBeNull();
    expect(within(panel).getByRole("button", { name: "Connect" })).toBeEnabled();
    expect(backend.requests.filter((r) => r.method === "hosts.auth_cancel")).toHaveLength(1);
    // Not held: not counted, no strip on any page.
    act(() => (document.querySelector('a[href="#/config"]') as HTMLElement).click());
    await waitFor(() => expect(screen.getByRole("navigation", { name: "Main" })).not.toHaveTextContent(/need you to sign in/));
    expect(strip()).toBeNull();
    expect(screen.queryByLabelText(/host needs you to sign in/)).toBeNull();
  });

  it("is not shown while the app has no live link to its daemon: that link is the last known", async () => {
    const { backend } = await app("/config");
    await screen.findByTestId("signin-strip");
    act(() => backend.setHubLink({ state: "reconnecting", error: "connection refused" }));
    await waitFor(() => expect(strip()).toBeNull());
    act(() => backend.setHubLink({ state: "connected" }));
    expect(await screen.findByTestId("signin-strip")).toBeInTheDocument();
  });
});

describe("the sign-in dialog", () => {
  async function open(host = "gpu-box", second = false) {
    const user = userEvent.setup();
    const r = await app("/config", { second });
    const strip = await screen.findByTestId("signin-strip");
    await user.click(await within(strip).findByRole("button", { name: second ? "Sign in to gpu-box" : "Sign in" }));
    const dlg = await screen.findByRole("dialog", { name: `Sign in to ${host}` });
    return { ...r, user, dlg };
  }

  it("cannot be dismissed by Esc or by a click outside, and no key press reaches Cancel", async () => {
    const { user, dlg, backend } = await open();
    expect(within(dlg).queryByRole("button", { name: "Close" })).toBeNull();
    await user.keyboard("{Escape}");
    fireEvent.keyDown(document.body, { key: "Escape" });
    fireEvent.mouseDown(dlg.parentElement!);
    fireEvent.click(dlg.parentElement!);
    await new Promise((r) => setTimeout(r, 30));
    expect(dialog("gpu-box")).toBeInTheDocument();
    expect(backend.requests.some((r) => r.method === "hosts.auth_cancel")).toBe(false);
    // A kind with no field (confirm, info) starts with the focus on the question, not on a button.
    // (A new question from the daemon replaces the one on screen: the dialog says the first was withdrawn, then shows the new one.)
    act(() => backend.setAuthPrompt("gpu", { prompt: "Are you sure you want to continue connecting (yes/no)?", kind: "confirm", attempt: 1 }));
    await user.click(await screen.findByRole("button", { name: "Show the next question" }));
    await waitFor(() => expect(screen.getByTestId("auth-prompt-text")).toHaveTextContent(/continue connecting/));
    await waitFor(() => expect(screen.getByTestId("auth-prompt-text")).toHaveFocus());
    await user.keyboard("{Enter}{ }");
    expect(backend.requests.some((r) => r.method === "hosts.auth_cancel" || r.method === "hosts.auth_answer")).toBe(false);
  });

  it("cancelling is an explicit button that says what it does", async () => {
    const { user, dlg, backend } = await open();
    expect(within(dlg).getByTestId("auth-cancel-note")).toHaveTextContent("Cancel sign-in ends this attempt. gpu-box does not reconnect on its own, so you will have to click Sign in again to try again.");
    await user.click(within(dlg).getByRole("button", { name: "Cancel sign-in" }));
    await waitFor(() => expect(backend.requests.filter((r) => r.method === "hosts.auth_cancel")).toHaveLength(1));
    expect(backend.requests.find((r) => r.method === "hosts.auth_cancel")?.params).toMatchObject({ host: "gpu" });
    await waitFor(() => expect(dialog("gpu-box")).toBeNull());
  });

  it("asks one host at a time, oldest first, says which and how many wait, and each answer carries its own question's id", async () => {
    const { user, dlg, backend } = await open("gpu-box", true);
    // hetzner asks as well, later: it waits its turn behind gpu-box.
    // gpu-box is being signed in: the strip now lists only hetzner, whose click is the one left.
    await user.click(await within(screen.getByTestId("signin-strip")).findByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(within(screen.getByRole("dialog")).getByTestId("auth-queue")).toHaveTextContent("1 more host is waiting after this one: hetzner-ax41."));
    expect(screen.getByRole("dialog", { name: "Sign in to gpu-box" })).toBe(dlg);
    const gpuPrompt = backend.promptOf("gpu")!;
    const hzPrompt = backend.promptOf("hz")!;
    expect(gpuPrompt.id).toBeLessThan(hzPrompt.id);
    // Answering gpu-box moves on to hetzner (its next question is newer than hetzner's).
    await user.type(within(dlg).getByLabelText("Password or passphrase"), "pw-for-gpu");
    await user.click(within(dlg).getByRole("button", { name: "Sign in" }));
    const next = await screen.findByRole("dialog", { name: "Sign in to hetzner-ax41" });
    expect(within(next).getByTestId("auth-queue")).toHaveTextContent("1 more host is waiting after this one: gpu-box.");
    expect((within(next).getByLabelText("Password or passphrase") as HTMLInputElement).value).toBe("");
    await user.type(within(next).getByLabelText("Password or passphrase"), "pw-for-hz");
    await user.click(within(next).getByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(authAnswers(backend)).toHaveLength(2));
    expect(authAnswers(backend)).toEqual([
      { host: "gpu", id: gpuPrompt.id, answer: "pw-for-gpu" },
      { host: "hz", id: hzPrompt.id, answer: "pw-for-hz" },
    ]);
  });

  it("a question the daemon withdraws is said so: the host under the cursor never changes silently, and what was typed goes nowhere", async () => {
    const { user, dlg, backend } = await open("gpu-box", true);
    await user.click(await within(screen.getByTestId("signin-strip")).findByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(within(dlg).getByTestId("auth-queue")).toBeInTheDocument());
    const typed = within(dlg).getByLabelText("Password or passphrase");
    await user.type(typed, "meant-for-gpu");
    act(() => backend.endAttempt("gpu"));
    const gone = await screen.findByTestId("auth-withdrawn");
    expect(gone).toHaveTextContent("The question from gpu-box was withdrawn.");
    expect(gone).toHaveTextContent("Its sign-in attempt ended before you answered, so nothing you typed was used and nothing was sent.");
    expect(gone).toHaveTextContent("1 more host is waiting: hetzner-ax41.");
    // The dialog is still gpu-box's: hetzner's question is not under the cursor yet.
    expect(dialog("gpu-box")).toBeInTheDocument();
    expect(dialog("hetzner-ax41")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Show the next question" }));
    const hz = await screen.findByRole("dialog", { name: "Sign in to hetzner-ax41" });
    expect((within(hz).getByLabelText("Password or passphrase") as HTMLInputElement).value).toBe("");
    expect(authAnswers(backend)).toEqual([]);
    expect(JSON.stringify(backend.requests)).not.toContain("meant-for-gpu");
    // gpu-box is held again, by the hub, and offers its click once more.
    await user.click(within(hz).getByRole("button", { name: "Cancel sign-in" }));
    await waitFor(() => expect(dialog("hetzner-ax41")).toBeNull());
  });

  it("a withdrawn question with nobody else waiting says so and closes on OK", async () => {
    const { user, backend } = await open();
    act(() => backend.endAttempt("gpu"));
    await screen.findByTestId("auth-withdrawn");
    await user.click(screen.getByRole("button", { name: "OK" }));
    await waitFor(() => expect(dialog("gpu-box")).toBeNull());
  });

  it("the daemon refuses a question it no longer holds, and the dialog shows that, not a toast", async () => {
    const { user, dlg, backend } = await open();
    const stale = backend.promptOf("gpu")!;
    await expect(backend.request("hosts.auth_answer", { host: "gpu", id: stale.id + 99, answer: "x" })).rejects.toMatchObject({ code: "not_found" });
    await expect(backend.request("hosts.auth_cancel", { host: "gpu", id: stale.id + 99 })).rejects.toMatchObject({ code: "not_found" });
    const h = hold(backend, "hosts.auth_answer");
    h.failAll({ code: "not_found", message: `not found: prompt ${stale.id} of gpu` });
    await user.type(within(dlg).getByLabelText("Password or passphrase"), "late");
    await user.click(within(dlg).getByRole("button", { name: "Sign in" }));
    const refused = await within(dlg).findByTestId("auth-refused");
    expect(refused).toHaveTextContent("That question is no longer pending: its sign-in attempt ended, so your answer was not used.");
    expect(screen.queryByText("Could not send your answer")).toBeNull();
    // What was typed is gone from the field; the daemon's id bound the request, so it could only ever reach that question.
    expect((within(dlg).getByLabelText("Password or passphrase") as HTMLInputElement).value).toBe("");
  });

  it("a refusal that arrives after the dialog has moved on is a toast, so it is never lost", async () => {
    const { user, dlg, backend } = await open();
    const h = hold(backend, "hosts.auth_answer");
    await user.type(within(dlg).getByLabelText("Password or passphrase"), "pw");
    await user.click(within(dlg).getByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(h.pending()).toBe(1));
    // While the answer is in flight the question is withdrawn (the attempt ended) and the dialog moves on.
    act(() => backend.endAttempt("gpu"));
    await waitFor(() => expect(dialog("gpu-box")).toBeNull());
    act(() => h.release());
    expect(await screen.findByText("Could not send your answer")).toBeInTheDocument();
    expect(screen.getByText(/not found: prompt \d+ of gpu/)).toBeInTheDocument();
  });

  it.each([
    ["helper_restart", "The host asks again because Marsad had to reconnect after updating its helper there."],
    ["id_restart", "The host asks again because Marsad had to reconnect after the host answered under a different identity."],
    ["windows_shell", "The host asks again because Marsad had to reconnect after switching to the shell Windows hosts need."],
    ["a_future_reason", "The host asks again."],
  ])("says in words, before the user answers, that a prompt is a second approval (%s)", async (repeat, words) => {
    const { backend } = await app("/config");
    act(() => backend.setAuthPrompt("gpu", { prompt: "Verification code: ", kind: "code", attempt: 1, repeat: repeat as never }));
    const note = await screen.findByTestId("auth-repeat");
    expect(note).toHaveTextContent(words);
    expect(screen.getByLabelText("One-time code")).toBeInTheDocument();
  });

  it("a first question carries no such note", async () => {
    const { backend } = await app("/config");
    act(() => backend.setAuthPrompt("gpu", { prompt: "Verification code: ", kind: "code", attempt: 1 }));
    await screen.findByLabelText("One-time code");
    expect(screen.queryByTestId("auth-repeat")).toBeNull();
  });

  it("the daemon's own repeat (a second connection within one sign-in) reaches the user as such", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    backend.repeatNext("gpu", "id_restart");
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/config"));
    await user.click(within(await screen.findByTestId("signin-strip")).getByRole("button", { name: "Sign in" }));
    await screen.findByRole("dialog", { name: "Sign in to gpu-box" });
    expect(screen.queryByTestId("auth-repeat")).toBeNull();
    await user.type(screen.getByLabelText("Password or passphrase"), "a");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    await user.type(await screen.findByLabelText("One-time code"), "123456");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    // Approved on the phone: the hub reconnects under the right id and asks again.
    await waitFor(() => expect(screen.getByTestId("auth-repeat")).toHaveTextContent("a different identity"), { timeout: 5000 });
    expect(screen.getByLabelText("Password or passphrase")).toBeInTheDocument();
  }, 10_000);

  it("masks every answer but the host-key confirmation, whatever the words the daemon's guess gave it", async () => {
    // A password that mentions a token used to be classed as a one-time code and shown in clear.
    const { user, backend } = await open();
    const dlg = () => screen.getByRole("dialog", { name: "Sign in to gpu-box" });
    act(() => backend.setAuthPrompt("gpu", { prompt: "Password for token-svc: ", kind: "code", attempt: 1 }));
    await user.click(await screen.findByRole("button", { name: "Show the next question" }));
    const field = await within(dlg()).findByLabelText("One-time code");
    expect(field).toHaveAttribute("type", "password");
    expect(within(dlg()).queryByRole("textbox")).toBeNull();
  });

  it("a question the guess calls info still takes a masked answer (no question is answered by its words alone)", async () => {
    const { user, backend } = await open();
    const dlg = () => screen.getByRole("dialog", { name: "Sign in to gpu-box" });
    act(() => backend.setAuthPrompt("gpu", { prompt: "Enter the PIN: ", kind: "info", attempt: 1 }));
    await user.click(await screen.findByRole("button", { name: "Show the next question" }));
    const field = (await within(dlg()).findByLabelText("Answer")) as HTMLInputElement;
    expect(field).toHaveAttribute("type", "password");
    await user.type(field, "4321");
    await user.click(within(dlg()).getByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(authAnswers(backend).map((a) => a.answer)).toEqual(["4321"]));
  });

  it("the host-key confirmation alone has no answer field: its buttons are yes and no", async () => {
    const { user, backend } = await open();
    const dlg = () => screen.getByRole("dialog", { name: "Sign in to gpu-box" });
    act(() => backend.setAuthPrompt("gpu", { prompt: "Are you sure you want to continue connecting (yes/no)?", kind: "confirm", attempt: 1 }));
    await user.click(await screen.findByRole("button", { name: "Show the next question" }));
    await waitFor(() => expect(within(dlg()).getByTestId("auth-prompt-text")).toHaveTextContent(/continue connecting/));
    expect(within(dlg()).queryByRole("textbox")).toBeNull();
    expect(dlg().querySelector("input[type=password]")).toBeNull();
    expect(within(dlg()).getByRole("button", { name: "Yes" })).toBeInTheDocument();
  });

  it("counts down to the daemon's own deadline, not to a guess", async () => {
    const backend = quietBackend({ now: () => Date.now() - 90_000 });
    mountApp("/config", backend);
    const user = userEvent.setup();
    await user.click(within(await screen.findByTestId("signin-strip")).getByRole("button", { name: "Sign in" }));
    const dlg = await screen.findByRole("dialog", { name: "Sign in to gpu-box" });
    // The fixture's prompt gives up 120 s after the (shifted) daemon clock: about 30 s from now.
    expect(within(dlg).getByText(/Gives up in (29|30|31)s/)).toBeInTheDocument();
  });

  it("never keeps what was typed: cleared when sent and when the question changes, absent from the store", async () => {
    const { user, dlg, backend } = await open();
    const field = within(dlg).getByLabelText("Password or passphrase") as HTMLInputElement;
    await user.type(field, "wrong");
    await user.click(within(dlg).getByRole("button", { name: "Sign in" }));
    expect(field.value).toBe("");
    expect(await screen.findByText(/That did not work. Try again \(attempt 2\)/)).toBeInTheDocument();
    const state = JSON.stringify((window as unknown as { __marsad: { state: () => unknown } }).__marsad.state());
    expect(state).not.toContain("wrong");
    expect(authAnswers(backend).map((a) => a.answer)).toEqual(["wrong"]);
    // The next question (a one-time code) starts empty, whatever was left in a field.
    const again = screen.getByLabelText("Password or passphrase") as HTMLInputElement;
    await user.type(again, "hunter2");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(((await screen.findByLabelText("One-time code")) as HTMLInputElement).value).toBe("");
  });

  it("shows a sending state while the answer is in flight and refuses a second send", async () => {
    const { user, dlg, backend } = await open();
    const h = hold(backend, "hosts.auth_answer");
    await user.type(within(dlg).getByLabelText("Password or passphrase"), "pw");
    const send = within(dlg).getByRole("button", { name: "Sign in" });
    await user.click(send);
    await waitFor(() => expect(send).toHaveAttribute("aria-busy", "true"));
    expect(within(dlg).getByRole("button", { name: "Cancel sign-in" })).toBeDisabled();
    expect(within(dlg).getByLabelText("Password or passphrase")).toHaveAttribute("readonly");
    expect(h.pending()).toBe(1);
    act(() => h.release());
    await waitFor(() => expect(within(screen.getByRole("dialog")).getByLabelText("One-time code")).toBeInTheDocument());
  });

  it("is not shown while the app has no live link to its daemon, and is back (or reported withdrawn) once it has", async () => {
    const { backend } = await open();
    act(() => backend.setHubLink({ state: "reconnecting", error: "connection refused" }));
    await waitFor(() => expect(dialog("gpu-box")).toBeNull());
    act(() => backend.setHubLink({ state: "connected" }));
    expect(await screen.findByRole("dialog", { name: "Sign in to gpu-box" })).toBeInTheDocument();
  });
});

describe("what the shell's navigate event opens", () => {
  it("a session when it names one, else the host", async () => {
    const { backend, history } = await app("/config");
    act(() => backend.navigate({ host: "gpu" }));
    expect(history.store.get()).toBe("/hosts?h=gpu");
    act(() => backend.navigate({ host: "hz", session: "abc" }));
    expect(history.store.get()).toBe("/session/hz/abc");
  });
});

describe("the words and the order", () => {
  const link = (l: Partial<Extract<LinkState, { state: "auth_needed" }>>): LinkState => ({ state: "auth_needed", problem: "auth_needed", detail: "d", ...l });
  const remote = (state: LinkState) => ({ link: "remote" as const, state });

  it("a held link is an ask; any other auth_needed is a problem, worded as one", () => {
    const held = linkView(remote(link({ held: true, cause: "reset" })));
    expect(held).toMatchObject({ key: "auth_needed", tone: "warn", word: "Needs you to sign in", held: true, connected: false });
    const plain = linkView(remote(link({})));
    expect(plain).toMatchObject({ key: "auth_needed", tone: "bad", word: "Sign-in did not succeed", detail: "d", connected: false });
    expect(plain.held).toBeUndefined();
    expect(linkView(remote(link({ held: false }))).held).toBeUndefined();
    // Last known is not an ask.
    expect(linkView({ link: "remote", state: link({ held: true }), stale: true }).key).toBe("stale");
  });

  it("one sentence for each way a prompt can be a repeat, a generic one for a way this build does not know", () => {
    expect(signInRepeatWords("helper_restart")).toBe("The host asks again because Marsad had to reconnect after updating its helper there.");
    expect(signInRepeatWords("id_restart")).toBe("The host asks again because Marsad had to reconnect after the host answered under a different identity.");
    expect(signInRepeatWords("windows_shell")).toBe("The host asks again because Marsad had to reconnect after switching to the shell Windows hosts need.");
    expect(signInRepeatWords("a_future_reason")).toBe("The host asks again.");
    expect(signInRepeatWords(undefined)).toBeNull();
    expect(signInRepeatWords(null)).toBeNull();
  });

  const q = (host: string, id: number): SignInQuestion => ({ hostName: host.toUpperCase(), prompt: { id, host, prompt: "p", kind: "secret", attempt: 1, expires_at_ms: 1 } as AuthPrompt });

  it("the screen follows the queue: oldest first, stable, silent after the user's own act, a notice after anything else", () => {
    const none = new Set<number>();
    // Nothing on screen: the first of the queue.
    const first = reconcileSignInScreen(NO_SCREEN, [q("a", 1), q("b", 2)], none);
    expect(first).toEqual({ kind: "asking", host: "a", hostName: "A", id: 1, answerable: true });
    // A newer question never takes the screen; the same object is kept while nothing changed.
    expect(reconcileSignInScreen(first, [q("a", 1), q("b", 2), q("c", 3)], none)).toBe(first);
    // The user answered a: the next one, silently.
    expect(reconcileSignInScreen(first, [q("b", 2)], new Set([1]))).toEqual({ kind: "asking", host: "b", hostName: "B", id: 2, answerable: true });
    expect(reconcileSignInScreen(first, [], new Set([1]))).toBe(NO_SCREEN);
    // a went away without the user: the screen says so and does not swap b under the cursor.
    const gone = reconcileSignInScreen(first, [q("b", 2)], none);
    expect(gone).toEqual({ kind: "withdrawn", host: "a", hostName: "A", id: 1 });
    expect(reconcileSignInScreen(gone, [q("b", 2)], none)).toBe(gone);
    // The same host asking again under a new id is a different question.
    expect(reconcileSignInScreen(first, [q("a", 5)], none)).toMatchObject({ kind: "withdrawn", id: 1 });
    const s: SignInScreen = reconcileSignInScreen(first, [q("a", 5)], new Set([1]));
    expect(s).toMatchObject({ kind: "asking", id: 5 });
    // A notice (approve on your phone) ends by itself when the user does that: nothing was withdrawn.
    const notice = reconcileSignInScreen(NO_SCREEN, [{ ...q("a", 7), prompt: { ...q("a", 7).prompt, kind: "info" } }], none);
    expect(notice).toMatchObject({ kind: "asking", id: 7, answerable: false });
    expect(reconcileSignInScreen(notice, [q("a", 8)], none)).toMatchObject({ kind: "asking", id: 8 });
    expect(reconcileSignInScreen(notice, [], none)).toBe(NO_SCREEN);
  });
});
