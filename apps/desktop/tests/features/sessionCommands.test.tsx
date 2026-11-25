import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import type { SessionSnapshot } from "../../src/bridge/wire";
import { sessionPath } from "../../src/router";
import { hold, quietBackend, renderApp, sessionByName } from "../helpers";

const COMPACT = { name: "compact", description: "Summarize history to free context", argument_hint: "" };
const NAME = "migrate-auth-middleware";

/**
 * A live, idle session with a completed turn and nothing waiting for the user: the state Compact and Fork are about.
 * `over` replaces any of the snapshot's fields (capabilities, slash_commands, phase, turns_completed ...).
 */
function idleSession(backend: FixtureBackend, over: Partial<SessionSnapshot> = {}) {
  const s = sessionByName(backend, NAME);
  backend.patchSession(s.host, s.id, { phase: { state: "idle" }, turn: null, turns_completed: 2, pending_gates: [], ...over });
  return s;
}
const withCaps = (backend: FixtureBackend, host: string, id: string, caps: Partial<SessionSnapshot["capabilities"]>) => ({
  capabilities: { ...backend.snapshot(host, id).capabilities, ...caps },
});

async function open(backend: FixtureBackend, s: { host: string; id: string }) {
  const r = await renderApp(sessionPath(s.host, s.id), backend);
  await screen.findByRole("heading", { name: NAME });
  return r;
}

const commandsSent = (backend: FixtureBackend, command: string) =>
  backend.requests.filter((r) => r.method === "session.command" && (r.params as { command: { command: string } }).command.command === command);

describe("Compact in the session header", () => {
  it("is not offered when the CLI does not support compacting, even if it lists the command", async () => {
    const backend = quietBackend();
    const s = idleSession(backend, { slash_commands: [COMPACT], ...withCaps(backend, "x1", sessionByName(backend, NAME).id, { compact: false }) });
    await open(backend, s);
    expect(screen.queryByRole("button", { name: "Compact" })).not.toBeInTheDocument();
  });

  it("is not offered when the CLI supports it but does not list it in its slash commands", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, NAME);
    idleSession(backend, { slash_commands: [], ...withCaps(backend, s.host, s.id, { compact: true }) });
    await open(backend, s);
    expect(screen.queryByRole("button", { name: "Compact" })).not.toBeInTheDocument();
  });

  it("is offered and enabled when the CLI supports it and lists it, and sends the compact command to the session", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, NAME);
    idleSession(backend, { slash_commands: [COMPACT], ...withCaps(backend, s.host, s.id, { compact: true }) });
    const user = userEvent.setup();
    await open(backend, s);
    const compact = screen.getByRole("button", { name: "Compact" });
    expect(compact).toBeEnabled();
    await user.click(compact);
    await waitFor(() => expect(commandsSent(backend, "compact")).toHaveLength(1));
    expect(commandsSent(backend, "compact")[0]).toMatchObject({ host: s.host, params: { id: s.id, command: { command: "compact" } } });
    // The CLI reports the compaction back, and the transcript says what it reported (and only that).
    expect(await screen.findByText(/History compacted at \d\d:\d\d \(manual\): 120k → 40k tokens in 2\.1s · 80k dropped so far/)).toBeInTheDocument();
  });

  it("is disabled while the agent is busy, so a compaction never queues behind a turn", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, NAME);
    idleSession(backend, { slash_commands: [COMPACT], ...withCaps(backend, s.host, s.id, { compact: true }), phase: { state: "working" } });
    await open(backend, s);
    expect(screen.getByRole("button", { name: "Compact" })).toBeDisabled();
    expect(commandsSent(backend, "compact")).toHaveLength(0);
  });
});

