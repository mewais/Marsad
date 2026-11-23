import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { WorkstreamView } from "../../src/bridge/wire";
import { SpawnHistory } from "../../src/features/workstreams/Spawning";
import { quietBackend, renderApp, sessionByName } from "../helpers";

const ORCHESTRATOR = "invoice-export-orchestrator";

describe("workstream: why a spawn request went away", () => {
  it("lists the ended requests under the waiting one: who asked, for what name, when, and how it ended in words", async () => {
    const r = await renderApp("/inbox", quietBackend());
    act(() => r.history.push("/workstreams/ws-ci"));
    await screen.findByTestId("graph-canvas", {}, { timeout: 4000 });
    const hist = await screen.findByTestId("spawn-history");
    const row = within(hist).getByTestId("spawn-outcome");
    expect(row).toHaveAttribute("data-end", "launched");
    expect(row).toHaveTextContent("ci-triage-lead asked for “ci-repro-windows”");
    expect(row).toHaveTextContent("It was started.");
    expect(row).toHaveTextContent(/\d\d:\d\d$/);
  });

  it("a request the human denied, and one whose asking turn was interrupted, are each listed with their own reason (newest first)", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/workstreams/ws-invoice"));
    await screen.findByTestId("graph-canvas", {}, { timeout: 4000 });
    act(() => backend.startSpawnScenario());
    // The asking agent's panel: its waiting request, and (below it) the ones that ended.
    await user.click(document.querySelector('[data-node="orchestrator"]') as HTMLElement);
    expect(screen.queryByTestId("spawn-history")).toBeNull(); // nothing has ended yet
    const gate = await screen.findByTestId("spawn-gate");
    await user.click(within(gate).getByRole("button", { name: /^Deny/ }));
    const hist = await screen.findByTestId("spawn-history");
    expect(within(hist).getAllByTestId("spawn-outcome").map((o) => o.getAttribute("data-end"))).toEqual(["denied"]);
    expect(hist).toHaveTextContent("invoice-export-orchestrator asked for “pdf-font-checker”");
    expect(hist).toHaveTextContent("It was denied.");
    // A second request waits and its turn is interrupted: the daemon takes the gate down and says why.
    act(() => backend.startSpawnScenario());
    await waitFor(() => expect(screen.getByTestId("spawn-gate")).toBeInTheDocument());
    const s = sessionByName(backend, ORCHESTRATOR);
    await act(async () => { await backend.request("session.command", { id: s.id, command: { command: "interrupt" } }, { host: s.host }); });
    await waitFor(() => expect(within(screen.getByTestId("spawn-history")).getAllByTestId("spawn-outcome")).toHaveLength(2));
    const rows = within(screen.getByTestId("spawn-history")).getAllByTestId("spawn-outcome");
    expect(rows.map((o) => o.getAttribute("data-end"))).toEqual(["turn_interrupted", "denied"]);
    expect(rows[0]).toHaveTextContent("The turn that asked was interrupted before the request was answered.");
    await waitFor(() => expect(screen.queryByTestId("spawn-gate")).toBeNull());
  });

  it("the asking agent's own panel lists its requests too", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/workstreams/ws-ci"));
    await screen.findByTestId("graph-canvas", {}, { timeout: 4000 });
    await user.click(document.querySelector('[data-node="lead"]') as HTMLElement);
    const hist = await screen.findByTestId("spawn-history");
    expect(hist).toHaveTextContent("ci-triage-lead asked for “ci-repro-windows”");
    await user.click(document.querySelector('[data-node="repro"]') as HTMLElement); // an agent that asked for nothing has no list
    await waitFor(() => expect(screen.queryByTestId("spawn-history")).toBeNull());
  });
});

