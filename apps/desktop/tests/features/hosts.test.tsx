import { act, cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { hostKindOf } from "../../src/features/hosts/AddHostDialog";
import { hold, quietBackend, renderApp } from "../helpers";
import { sameMachineAs } from "../../src/state/hosts";

async function hostsPage(path = "/hosts") {
  const backend = quietBackend();
  const r = await renderApp("/inbox", backend);
  act(() => r.history.push(path));
  await screen.findByRole("listbox", { name: "Hosts" });
  return r;
}

describe("hosts", () => {
  it("lists every host with its link in words and installed CLIs", async () => {
    await hostsPage();
    const list = screen.getByRole("listbox", { name: "Hosts" });
    const row = (id: string) => list.querySelector(`[data-host="${id}"]`) as HTMLElement;
    expect(within(row("win")).getByText(/Reconnecting/)).toBeInTheDocument();
    expect(within(row("gpu")).getByText("Needs you to sign in")).toBeInTheDocument();
    expect(within(row("pi")).getByText(/Updating helper/)).toBeInTheDocument();
    await waitFor(() => expect(within(row("x1")).getByText("2.1.4")).toBeInTheDocument());
  });

  it("a held host waits for the explicit Sign in click, then walks secret, one-time code and push prompts", async () => {
    const user = userEvent.setup();
    const { backend } = await hostsPage("/hosts?h=gpu");
    // Nothing was asked of the host without the click.
    expect(backend.requests.some((r) => r.method === "hosts.reconnect")).toBe(false);
    await user.click(within(screen.getByRole("complementary", { name: "gpu-box details" })).getByRole("button", { name: "Sign in" }));
    const dlg = await screen.findByRole("dialog", { name: "Sign in to gpu-box" });
    expect(within(dlg).getByTestId("auth-prompt-text")).toHaveTextContent("ml@10.0.4.21's password:");
    const pw = within(dlg).getByLabelText("Password or passphrase");
    expect(pw).toHaveAttribute("type", "password");
    await user.type(pw, "wrong");
    await user.click(within(dlg).getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText(/That did not work. Try again \(attempt 2\)/)).toBeInTheDocument();
    expect((screen.getByLabelText("Password or passphrase") as HTMLInputElement).value).toBe("");
    await user.type(screen.getByLabelText("Password or passphrase"), "hunter2");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    const code = await screen.findByLabelText("One-time code");
    expect(code).toHaveAttribute("autocomplete", "one-time-code");
    await user.type(code, "123456");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText(/Waiting for approval on your device/)).toBeInTheDocument();
    expect(screen.getByTestId("auth-prompt-text")).toHaveTextContent(/Duo Push/);
    const answers = backend.requests.filter((r) => r.method === "hosts.auth_answer").map((r) => (r.params as { answer: string }).answer);
    expect(answers).toEqual(["wrong", "hunter2", "123456"]);
  });

  it("yes/no prompts answer yes or no; cancel gives up", async () => {
    const user = userEvent.setup();
    const { backend } = await hostsPage("/hosts?h=gpu");
    act(() => backend.setAuthPrompt("gpu", { prompt: "The authenticity of host 'gpu-box' can't be established.\nAre you sure you want to continue connecting (yes/no)?", kind: "confirm", attempt: 1 }));
    const dlg = await screen.findByRole("dialog", { name: "Sign in to gpu-box" });
    expect(within(dlg).getByTestId("auth-prompt-text").textContent).toContain("can't be established.\nAre you sure");
    await user.click(within(dlg).getByRole("button", { name: "Cancel sign-in" }));
    await waitFor(() => expect(backend.requests.some((r) => r.method === "hosts.auth_cancel")).toBe(true));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Sign in to gpu-box" })).toBeNull());
  });

  it("add host: builds the host kind and shows live progress", async () => {
    expect(hostKindOf({ kind: "ssh", name: "x", target: "ops@ci", port: "2222", identity: "", options: "ProxyJump=b", distro: "" })).toEqual({ kind: "ssh", target: "ops@ci", port: 2222, identity_file: null, options: ["ProxyJump=b"] });
    const user = userEvent.setup();
    const { backend } = await hostsPage("/hosts?add=1");
    const dlg = await screen.findByRole("dialog", { name: "Add host" });
    await user.click(await within(dlg).findByRole("button", { name: /ci-runner-02/ }));
    expect(within(dlg).getByLabelText("SSH target")).toHaveValue("ci-runner-02");
    await user.click(within(dlg).getByRole("button", { name: "Add and connect" }));
    await waitFor(() => expect(backend.requests.find((r) => r.method === "hosts.add")?.params).toMatchObject({ name: "ci-runner-02", kind: { kind: "ssh", target: "ci-runner-02", port: 22 } }));
    expect(await within(dlg).findByText(/Found Claude Code 2.1.4/, {}, { timeout: 3000 })).toBeInTheDocument();
  });

  it("add host: WSL offers the detected distributions with the default preselected, typing as fallback", async () => {
    const user = userEvent.setup();
    const { backend } = await hostsPage("/hosts?add=1");
    const dlg = await screen.findByRole("dialog", { name: "Add host" });
    await user.click(await within(dlg).findByRole("button", { name: "WSL" }));
    const group = await within(dlg).findByRole("radiogroup", { name: "WSL distribution" });
    expect(within(group).getByRole("radio", { name: /Ubuntu-24.04/ })).toHaveAttribute("aria-checked", "true");
    expect(within(group).getByRole("radio", { name: /Ubuntu-24.04/ })).toHaveTextContent(/default/);
    await user.click(within(group).getByRole("radio", { name: /Debian/ }));
    await user.click(within(group).getByRole("radio", { name: "Other…" }));
    await user.type(within(dlg).getByLabelText("Distribution name"), "Alpine");
    await user.click(within(group).getByRole("radio", { name: /Debian/ }));
    expect(within(dlg).queryByLabelText("Distribution name")).toBeNull();
    await user.click(within(dlg).getByRole("button", { name: "Add and connect" }));
    await waitFor(() => expect(backend.requests.find((r) => r.method === "hosts.add")?.params).toMatchObject({ name: "Ubuntu-24.04", kind: { kind: "wsl", distro: "Debian" } }));
  });

  it("a configuration failure says what is wrong, does not count attempts, and an unknown distro can be fixed in place", async () => {
    const user = userEvent.setup();
    const { backend, history } = await hostsPage("/hosts?add=1");
    const dlg = await screen.findByRole("dialog", { name: "Add host" });
    await user.click(await within(dlg).findByRole("button", { name: "WSL" }));
    await within(dlg).findByRole("radiogroup", { name: "WSL distribution" });
    await user.click(within(dlg).getByRole("button", { name: "Add and connect" }));
    await waitFor(() => expect(backend.requests.some((r) => r.method === "hosts.add")).toBe(true));
    await user.click(within(dlg).getByRole("button", { name: /Close and keep going|Done/ }));
    act(() => backend.setLink("ubuntu-24-04", { state: "failed", problem: "unknown_distro", reason: 'WSL has no distribution named "Ubuntu". Installed: Ubuntu-24.04, Debian. Pick an installed distribution for this host.' }));
    act(() => history.push("/hosts?h=ubuntu-24-04"));
    const panel = await screen.findByRole("complementary", { name: "Ubuntu-24.04 details" });
    expect((await within(panel).findAllByText("WSL distribution not found")).length).toBeGreaterThan(0);
    expect(within(panel).getAllByText(/Installed: Ubuntu-24.04, Debian/).length).toBeGreaterThan(0);
    expect(within(panel).queryByText(/The connection dropped/)).toBeNull();
    expect(within(panel).queryByText(/Attempt/)).toBeNull();
    await user.click(await within(panel).findByRole("radio", { name: /Debian/ }));
    await user.click(within(panel).getByRole("button", { name: "Use this distribution" }));
    await waitFor(() => expect(backend.requests.find((x) => x.method === "hosts.update")?.params).toMatchObject({ kind: { kind: "wsl", distro: "Debian" } }));
    await waitFor(() => expect(backend.requests.some((x) => x.method === "hosts.reconnect")).toBe(true));
  });
  describe("sign out of a shared ssh connection", () => {
    const requests = (b: { requests: { method: string; params?: unknown }[] }, m: string) => b.requests.filter((r) => r.method === m);

    it("a connected host that has one offers Sign out: the dialog says what happens, the hub is asked once, the host then waits for the user and is not reconnected by itself", async () => {
      const user = userEvent.setup();
      const { backend } = await hostsPage("/hosts?h=hz");
      const panel = await screen.findByRole("complementary", { name: "hetzner-ax41 details" });
      expect(await within(panel).findByText("Kept open and shared, until you sign out or it ends")).toBeInTheDocument();
      await user.click(within(panel).getByRole("button", { name: "Sign out" }));
      const dlg = await screen.findByRole("dialog", { name: "Sign out of hetzner-ax41?" });
      expect(dlg).toHaveTextContent("Marsad closes the connection it keeps open to hetzner-ax41. Sessions already running there keep running, and Marsad will not reconnect until you sign in again.");
      // Nothing is asked until the user confirms.
      expect(requests(backend, "hosts.signout")).toEqual([]);
      await user.click(within(dlg).getByRole("button", { name: "Sign out" }));
      await waitFor(() => expect(requests(backend, "hosts.signout")).toHaveLength(1));
      expect(requests(backend, "hosts.signout")[0].params).toEqual({ id: "hz" });
      // The hub parks it (a key host too): the card says it was the user's act, and offers the one click.
      const held = await within(panel).findByTestId("host-held");
      expect(held).toHaveTextContent("Needs you to sign in");
      expect(within(panel).getByTestId("held-detail")).toHaveTextContent("You signed out of this host: connect it to sign in again.");
      expect(held).toHaveTextContent("Marsad will not reconnect it on its own");
      expect(within(panel).queryByRole("button", { name: "Sign out" })).toBeNull();
      expect(requests(backend, "hosts.reconnect")).toEqual([]);
      // One click signs it in again.
      await user.click(within(held).getByRole("button", { name: "Sign in" }));
      await waitFor(() => expect(requests(backend, "hosts.reconnect")).toHaveLength(1));
      expect(await within(panel).findByRole("button", { name: "Sign out" }, { timeout: 4000 })).toBeInTheDocument();
    });

    it("a host the hub says has no shared connection (a Windows hub) and the local host offer no Sign out", async () => {
      const { backend } = await hostsPage("/hosts?h=hz");
      let panel = await screen.findByRole("complementary", { name: "hetzner-ax41 details" });
      await within(panel).findByRole("button", { name: "Sign out" });
      // The decision is the hub's: the same host, its entry saying there is no shared connection.
      const entry = (await backend.request<{ hosts: { id: string }[] }>("hosts.list")).hosts.find((h) => h.id === "hz")!;
      await act(async () => {
        await backend.request("hosts.update", { ...entry, shared_connection: false });
      });
      await waitFor(() => expect(within(panel).queryByRole("button", { name: "Sign out" })).toBeNull());
      expect(within(panel).queryByText(/Kept open and shared/)).toBeNull();
      expect(within(panel).getByRole("button", { name: "Remove host" })).toBeInTheDocument();
      // The local host never has one.
      cleanup();
      await hostsPage("/hosts?h=x1");
      panel = await screen.findByRole("complementary", { name: "thinkpad-x1 details" });
      expect(within(panel).queryByRole("button", { name: "Sign out" })).toBeNull();
    });

    it("a host that is not connected has no connection to sign out of", async () => {
      await hostsPage("/hosts?h=win");
      const panel = await screen.findByRole("complementary", { name: "win-build details" });
      expect(within(panel).queryByRole("button", { name: "Sign out" })).toBeNull();
    });

    it("the hub's refusal is shown", async () => {
      const user = userEvent.setup();
      const { backend } = await hostsPage("/hosts?h=hz");
      const panel = await screen.findByRole("complementary", { name: "hetzner-ax41 details" });
      const h = hold(backend, "hosts.signout");
      await user.click(await within(panel).findByRole("button", { name: "Sign out" }));
      await user.click(within(await screen.findByRole("dialog", { name: "Sign out of hetzner-ax41?" })).getByRole("button", { name: "Sign out" }));
      await waitFor(() => expect(h.pending()).toBe(1));
      act(() => h.reject({ code: "invalid_params", message: "this host has no shared ssh connection to sign out of" }));
      expect(await screen.findByText("Could not sign out of hetzner-ax41")).toBeInTheDocument();
      expect(screen.getByText("this host has no shared ssh connection to sign out of")).toBeInTheDocument();
    });
  });

  it("names hosts that reach the same machine (same machine key)", () => {
    const e = (id: string, machine?: string) => ({ entry: { id, name: id.toUpperCase(), machine } as never });
    const hosts = { a: e("a", "m1"), a2: e("a2", "m1"), b: e("b", "m2"), c: e("c") };
    expect(sameMachineAs(hosts, "a")).toEqual(["A2"]);
    expect(sameMachineAs(hosts, "b")).toEqual([]);
    expect(sameMachineAs(hosts, "c")).toEqual([]);
  });
  it("shows where the host's environment came from, its PATH, and refreshes it", async () => {
    const user = userEvent.setup();
    const { backend } = await hostsPage("/hosts?h=x1");
    const panel = await screen.findByRole("complementary", { name: "thinkpad-x1 details" });
    expect(await within(panel).findByText(/Your login shell \(login \+ interactive/)).toBeInTheDocument();
    expect(within(panel).getByText("/bin/bash")).toBeInTheDocument();
    expect(within(panel).getByText(/PATH \(5 entries\)/)).toBeInTheDocument();
    expect(within(panel).getByRole("list", { name: "PATH", hidden: true })).toHaveTextContent(".nvm/versions/node");
    await user.click(within(panel).getByRole("button", { name: "Refresh environment" }));
    await waitFor(() => expect(backend.requests.some((r) => r.method === "host.environment.refresh" && r.host === "x1")).toBe(true));
  });

  it("edits setup commands, variables (a secret stays write-only) and agent commands, saves and tests", async () => {
    const user = userEvent.setup();
    const { backend } = await hostsPage("/hosts?h=x1");
    const panel = await screen.findByRole("complementary", { name: "thinkpad-x1 details" });
    const setup = await within(panel).findByLabelText(/Setup commands/);
    const save = within(panel).getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    await user.type(setup, "module load tools");
    await user.click(within(panel).getByRole("button", { name: "Add variable" }));
    await user.type(within(panel).getByLabelText("Variable name"), "API_TOKEN");
    await user.click(within(panel).getByRole("checkbox", { name: "Secret" }));
    const value = within(panel).getByLabelText("Value of API_TOKEN");
    expect(value).toHaveAttribute("type", "password");
    expect(save).toBeDisabled(); // a new secret needs a value
    await user.type(value, "s3cret-ui");
    await user.type(within(panel).getByLabelText("Command for Claude Code"), "genie");
    // Test runs saved settings only.
    expect(within(panel).getByRole("button", { name: "Test" })).toBeDisabled();
    await user.click(save);
    await waitFor(() => expect(backend.requests.some((r) => r.method === "hosts.environment.set")).toBe(true));
    const sent = backend.requests.find((r) => r.method === "hosts.environment.set")!.params as { setup: string; vars: { name: string; value: string; secret: boolean }[]; commands: Record<string, string> };
    expect(sent.setup).toBe("module load tools");
    expect(sent.vars).toEqual([{ name: "API_TOKEN", value: "s3cret-ui", secret: true }]);
    expect(sent.commands).toEqual({ claude: "genie" });
    // After saving the secret is never shown again; the setup outcome and the command are.
    await waitFor(() => expect(within(panel).getByLabelText("Value of API_TOKEN")).toHaveValue("••••••••"));
    expect(panel.innerHTML).not.toContain("s3cret-ui");
    expect(await within(panel).findByText(/Setup commands ran in/)).toBeInTheDocument();
    await waitFor(() => expect(within(panel).getAllByText(/genie →/).length).toBeGreaterThan(0));
    // Saving again without touching the secret keeps it (no value sent).
    await user.type(within(panel).getByLabelText(/Setup commands/), "{enter}export X=1");
    await user.click(within(panel).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(backend.requests.filter((r) => r.method === "hosts.environment.set").length).toBe(2));
    const second = backend.requests.filter((r) => r.method === "hosts.environment.set")[1].params as { vars: { value: string | null }[] };
    expect(second.vars[0].value).toBeNull();
    // Test: per agent the command, where it resolved and its version.
    await user.click(within(panel).getByRole("button", { name: "Test" }));
    const result = await within(panel).findByLabelText("Test result");
    expect(within(result).getByText(/genie → \/opt\/site\/modules\/tools\/bin\/genie/)).toBeInTheDocument();
  });

  it("says where secret values are kept: the system store, or the owner-only file and why", async () => {
    const { backend } = await hostsPage("/hosts?h=x1");
    const panel = await screen.findByRole("complementary", { name: "thinkpad-x1 details" });
    expect(await within(panel).findByLabelText("Where secrets are kept")).toHaveTextContent("Secret values are kept in Secret Service on the computer running Marsad.");
    cleanup();
    backend.envs.secretsLocation = { kind: "file", path: "/home/me/.marsad/machines/k/store/secrets.json", reason: "the Secret Service cannot be used: none is running on the session bus (GNOME Keyring, KWallet)" };
    act(() => { localStorage.clear(); });
    const again = await renderApp("/inbox", backend);
    act(() => again.history.push("/hosts?h=x1"));
    const p2 = await screen.findByRole("complementary", { name: "thinkpad-x1 details" });
    const where = await within(p2).findByLabelText("Where secrets are kept");
    expect(where).toHaveTextContent("readable only by you, because the Secret Service cannot be used: none is running");
    expect(where).toHaveTextContent("store/secrets.json");
    expect(where.className).toContain("c-needs");
  });

  it("says plainly when the setup fails and when an agent command is not the agent", async () => {
    const user = userEvent.setup();
    await hostsPage("/hosts?h=x1");
    const panel = await screen.findByRole("complementary", { name: "thinkpad-x1 details" });
    await user.type(await within(panel).findByLabelText(/Setup commands/), "module load nope");
    await user.type(within(panel).getByLabelText("Command for Claude Code"), "lister");
    await user.click(within(panel).getByRole("button", { name: "Save" }));
    const alert = await within(panel).findByRole("alert");
    expect(alert).toHaveTextContent("Setup commands failed");
    expect(alert).toHaveTextContent("exit status 1");
    expect(alert).toHaveTextContent("module: nope not found");
    await user.click(within(panel).getByRole("button", { name: "Test" }));
    const result = await within(panel).findByLabelText("Test result");
    expect(within(result).getByText(/`lister` does not answer like Claude Code/)).toBeInTheDocument();
  });

  it("ask before connecting: toggling sends hosts.update, reflects from entry, and shows Waiting for you to connect when disconnected", async () => {
    const user = userEvent.setup();
    const { backend } = await hostsPage("/hosts?h=hz");
    const panel = await screen.findByRole("complementary", { name: "hetzner-ax41 details" });
    const ask = within(panel).getByRole("checkbox", { name: /Ask me before connecting/ });
    expect(ask).not.toBeChecked();
    expect(panel).toHaveTextContent("Marsad won't connect this host on its own — use this for hosts whose sign-in sends a phone approval.");
    await user.click(ask);
    await waitFor(() => expect(backend.requests.find((r) => r.method === "hosts.update")?.params).toMatchObject({ id: "hz", ask_before_connecting: true }));
    await waitFor(() => expect(ask).toBeChecked());
    // Still connected: no waiting card.
    expect(within(panel).queryByText("Waiting for you to connect")).toBeNull();
    // When the host isn't connected, the host card shows "Waiting for you to connect" with the Connect button.
    act(() => backend.dropLink("hz", "connection reset by peer"));
    const held = await within(panel).findByTestId("host-held");
    expect(held).toHaveTextContent("Waiting for you to connect");
    const connectBtn = within(held).getByRole("button", { name: "Connect" });
    await user.click(connectBtn);
    await waitFor(() => expect(backend.requests.some((r) => r.method === "hosts.reconnect" && (r.params as { id: string }).id === "hz")).toBe(true));
    await waitFor(() => expect(within(panel).queryByTestId("host-held")).toBeNull());
    // Toggling off sends hosts.update with false and reflects back on the checkbox.
    await user.click(ask);
    await waitFor(() => {
      const updates = backend.requests.filter((r) => r.method === "hosts.update");
      expect(updates[updates.length - 1]?.params).toMatchObject({ id: "hz", ask_before_connecting: false });
    });
    await waitFor(() => expect(ask).not.toBeChecked());
    // Local host does not show the setting.
    cleanup();
    await hostsPage("/hosts?h=x1");
    const localPanel = await screen.findByRole("complementary", { name: "thinkpad-x1 details" });
    expect(within(localPanel).queryByRole("checkbox", { name: /Ask me before connecting/ })).toBeNull();
  });

  it("ask before connecting on a host with interactive auth shows Waiting for you to connect and Sign in button", async () => {
    const user = userEvent.setup();
    const { backend } = await hostsPage("/hosts?h=gpu");
    const panel = await screen.findByRole("complementary", { name: "gpu-box details" });
    expect(within(await within(panel).findByTestId("host-held")).getByText("Needs you to sign in")).toBeInTheDocument();
    await user.click(within(panel).getByRole("checkbox", { name: /Ask me before connecting/ }));
    await waitFor(() => expect(backend.requests.find((r) => r.method === "hosts.update")?.params).toMatchObject({ id: "gpu", ask_before_connecting: true }));
    const held = await within(panel).findByTestId("host-held");
    expect(held).toHaveTextContent("Waiting for you to connect");
    expect(within(held).getByRole("button", { name: "Sign in" })).toBeInTheDocument();
  });
});