describe("Fork in the session header", () => {
  it("is not offered when the CLI cannot fork", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, NAME);
    idleSession(backend, withCaps(backend, s.host, s.id, { fork: false }));
    await open(backend, s);
    expect(screen.queryByRole("button", { name: "Fork" })).not.toBeInTheDocument();
  });

  it("is not offered while the agent is busy", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, NAME);
    idleSession(backend, { ...withCaps(backend, s.host, s.id, { fork: true }), phase: { state: "working" } });
    await open(backend, s);
    expect(screen.queryByRole("button", { name: "Fork" })).not.toBeInTheDocument();
  });

  it("is not offered before the session has completed a turn", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, NAME);
    idleSession(backend, { ...withCaps(backend, s.host, s.id, { fork: true }), turns_completed: 0 });
    await open(backend, s);
    expect(screen.queryByRole("button", { name: "Fork" })).not.toBeInTheDocument();
  });

  it("forks the session on its host and moves to the new session, leaving the parent as it was", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, NAME);
    idleSession(backend, withCaps(backend, s.host, s.id, { fork: true }));
    const user = userEvent.setup();
    const { history } = await open(backend, s);
    await user.click(screen.getByRole("button", { name: "Fork" }));
    const forks = backend.requests.filter((r) => r.method === "sessions.fork");
    expect(forks).toHaveLength(1);
    expect(forks[0]).toMatchObject({ host: s.host, params: { parent: s.id, name: null } });
    await waitFor(() => expect(history.store.get()).toMatch(new RegExp(`^/session/${s.host}/fxfk`)));
    const child = history.store.get().split("/").pop()!;
    expect(backend.snapshot(s.host, child).forked_from).toEqual({ session: s.id, native_session_id: backend.snapshot(s.host, s.id).native_session_id ?? "" });
    expect(backend.snapshot(s.host, s.id).phase).toEqual({ state: "idle" });
  });

  it("a refused fork says so and stays on the session", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, NAME);
    idleSession(backend, withCaps(backend, s.host, s.id, { fork: true }));
    const refusal = hold(backend, "sessions.fork");
    refusal.failAll({ code: "invalid_state", message: "the session is working" });
    const user = userEvent.setup();
    const { history } = await open(backend, s);
    await user.click(screen.getByRole("button", { name: "Fork" }));
    expect(await screen.findByText("Could not fork the session")).toBeInTheDocument();
    expect(history.store.get()).toBe(sessionPath(s.host, s.id));
  });
});

describe("the banner of a forked session", () => {
  async function forkOf(backend: FixtureBackend) {
    const s = sessionByName(backend, NAME);
    idleSession(backend, withCaps(backend, s.host, s.id, { fork: true }));
    const user = userEvent.setup();
    const r = await open(backend, s);
    await user.click(screen.getByRole("button", { name: "Fork" }));
    await screen.findByTestId("forked-from");
    return { ...r, s, user };
  }

  it("is shown on the fork, naming the parent", async () => {
    const backend = quietBackend();
    const { s, history } = await forkOf(backend);
    expect(screen.getByTestId("forked-from")).toHaveTextContent(`Forked from ${NAME} · open`);
    expect(history.store.get()).not.toBe(sessionPath(s.host, s.id));
  });

  it("is not shown on a session that was not forked", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, NAME);
    await open(backend, s);
    expect(screen.queryByTestId("forked-from")).not.toBeInTheDocument();
  });

  it("links to the parent: following it opens the parent session", async () => {
    const backend = quietBackend();
    const { s, history, user } = await forkOf(backend);
    await user.click(within(screen.getByTestId("forked-from")).getByRole("link", { name: "open" }));
    expect(history.store.get()).toBe(sessionPath(s.host, s.id));
    await screen.findByRole("heading", { name: NAME });
  });

  it("names the parent by its id when the parent is not in the fleet any more", async () => {
    const backend = quietBackend();
    const { s } = await forkOf(backend);
    await backend.request("sessions.delete", { id: s.id }, { host: s.host });
    await waitFor(() => expect(screen.getByTestId("forked-from")).toHaveTextContent(`Forked from ${s.id} · open`));
  });
});
