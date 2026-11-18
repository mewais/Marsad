import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { Gate } from "../../src/features/gates/Gate";
import type { GateDensity } from "../../src/features/gates/Gate";
import { permissionChoices } from "../../src/features/gates/choices";
import type { GateOf, GateRequest } from "../../src/bridge/wire";
import { memoryHistory, RouterProvider } from "../../src/router";
import { AppProvider, createApp, selectView } from "../../src/state";
import { quietBackend, sessionByName } from "../helpers";

async function setup(name: string, density: GateDensity = "full", gate?: GateRequest) {
  localStorage.clear();
  const backend = quietBackend();
  const app = createApp(backend);
  const s = sessionByName(backend, name);
  await waitFor(() => expect(app.store.get().sessions[s.key]).toBeDefined());
  await waitFor(() => expect(app.store.get().drivers[s.snap.driver]).toBeDefined());
  if (gate) {
    backend.raiseGate(s.host, s.id, gate);
    await waitFor(() => expect(app.store.get().sessions[s.key].pending_gates.some((g) => g.gate.request_id === gate.request_id)).toBe(true));
  }
  const view = () => selectView(app.store.get(), s.key)!;
  const pg = () => (gate ? view().gates.find((g) => g.gate.request_id === gate.request_id)! : view().gates[0]);
  render(
    <AppProvider value={app}>
      <RouterProvider history={memoryHistory()}>
        <Gate view={view()} pg={pg()} density={density} active expanded />
      </RouterProvider>
    </AppProvider>,
  );
  const sent = () => backend.requests.filter((r) => r.method === "session.command");
  return { backend, app, s, sent };
}