describe("a spawn outcome in words", () => {
  const view = (recent: WorkstreamView["recent_spawns"]) => ({ workstream: { nodes: [{ id: "lead", name: "ci-triage-lead" }] }, recent_spawns: recent }) as unknown as WorkstreamView;
  const outcome = (over: Partial<WorkstreamView["recent_spawns"][number]>): WorkstreamView["recent_spawns"][number] => ({ request_id: "r1", node: "lead", host: "h", session: "s", name: "scout", ended_ms: Date.UTC(2026, 10, 1, 12, 5), end: { code: "launched" }, ...over });

  it("says a refusal with what the supervisor said and its code, a failed launch the same, and a code it does not know generically", () => {
    render(
      <SpawnHistory view={view([
        outcome({ request_id: "a", end: { code: "refused", cause: "spawn_limit" }, detail: "Spawning is limited to 8 agents in this workstream." }),
        outcome({ request_id: "b", end: { code: "launch_failed", cause: "spawn_unavailable" } }),
        outcome({ request_id: "c", end: { code: "something_new" as never } }),
      ])} />,
    );
    const rows = screen.getAllByTestId("spawn-outcome");
    expect(rows[0]).toHaveTextContent("It was refused. Spawning is limited to 8 agents in this workstream. (spawn_limit)");
    expect(within(rows[0]).getByTestId("spawn-cause")).toHaveTextContent("(spawn_limit)");
    expect(rows[1]).toHaveTextContent("It was approved, but the agent could not be started. (spawn_unavailable)");
    expect(rows[2]).toHaveTextContent("It ended for a reason this version does not know (something_new).");
    expect(within(rows[2]).queryByTestId("spawn-cause")).toBeNull();
  });

  it("shows nothing when nothing has ended", () => {
    const { container } = render(<SpawnHistory view={view([])} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("a gate on a session that someone else took down", () => {
  it("the session says why in its transcript, and the open card's notice names the outcome instead of the generic one", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, ORCHESTRATOR);
    const r = await renderApp(`/session/${s.host}/${s.id}`, backend);
    await screen.findByRole("heading", { name: ORCHESTRATOR });
    act(() => backend.startSpawnScenario());
    await waitFor(() => expect(document.querySelector('[data-gate="spawn"]')).not.toBeNull());
    await act(async () => { await backend.request("session.command", { id: s.id, command: { command: "interrupt" } }, { host: s.host }); });
    await waitFor(() => expect(document.querySelector('[data-gate="spawn"]')).toBeNull());
    // The notice the card leaves behind: the outcome, not "no longer waits".
    expect((await screen.findAllByText("Request ended")).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/The request to start pdf-font-checker ended without your answer\. The turn that asked was interrupted before the request was answered\./).length).toBeGreaterThan(0);
    expect(screen.queryByText("No longer needed")).toBeNull();
    // And the transcript keeps it, in order.
    const tr = document.querySelector(".transcript") as HTMLElement;
    expect(within(tr).getByText("The request to start pdf-font-checker ended without your answer. The turn that asked was interrupted before the request was answered.")).toBeInTheDocument();
    void r;
  });

  it("a gate the human answered, or the protocol resolved, leaves no 'taken down' notice (there was no reason)", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, "migrate-auth-middleware");
    await renderApp(`/session/${s.host}/${s.id}`, backend);
    await screen.findByRole("heading", { name: "migrate-auth-middleware" });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("radio", { name: /Migrate on first request/ }));
    await user.click(within(document.querySelector('[data-gate="question"]') as HTMLElement).getByRole("button", { name: /Send/ }));
    await waitFor(() => expect(backend.requests.some((q) => q.method === "session.command")).toBe(true));
    await waitFor(() => expect(document.querySelector(".transcript")?.textContent).not.toMatch(/ended without your answer/));
  });

  it("legacy: a journal's bare 'withdrawn' is a reason like any other, in words", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, ORCHESTRATOR);
    await renderApp(`/session/${s.host}/${s.id}`, backend);
    await screen.findByRole("heading", { name: ORCHESTRATOR });
    act(() => backend.startSpawnScenario());
    await waitFor(() => expect(document.querySelector('[data-gate="spawn"]')).not.toBeNull());
    act(() => backend.withdrawGate(s.host, s.id, "wasl-spawn-fx-fonts"));
    const tr = document.querySelector(".transcript") as HTMLElement;
    await waitFor(() => expect(tr.textContent).toContain("The request to start pdf-font-checker ended without your answer. The request was taken down by its owner; no reason was given."));
  });
});
