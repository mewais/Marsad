import { permissionKind, permissionLine, permissionTitle } from "../../src/lib/gatewords";
import { sessionName } from "../../src/state/domain";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { Gate } from "../../src/features/gates/Gate";
import type { GateDensity } from "../../src/features/gates/Gate";
import { gateChoices } from "../../src/features/gates/choices";
import { installShortcuts } from "../../src/shortcuts/registry";
import type { GateRequest } from "../../src/bridge/wire";
import { claudeChoices, SPAWN_CHOICES } from "../../src/bridge/fixture/sessions";
import { memoryHistory, RouterProvider } from "../../src/router";
import { AppProvider, createApp, selectView } from "../../src/state";
import { quietBackend, sessionByName } from "../helpers";

async function setup(name: string, density: GateDensity = "full", gate?: GateRequest, expanded = true) {
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
        <Gate view={view()} pg={pg()} density={density} active expanded={expanded} />
      </RouterProvider>
    </AppProvider>,
  );
  const sent = () => backend.requests.filter((r) => r.method === "session.command");
  return { backend, app, s, sent };
}

describe("gate components", () => {
  it("permission (Claude): the driver's choices with neutral labels, and the rules it would remember", async () => {
    const user = userEvent.setup();
    const { sent, s } = await setup("pdf-renderer-worker");
    expect(screen.getByText(/Claude Code's choices · neutral labels/)).toBeInTheDocument();
    expect(screen.queryByText(/Marsad standard choices/)).toBeNull();
    expect(screen.getByRole("button", { name: /^Allow and remember/ })).toHaveAttribute("title", "allow rule Bash(pnpm add:*) (localSettings)");
    expect(screen.getByRole("button", { name: /^Deny\s*\d?$/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Deny and stop the turn/ })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Allow\s*1?$/ }));
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0].params).toMatchObject({ id: s.id, command: { command: "answer_gate", request_id: "p-pdf", decision: { choice: "allow" } } });
  });

  it("permission: a choice that takes optional text sends what was typed with it", async () => {
    const user = userEvent.setup();
    const { sent } = await setup("pdf-renderer-worker");
    await user.type(screen.getByLabelText(/Tell Claude Code what to do differently/), "use npm instead");
    await user.click(screen.getByRole("button", { name: /^Deny\s*\d?$/ }));
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0].params).toMatchObject({ command: { decision: { choice: "deny", text: "use npm instead" } } });
  });

  it("permission (Claude): 'allow and remember' picks the driver's choice by id", async () => {
    const user = userEvent.setup();
    const { sent } = await setup("pdf-renderer-worker");
    await user.click(screen.getByRole("button", { name: /Allow and remember/ }));
    await waitFor(() => expect(sent()[0].params).toMatchObject({ command: { decision: { choice: "allow_always" } } }));
  });

  describe("what a choice applies beyond the answer", () => {
    const EFFECTS = ["allow rule Bash(pnpm test:*) (localSettings)", "add directory ~/src/ledgerline (session)", "switch to mode acceptEdits (session)"];
    const remembered = (effects: string[]): GateRequest => ({
      gate_type: "permission", request_id: "p-effects", tool_name: "Bash", command: "pnpm test", cwd: "~/src/ledgerline", explanation: "", raw_input: { command: "pnpm test" },
      choices: claudeChoices("allow rule Bash(pnpm test:*) (localSettings)", effects),
    });

    it.each([
      ["the session view", "full", true],
      ["the inbox row, expanded", "row", true],
      ["the inbox row, one line", "row", false],
    ] as const)("is listed under its label before any click (%s)", async (_where, density, expanded) => {
      const user = userEvent.setup();
      const { sent, s } = await setup("csv-schema-worker", density, remembered(EFFECTS), expanded);
      const block = document.querySelector('[data-effects="allow_always"]') as HTMLElement;
      expect(block).toHaveTextContent("Allow and remember will also:");
      expect(within(block).getAllByRole("listitem").map((li) => li.textContent)).toEqual(EFFECTS);
      const remember = screen.getByRole("button", { name: /^Allow and remember/ });
      expect(remember).toHaveAttribute("aria-describedby", block.id);
      expect(sent()).toHaveLength(0);
      await user.click(remember);
      await waitFor(() => expect(sent().length).toBe(1));
      expect(sent()[0].params).toMatchObject({ id: s.id, command: { command: "answer_gate", request_id: "p-effects", decision: { choice: "allow_always" } } });
    });

    it("its number key still answers with it", async () => {
      const user = userEvent.setup();
      // The global key listener is installed by the App; this test mounts the gate alone.
      const off = installShortcuts();
      try {
        const { sent } = await setup("csv-schema-worker", "full", remembered(EFFECTS));
        await user.keyboard("2");
        await waitFor(() => expect(sent().length).toBe(1));
        expect(sent()[0].params).toMatchObject({ command: { request_id: "p-effects", decision: { choice: "allow_always" } } });
      } finally {
        off();
      }
    });

    it("a gate whose choices have no effects renders no such block", async () => {
      const gate: GateRequest = { gate_type: "permission", request_id: "p-plain", tool_name: "Bash", command: "pnpm test", cwd: "~/src/ledgerline", explanation: "", raw_input: null, choices: claudeChoices() };
      await setup("csv-schema-worker", "full", gate);
      expect(document.querySelector(".choice-effects")).toBeNull();
      expect(screen.getByRole("button", { name: /^Allow/ })).not.toHaveAttribute("aria-describedby");
    });
  });

  it("permission (Codex): exactly the decisions codex offers", async () => {
    await setup("ws-reconnect-backoff");
    const labels = within(screen.getByRole("group", { name: /choices/i })).getAllByRole("button").map((b) => b.textContent?.replace(/\d$/, ""));
    expect(labels).toEqual(["Allow", "Allow and remember", "Deny and stop the turn"]);
  });

  it("uses the agent's own wording when its protocol has wording (ACP options)", () => {
    const base = { gate_type: "permission" as const, request_id: "r", tool_name: "bash", command: "ls", cwd: ".", explanation: "", raw_input: null };
    expect(gateChoices({ ...base, choices: claudeChoices() }).source).toBe("neutral");
    const acp = gateChoices({ ...base, choices: [{ id: "once", label: "Allow once", kind: "allow_once" }, { id: "always", label: "Always allow", kind: "allow_always" }, { id: "reject", label: "Reject", kind: "reject_once" }] });
    expect(acp.source).toBe("agent");
    expect(acp.choices.map((c) => c.kind)).toEqual(["allow", "always", "deny"]);
    expect(acp.choices[0].decision()).toEqual({ choice: "once" });
    expect(acp.choices[2].decision("ignored")).toEqual({ choice: "reject" });
  });

  it("single-choice question: pick an option and send the answer keyed by question text", async () => {
    const user = userEvent.setup();
    const { sent } = await setup("migrate-auth-middleware");
    const send = screen.getByRole("button", { name: /Send answer/ });
    expect(send).toBeDisabled();
    await user.click(screen.getByRole("radio", { name: /Migrate on first request/ }));
    await user.click(send);
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0].params).toMatchObject({ command: { command: "answer_question", request_id: "q-cookies", answer: { answers: { "How should legacy session cookies be handled during rollout?": ["Migrate on first request"] } } } });
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
    const gate: GateRequest = { gate_type: "question", request_id: "q-multi", questions: [{ id: "targets", question: "Which targets?", header: "Targets", multi_select: true, allow_other: false, options: [{ label: "linux", description: "" }, { label: "macos", description: "" }, { label: "windows", description: "" }] }] };
    const { sent } = await setup("csv-schema-worker", "full", gate);
    await user.click(screen.getByRole("checkbox", { name: /linux/ }));
    await user.click(screen.getByRole("checkbox", { name: /windows/ }));
    await user.click(screen.getByRole("button", { name: /Send answer/ }));
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0].params).toMatchObject({ command: { answer: { answers: { targets: ["linux", "windows"] } } } });
    expect(screen.queryByLabelText("Other answer")).toBeNull();
  });

  it("plan approval: approve or keep planning", async () => {
    const user = userEvent.setup();
    const gate: GateRequest = { gate_type: "plan", request_id: "pl1", summary: "1. Add shim\n2. Migrate routes", choices: claudeChoices().map((c) => (c.id === "allow" ? { ...c, label: "Approve plan" } : c.id === "deny" ? { ...c, label: "Keep planning" } : c)) };
    const { sent } = await setup("csv-schema-worker", "full", gate);
    expect(screen.getByText(/Add shim/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Approve plan/ }));
    await waitFor(() => expect(sent()[0].params).toMatchObject({ command: { request_id: "pl1", decision: { choice: "allow" } } }));
  });

  it("handoff: Wasl's choices, answered on the sending session like any gate", async () => {
    const user = userEvent.setup();
    const { sent, s } = await setup("invoice-export-orchestrator");
    expect(screen.getByText("Marsad handoff choices")).toBeInTheDocument();
    expect(within(screen.getByRole("group", { name: /choices/i })).getAllByRole("button")).toHaveLength(3);
    await user.click(screen.getByRole("button", { name: /^Deny/ }));
    await waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0]).toMatchObject({ method: "session.command", host: "x1", params: { id: s.id, command: { command: "answer_gate", request_id: "wasl-handoff-t4print", decision: { choice: "deny" } } } });
  });

  it("shows a pending state while the request is in flight and a toast when the host refuses it", async () => {
    const user = userEvent.setup();
    localStorage.clear();
    const backend = quietBackend({ latencyMs: 30 });
    const app = createApp(backend);
    const s = sessionByName(backend, "upgrade-tauri-2-5");
    await waitFor(() => expect(app.store.get().sessions[s.key]).toBeDefined());
    backend.raiseGate(s.host, s.id, { gate_type: "plan", request_id: "pl-x1", summary: "Print both sizes", choices: claudeChoices() });
    await waitFor(() => expect(app.store.get().sessions[s.key].pending_gates.length).toBe(1));
    const view = selectView(app.store.get(), s.key)!;
    // The host answers the command with an error (it went away between the click and the answer).
    const request = backend.request.bind(backend);
    backend.request = (<T,>(method: string, params?: unknown, opts?: object) =>
      method === "session.command" ? new Promise<T>((_, reject) => setTimeout(() => reject({ code: "upstream", message: "x1 dropped the connection" }), 30)) : request<T>(method, params, opts)) as typeof backend.request;
    const { ToastRegion } = await import("../../src/ui");
    render(<AppProvider value={app}><RouterProvider history={memoryHistory()}><Gate view={view} pg={view.gates[0]} density="full" active /><ToastRegion /></RouterProvider></AppProvider>);
    void user;
    const ok = app.actions.answerGate(view.key, "pl-x1", { choice: "allow" });
    await waitFor(() => expect(Object.values(app.store.get().pending)).toContain("pending"));
    expect(await ok).toBe(false);
    expect(await screen.findByRole("alert")).toHaveTextContent(/x1 dropped the connection/);
    expect(Object.keys(app.store.get().pending)).toHaveLength(0);
  });

  it("a host that is not connected is not asked: the answer is refused at once with the reason, and the buttons say why", async () => {
    localStorage.clear();
    const backend = quietBackend();
    const app = createApp(backend);
    const s = sessionByName(backend, "windows-print-check");
    await waitFor(() => expect(app.store.get().sessions[s.key]).toBeDefined());
    backend.raiseGate(s.host, s.id, { gate_type: "plan", request_id: "pl-win", summary: "Print both sizes", choices: claudeChoices() });
    await waitFor(() => expect(app.store.get().sessions[s.key].pending_gates.length).toBe(1));
    const view = selectView(app.store.get(), s.key)!;
    const { ToastRegion } = await import("../../src/ui");
    render(<AppProvider value={app}><RouterProvider history={memoryHistory()}><Gate view={view} pg={view.gates[0]} density="full" active /><ToastRegion /></RouterProvider></AppProvider>);
    for (const b of within(screen.getByRole("group", { name: /choices/i })).getAllByRole("button")) {
      expect(b).toBeDisabled();
      expect(b).toHaveAttribute("title", expect.stringMatching(/win-build is not connected/));
    }
    const before = backend.requests.length;
    expect(await app.actions.answerGate(view.key, "pl-win", { choice: "allow" })).toBe(false);
    expect(await within(screen.getByLabelText("Notifications")).findByText(/win-build is not connected/)).toBeInTheDocument();
    expect(backend.requests.length).toBe(before);
    expect(Object.keys(app.store.get().pending)).toHaveLength(0);
  });
});

