import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { quietBackend, renderApp, sessionByName } from "../helpers";

const SHELL_WITH_FILES = "Shell commands can't take attachments. Remove the files, or remove the !.";

/** Opens a session's view and waits for its composer. */
async function openSession(name: string, backend = quietBackend()) {
  const s = sessionByName(backend, name);
  await renderApp(`/session/${s.host}/${s.id}`, backend);
  await screen.findByRole("heading", { name });
  return { backend, s, box: (await screen.findByRole("textbox", { name: "Message" })) as HTMLTextAreaElement };
}

/** Attaches one PNG the way a paste does. */
function pastePng(box: HTMLElement) {
  const png = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "shot.png", { type: "image/png" });
  fireEvent.paste(box, { clipboardData: { files: [png], types: ["Files"] } });
}

/** The requests of one method, in order. */
const calls = (backend: FixtureBackend, method: string) => backend.requests.filter((r) => r.method === method);

describe("composer: ! shell mode", () => {
  it("! switches the composer to shell mode; the hint says the agent's shell runs it when the agent can run one", async () => {
    const user = userEvent.setup();
    const { box } = await openSession("ws-reconnect-backoff");
    expect(screen.queryByTestId("shell-mode")).toBeNull();
    await user.type(box, "!");
    expect(screen.getByTestId("shell-mode")).toHaveTextContent("Shell");
    expect(screen.getByTestId("shell-hint")).toHaveTextContent("Runs in the agent's shell (full access)");
    await user.clear(box);
    await user.type(box, "/");
    expect(screen.queryByTestId("shell-mode")).toBeNull();
  });

  it("without the agent's shell the hint names the host and the session's folder", async () => {
    const user = userEvent.setup();
    const { box, s } = await openSession("migrate-auth-middleware");
    await user.type(box, "!");
    expect(screen.getByTestId("shell-hint")).toHaveTextContent(`Runs on thinkpad-x1 in ${s.snap.cwd}`);
    expect(screen.queryByText(/full access/)).toBeNull();
  });

  it("with an agent that runs its own shell, the command is sent as RunShell and the host runs nothing", async () => {
    const user = userEvent.setup();
    const { backend, box } = await openSession("ws-reconnect-backoff");
    await user.type(box, "!git status");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(calls(backend, "session.command")).toHaveLength(1));
    expect(calls(backend, "session.command")[0].params).toEqual({ id: expect.any(String), command: { command: "run_shell", line: "git status" } });
    expect(calls(backend, "host.exec.start")).toEqual([]);
    expect(box).toHaveValue("");
  });

  it("without it, the command runs on the session's host in its folder, streams, and cancelling it is recorded", async () => {
    const user = userEvent.setup();
    const { backend, box, s } = await openSession("migrate-auth-middleware");
    await user.type(box, "!make test");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(calls(backend, "host.exec.start")).toHaveLength(1));
    expect(calls(backend, "host.exec.start")[0]).toMatchObject({ host: "x1", params: { cwd: s.snap.cwd, line: "make test" } });
    expect(calls(backend, "session.command")).toEqual([]);
    expect(box).toHaveValue("");

    const panel = await screen.findByTestId("shell-run");
    expect(within(panel).getByTestId("shell-run-state")).toHaveTextContent("Running…");
    await waitFor(() => expect(backend.subscriptions.map((x) => x.topic)).toContain("host.exec:ex1"));

    act(() => backend.execs.output("ex1", "building\n"));
    act(() => backend.execs.output("ex1", "3 tests\n", "stderr"));
    expect(await screen.findByTestId("shell-output")).toHaveTextContent("building 3 tests");

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(calls(backend, "host.exec.cancel")).toEqual([expect.objectContaining({ host: "x1", params: { exec_id: "ex1" } })]));
    expect(await screen.findByText("Cancelled")).toBeInTheDocument();
    expect(backend.subscriptions.map((x) => x.topic)).not.toContain("host.exec:ex1");

    // The run is in the conversation: the host journals it as the session's own event.
    await waitFor(() => expect(screen.getByTestId("shell-run-record")).toHaveTextContent("Saved to the conversation"));
    expect(calls(backend, "session.local_shell")).toEqual([
      expect.objectContaining({
        host: "x1",
        params: { session: s.id, command: "make test", cwd: s.snap.cwd, exit_code: null, output: "building\n3 tests\n", output_truncated: false },
      }),
    ]);
  });

  it("a finished command shows its exit code and is recorded with it; the time limit says so", async () => {
    const user = userEvent.setup();
    const { backend, box, s } = await openSession("migrate-auth-middleware");
    await user.type(box, "!npm test");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(calls(backend, "host.exec.start")).toHaveLength(1));
    act(() => backend.execs.output("ex1", "ok\n"));
    act(() => backend.execs.exit("ex1", { code: 3 }));
    expect(await screen.findByText("Exit code 3")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId("shell-run-record")).toHaveTextContent("Saved to the conversation"));
    expect(calls(backend, "session.local_shell")[0].params).toMatchObject({ session: s.id, command: "npm test", exit_code: 3, output: "ok\n" });

    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    await user.type(box, "!long-job");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(calls(backend, "host.exec.start")).toHaveLength(2));
    act(() => backend.execs.exit("ex2", { code: null, signal: 9, reason: "time_limit" }));
    expect(await screen.findByText("Stopped at the 1-hour limit")).toBeInTheDocument();
  });

  it("while a command runs here, another one is not started", async () => {
    const user = userEvent.setup();
    const { backend, box } = await openSession("migrate-auth-middleware");
    await user.type(box, "!sleep 100");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(calls(backend, "host.exec.start")).toHaveLength(1));
    await user.type(box, "!ls");
    expect(await screen.findByTestId("shell-blocked")).toHaveTextContent("A command is still running here");
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(calls(backend, "host.exec.start")).toHaveLength(1);
  });

  it("a session that has ended shows the command's end and does not save it to the conversation", async () => {
    const user = userEvent.setup();
    const { backend, box, s } = await openSession("upgrade-tauri-2-5");
    await backend.request("sessions.stop", { id: s.id }, { host: s.host });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message" })).toHaveAttribute("placeholder", expect.stringContaining("ended")));
    await user.type(box, "!ls");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(calls(backend, "host.exec.start")).toHaveLength(1));
    act(() => backend.execs.exit("ex1", { code: 0 }));
    expect(await screen.findByTestId("shell-run-record")).toHaveTextContent("Not saved to the conversation (the session has ended)");
    expect(calls(backend, "session.local_shell")).toEqual([]);
  });

  it("attachments are refused in shell mode, both pasted into a command and added before the !", async () => {
    const user = userEvent.setup();
    const { backend, box } = await openSession("migrate-auth-middleware");
    await user.type(box, "!ls");
    pastePng(box);
    expect(await screen.findByTestId("shell-refused")).toHaveTextContent("Shell commands can't take attachments.");
    expect(screen.queryAllByTestId("pending-attachment")).toHaveLength(0);
    expect(screen.getByRole("button", { name: "Shell commands take no attachments" })).toBeDisabled();

    await user.clear(box);
    pastePng(box);
    expect(await screen.findAllByTestId("pending-attachment")).toHaveLength(1);
    await user.type(box, "!");
    expect(await screen.findByText(SHELL_WITH_FILES)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await user.keyboard("{Enter}");
    expect(calls(backend, "host.exec.start")).toEqual([]);
    expect(calls(backend, "session.command")).toEqual([]);
  });
});