describe("gate components", () => {
  it("permission (Claude): no wording on the wire, so Marsad's labels plus the agent's own rule text", async () => {
    const user = userEvent.setup();
    const { sent, s } = await setup("pdf-renderer-worker");
    expect(screen.getByText("Marsad standard choices")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Always allow Bash\(pnpm add:\*\)/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Deny/ })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Allow\s*1?$/ }));
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0].params).toMatchObject({ id: s.id, command: { command: "answer_gate", request_id: "p-pdf", decision: { action: "approve" } } });
  });

  it("permission: deny with a reason needs a second click and carries the reason", async () => {
    const user = userEvent.setup();
    const { sent } = await setup("pdf-renderer-worker");
    const deny = screen.getByRole("button", { name: /^Deny/ });
    await user.click(deny);
    expect(sent().length).toBe(0);
    await user.type(screen.getByLabelText(/Tell Claude Code what to do differently/), "use npm instead");
    await user.click(deny);
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0].params).toMatchObject({ command: { decision: { action: "deny", reason: "use npm instead" } } });
  });

  it("permission (Claude): 'always allow' echoes the agent's suggestions verbatim", async () => {
    const user = userEvent.setup();
    const { sent, s } = await setup("pdf-renderer-worker");
    await user.click(screen.getByRole("button", { name: /Always allow/ }));
    await waitFor(() => expect(sent()[0].params).toMatchObject({ command: { decision: { action: "approve", permission_updates: (s.snap.pending_gates[0].gate as GateOf<"permission">).permission_suggestions } } }));
  });

  it("permission (Codex): no suggestions, so only Allow and Deny", async () => {
    await setup("ws-reconnect-backoff");
    const labels = within(screen.getByRole("group", { name: /choices/i })).getAllByRole("button").map((b) => b.textContent?.replace(/\d$/, ""));
    expect(labels.map((l) => l?.replace("…", ""))).toEqual(["Allow", "Deny"]);
  });

  it("falls back to Marsad's standard choices and says so when the agent sent none", () => {
    const base = { gate_type: "permission" as const, request_id: "r", tool_name: "Bash", command: "ls", cwd: ".", explanation: "", is_elicitation: false, raw_input: null };
    const set = permissionChoices(base);
    expect(set.source).toBe("marsad");
    const acp = permissionChoices({ ...base, tool_name: "x", permission_suggestions: [{ optionId: "allow_once", name: "Allow once", kind: "allow_once" }, { optionId: "reject_once", name: "Reject", kind: "reject_once" }] });
    expect(acp.source).toBe("agent");
    expect(acp.choices.map((c) => c.kind)).toEqual(["allow", "deny"]);
    expect(acp.choices[0].decision()).toEqual({ action: "approve", elicitation_content: { optionId: "allow_once" } });
  });

  it("single-choice question: pick an option and send the answer keyed by question text", async () => {
    const user = userEvent.setup();
    const { sent } = await setup("migrate-auth-middleware");
    const send = screen.getByRole("button", { name: /Send answer/ });
    expect(send).toBeDisabled();
    await user.click(screen.getByRole("radio", { name: /Migrate on first request/ }));
    await user.click(send);
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0].params).toMatchObject({ command: { command: "answer_question", request_id: "q-cookies", answer: { answers: { "How should legacy session cookies be handled during rollout?": "Migrate on first request" } } } });
  });

  it("question with free-text Other sends what was typed", async () => {
    const user = userEvent.setup();
    const { sent } = await setup("migrate-auth-middleware");
    await user.type(screen.getByLabelText("Other answer"), "Keep both for a week");
    await user.click(screen.getByRole("button", { name: /Send answer/ }));
    await waitFor(() => expect(sent().length).toBe(1));
    expect(JSON.stringify(sent()[0].params)).toContain("Keep both for a week");
  });

  it("multi-choice question joins the selected labels", async () => {
    const user = userEvent.setup();
    const gate: GateRequest = { gate_type: "question", request_id: "q-multi", questions: [{ question: "Which targets?", header: "Targets", multi_select: true, options: [{ label: "linux", description: "" }, { label: "macos", description: "" }, { label: "windows", description: "" }] }] };
    const { sent } = await setup("csv-schema-worker", "full", gate);
    await user.click(screen.getByRole("checkbox", { name: /linux/ }));
    await user.click(screen.getByRole("checkbox", { name: /windows/ }));
    await user.click(screen.getByRole("button", { name: /Send answer/ }));
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0].params).toMatchObject({ command: { answer: { answers: { "Which targets?": "linux, windows" } } } });
  });

  it("plan approval: approve or keep planning", async () => {
    const user = userEvent.setup();
    const gate: GateRequest = { gate_type: "plan", request_id: "pl1", summary: "1. Add shim\n2. Migrate routes" };
    const { sent } = await setup("csv-schema-worker", "full", gate);
    expect(screen.getByText(/Add shim/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Approve plan/ }));
    await waitFor(() => expect(sent()[0].params).toMatchObject({ command: { request_id: "pl1", decision: { action: "approve" } } }));
  });

  it("handoff: Marsad's three fixed choices, answered on the sending session like any gate", async () => {
    const user = userEvent.setup();
    const { sent, s } = await setup("invoice-export-orchestrator");
    expect(screen.getByText("Marsad handoff choices")).toBeInTheDocument();
    expect(within(screen.getByRole("group", { name: "Handoff choices" })).getAllByRole("button")).toHaveLength(3);
    await user.click(screen.getByRole("button", { name: /Deny/ }));
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0]).toMatchObject({ method: "session.command", host: "x1", params: { id: s.id, command: { command: "answer_gate", request_id: "wasl-handoff-t4print", decision: { action: "deny" } } } });
  });

  it("shows a pending state while the request is in flight and a toast when the host is unreachable", async () => {
    const user = userEvent.setup();
    localStorage.clear();
    const backend = quietBackend({ latencyMs: 30 });
    const app = createApp(backend);
    const s = sessionByName(backend, "windows-print-check");
    await waitFor(() => expect(app.store.get().sessions[s.key]).toBeDefined());
    backend.raiseGate(s.host, s.id, { gate_type: "plan", request_id: "pl-win", summary: "Print both sizes" });
    await waitFor(() => expect(app.store.get().sessions[s.key].pending_gates.length).toBe(1));
    const view = selectView(app.store.get(), s.key)!;
    // Answer through the session's own host to hit the unreachable win host.
    const { ToastRegion } = await import("../../src/ui");
    render(<AppProvider value={app}><RouterProvider history={memoryHistory()}><Gate view={view} pg={view.gates[0]} density="full" active /><ToastRegion /></RouterProvider></AppProvider>);
    void user;
    const ok = app.actions.answerGate(view.key, "pl-win", { action: "approve" });
    await waitFor(() => expect(Object.values(app.store.get().pending)).toContain("pending"));
    expect(await ok).toBe(false);
    expect(await screen.findByRole("alert")).toHaveTextContent(/win-build is not connected/);
    expect(Object.keys(app.store.get().pending)).toHaveLength(0);
  });
});