describe("permission gate wording", () => {
  const perm = (over: Record<string, unknown>) => ({ gate_type: "permission", request_id: "r", tool_name: "question", command: "Which cookie migration should I implement?", cwd: "", explanation: "", choices: [], raw_input: null, ...over }) as Parameters<typeof permissionKind>[0];
  it("an ACP permission-with-options that is a question reads as a question, never 'Run'", () => {
    const q = perm({ tool_name: "question", raw_input: { questions: [{ question: "Which cookie migration should I implement?" }] } });
    expect(permissionKind(q)).toBe("question");
    expect(permissionLine(q)).toEqual({ prefix: "", text: "Which cookie migration should I implement?", code: false });
    expect(permissionTitle(q)).toBe("Question");
  });
  it("only command executions say Run; other tools name the tool", () => {
    expect(permissionLine(perm({ tool_name: "Bash", command: "pnpm test", raw_input: { command: "pnpm test" } })).prefix).toBe("Run ");
    expect(permissionLine(perm({ tool_name: "execute", command: "ls", raw_input: null })).prefix).toBe("Run ");
    expect(permissionLine(perm({ tool_name: "Edit", command: "src/a.ts", raw_input: { file_path: "src/a.ts" } })).prefix).toBe("Edit: ");
  });
  it("a session without a name is never shown by its id", () => {
    const s = { id: "4f1c9a2e-77", display_name: null, title: null, cwd: "/w/auth-service", turn: null } as unknown as Parameters<typeof sessionName>[0];
    expect(sessionName(s)).toBe("New session in auth-service");
  });
});

