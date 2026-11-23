import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { launchRequest, resumeRows, validate, type NewSessionForm } from "../../src/features/newsession/model";
import { inventoryFor, FIXTURE_HOSTS } from "../../src/bridge/fixture/catalog";
import { renderApp } from "../helpers";

const inv = () => inventoryFor(FIXTURE_HOSTS[0], "~/src/ledgerline", Date.now()).find((d) => d.driver === "claude")!;
const form = (p: Partial<NewSessionForm> = {}): NewSessionForm => ({ host: "x1", name: "", driver: "claude", cwd: "~/src/ledgerline", model: "", mode: "", start: "new", resumeId: "", latest: false, prompt: "", ...p });

describe("new session: logic", () => {
  it("lists the CLI's sessions newest first and tags where they were started", () => {
    const rows = resumeRows(inv(), [{ driver: "claude", native_session_id: "native-7" } as never]);
    expect(rows[0].title).toBe("Refactor invoice totals rounding");
    expect(rows[0].startedIn).toBe("marsad");
    expect(rows[1].startedIn).toBe("terminal");
    expect(rows.map((r) => r.modified_ms)).toEqual([...rows.map((r) => r.modified_ms)].sort((a, b) => b - a));
    expect(resumeRows(inv(), [], "stripe").map((r) => r.id)).toEqual(["c-term-1"]);
  });
  it("builds the launch request for each start mode", () => {
    expect(launchRequest(form({ prompt: " hi " }))).toMatchObject({ host: "x1", driver: "claude", resume: { kind: "new" }, first_prompt: "hi", model: null });
    expect(launchRequest(form({ start: "continue", resumeId: "c-term-1" })).resume).toEqual({ kind: "resume", native_session_id: "c-term-1" });
    expect(launchRequest(form({ start: "continue", latest: true })).resume).toEqual({ kind: "latest" });
  });
  it("explains what is missing", () => {
    expect(validate(form(), inv(), false)).toMatch(/not connected/);
    expect(validate(form({ cwd: "" }), inv(), true)).toMatch(/project folder/);
    expect(validate(form({ start: "continue" }), inv(), true)).toMatch(/Pick a session/);
    expect(validate(form(), inv(), true)).toBe("");
  });
});

describe("new session: dialog", () => {
  it("starts a new session from inventory values and opens it", async () => {
    const user = userEvent.setup();
    const { backend, history } = await renderApp();
    await user.keyboard("n");
    const dlg = await screen.findByRole("dialog", { name: "New session" });
    await waitFor(() => expect(within(dlg).getByRole("radio", { name: /Claude Code/ })).toHaveAttribute("aria-checked", "true"));
    expect(within(dlg).getByRole("radio", { name: /Kilo Code/ })).toBeDisabled();
    expect(within(dlg).getByRole("radio", { name: /win-build/ })).toBeDisabled();
    await waitFor(() => expect(within(dlg).getByLabelText("Project folder")).toHaveValue("~/src/ledgerline"));
    await user.type(within(dlg).getByLabelText(/First message/), "Add a health check");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    await waitFor(() => expect(backend.requests.some((r) => r.method === "sessions.launch")).toBe(true));
    const req = backend.requests.find((r) => r.method === "sessions.launch")!;
    expect(req.params).toMatchObject({ host: "x1", driver: "claude", model: "claude-sonnet-4.5", mode: "default", first_prompt: "Add a health check", resume: { kind: "new" } });
    await waitFor(() => expect(history.store.get()).toMatch(/^\/session\/x1\//));
  });

  it("Esc on the open model menu closes only the menu: the dialog and the typed message stay; the next Esc closes the dialog", async () => {
    const user = userEvent.setup();
    await renderApp();
    await user.keyboard("n");
    const dlg = await screen.findByRole("dialog", { name: "New session" });
    await waitFor(() => expect(within(dlg).getByLabelText("Project folder")).toHaveValue("~/src/ledgerline"));
    await user.type(within(dlg).getByLabelText(/First message/), "Add a health check");
    const model = await within(dlg).findByRole("button", { name: /^Model: / });
    await waitFor(() => expect(model).not.toHaveAccessibleName("Model: Reported after start"));
    await user.click(model);
    const list = await screen.findByRole("listbox", { name: "Model" });
    expect(within(list).getAllByRole("option").length).toBeGreaterThan(1);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox", { name: "Model" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "New session" })).toBeInTheDocument();
    expect(within(dlg).getByLabelText(/First message/)).toHaveValue("Add a health check");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New session" })).toBeNull());
  });

  it("continues an existing CLI session picked from the list", async () => {
    const user = userEvent.setup();
    const { backend } = await renderApp();
    await user.keyboard("n");
    const dlg = await screen.findByRole("dialog", { name: "New session" });
    await waitFor(() => expect(within(dlg).getByLabelText("Project folder")).toHaveValue("~/src/ledgerline"));
    await user.click(within(dlg).getByRole("button", { name: "Continue an existing one" }));
    const list = await within(dlg).findByRole("listbox", { name: "Sessions to continue" });
    await waitFor(() => expect(within(list).getAllByRole("option").length).toBeGreaterThan(3));
    await user.click(within(list).getByRole("option", { name: /Fix Stripe webhook/ }));
    await user.click(within(dlg).getByRole("button", { name: /Continue session/ }));
    await waitFor(() => expect(backend.requests.find((r) => r.method === "sessions.launch")?.params).toMatchObject({ resume: { kind: "resume", native_session_id: "c-term-1" } }));
  });
});

describe("new session: the host's agent command", () => {
  it("shows the command in the agent picker, the discovery line and the session's Config tab", async () => {
    const user = userEvent.setup();
    const { backend, history } = await renderApp();
    backend.envs.set({ host: "x1", setup: "module load tools", vars: [], commands: { claude: "genie" }, revision: "" }, true);
    await user.keyboard("n");
    const dlg = await screen.findByRole("dialog", { name: "New session" });
    const claude = await within(dlg).findByRole("radio", { name: /Claude Code/ });
    await waitFor(() => expect(claude).toHaveTextContent("genie"));
    expect(within(dlg).getByText(/discovered from genie \(this host's command for Claude Code\)/)).toBeInTheDocument();
    await waitFor(() => expect(within(dlg).getByLabelText("Project folder")).toHaveValue("~/src/ledgerline"));
    await user.type(within(dlg).getByLabelText(/First message/), "hello");
    await user.click(within(dlg).getByRole("button", { name: /Start session/ }));
    await waitFor(() => expect(history.store.get()).toMatch(/^\/session\/x1\//));
    history.push(`${history.store.get().split("?")[0]}?tab=config`);
    expect(await screen.findByText("Command")).toBeInTheDocument();
    expect(screen.getByText(/runs as/)).toHaveTextContent("Claude Code runs as genie on");
  });

  it("warns in the picker when the command does not answer like the agent", async () => {
    const user = userEvent.setup();
    const { backend } = await renderApp();
    backend.envs.set({ host: "x1", setup: "", vars: [], commands: { claude: "lister" }, revision: "" }, true);
    await user.keyboard("n");
    const dlg = await screen.findByRole("dialog", { name: "New session" });
    const claude = await within(dlg).findByRole("radio", { name: /Claude Code/ });
    await waitFor(() => expect(claude.querySelector(".cmd.c-needs")).not.toBeNull());
    // The selected agent's warning is spelled out once the folder's inventory is in.
    await waitFor(() => expect(within(dlg).getByText(/does not answer like Claude Code/, { selector: ".disc" })).toBeInTheDocument(), { timeout: 3000 });
  });
});