describe("spawn gates", () => {
  it("says who wants to start whom, where, with what and why; a standalone asker sees the new workstream; deny carries a reason", async () => {
    const user = userEvent.setup();
    const gate: GateRequest = {
      gate_type: "spawn", request_id: "wasl-spawn-t1", from: "planner", name: "scout", role: "Looks around the repo", host: "hetzner-ax41", driver: "Claude Code", model: "haiku", cwd: "~/src/x", mode: null, instruction: null,
      workstream: "planner + workers", new_workstream: true, details: null, choices: SPAWN_CHOICES,
    };
    const { sent } = await setup("tile-cache-eviction", "row", gate);
    const row = document.querySelector('[data-gate="spawn"]') as HTMLElement;
    expect(row).toHaveTextContent("Spawn a worker · planner wants to start scout on hetzner-ax41");
    expect(within(row).getByTestId("spawn-facts")).toHaveTextContent("Claude Code · haiku");
    expect(row).toHaveTextContent("approving creates the workstream “planner + workers”");
    expect(within(row).getByText("Marsad spawn choices")).toBeInTheDocument();
    await user.type(within(row).getByRole("textbox"), "not now");
    await user.click(within(row).getByRole("button", { name: /^Deny/ }));
    await waitFor(() => expect(sent().slice(-1)[0]?.params).toMatchObject({ command: { command: "answer_gate", request_id: "wasl-spawn-t1", decision: { choice: "deny", text: "not now" } } }));
  });
});

describe("a gate that goes away while it is open", () => {
  const spawn: GateRequest = {
    gate_type: "spawn", request_id: "wasl-spawn-gone", from: "planner", name: "scout", role: "Looks around the repo", host: "hetzner-ax41", driver: "Claude Code", model: null, cwd: "~/src/x", mode: null, instruction: null,
    workstream: "Invoice export v2", new_workstream: false, details: null, choices: SPAWN_CHOICES,
  };

  async function openRow() {
    localStorage.clear();
    const { toastStore } = await import("../../src/ui/toast");
    toastStore.set([]);
    const backend = quietBackend();
    const app = createApp(backend);
    const s = sessionByName(backend, "tile-cache-eviction");
    await waitFor(() => expect(app.store.get().sessions[s.key]).toBeDefined());
    backend.raiseGate(s.host, s.id, spawn);
    await waitFor(() => expect(app.store.get().sessions[s.key].pending_gates.some((g) => g.gate.request_id === spawn.request_id)).toBe(true));
    const { ToastRegion } = await import("../../src/ui");
    const { InboxRow } = await import("../../src/features/inbox/InboxRow");
    render(
      <AppProvider value={app}>
        <RouterProvider history={memoryHistory()}>
          <InboxRow sessionKey={s.key} selected onSelect={() => {}} onOpen={() => {}} />
          <ToastRegion />
        </RouterProvider>
      </AppProvider>,
    );
    await waitFor(() => expect(document.querySelector('[data-gate="spawn"]')).not.toBeNull());
    return { backend, app, s };
  }

  it("withdrawn: the card leaves with a 'no longer needed' toast, and a late answer says the same instead of failing", async () => {
    const { backend, app, s } = await openRow();
    backend.withdrawGate(s.host, s.id, spawn.request_id);
    await waitFor(() => expect(document.querySelector('[data-gate="spawn"]')).toBeNull());
    expect(await screen.findByText("No longer needed")).toBeInTheDocument();
    expect(screen.getByText("planner no longer waits to start scout.")).toBeInTheDocument();
    // An answer already on its way (or sent from another view) meets the typed refusal.
    expect(await app.actions.answerGate(s.key, spawn.request_id, { choice: "allow_once" })).toBe(false);
    await waitFor(() => expect(screen.getByText(/withdrawn before your answer arrived/)).toBeInTheDocument());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("answered from here: the card leaves without a toast", async () => {
    const user = userEvent.setup();
    await openRow();
    const row = document.querySelector('[data-gate="spawn"]') as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /^Allow once/ }));
    await waitFor(() => expect(document.querySelector('[data-gate="spawn"]')).toBeNull());
    expect(screen.queryByText("No longer needed")).toBeNull();
  });
});
