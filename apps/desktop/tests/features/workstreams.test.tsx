import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { FixtureBackend } from "../../src/bridge/fixture/FixtureBackend";
import { arrange, labelSize, layoutGraph, totalCrossings, totalLabelOverlaps, type LayoutInput } from "../../src/features/workstreams/layout";
import { crossings, route } from "../../src/features/workstreams/route";
import { checklist, edgeLabel, summary } from "../../src/features/workstreams/model";
import type { WorkstreamView } from "../../src/bridge/wire";
import { quietBackend, renderApp } from "../helpers";

function views(): WorkstreamView[] {
  const b = new FixtureBackend({ live: false, latencyMs: 0, speed: 0 });
  const r = b.hub.ws.reset();
  b.dispose();
  return r.kind === "reset" ? r.workstreams : [];
}
const inputOf = (v: WorkstreamView, pos = false): LayoutInput => ({
  lanes: [...new Set(v.nodes.map((n) => n.host))].map((host) => ({ host })),
  nodes: v.workstream.nodes.map((n, i) => ({ id: n.id, host: v.nodes.find((x) => x.node === n.id)!.host, w: 230, h: 112, pos: pos ? { x: 200 + i * 270, y: 20 + (i % 2) * 40 } : null })),
  edges: v.workstream.edges.map((e) => ({ id: `${e.from}>${e.to}`, from: e.from, to: e.to })),
});

describe("graph layout and routing", () => {
  it("no edge crosses a node on the fixture workstreams (ELK placement)", async () => {
    for (const v of views()) {
      const l = await layoutGraph(inputOf(v));
      expect(totalCrossings(l)).toBe(0);
      expect(totalLabelOverlaps(l)).toBe(0);
      expect(Object.keys(l.edges)).toHaveLength(v.workstream.edges.length);
      // lanes do not overlap and every node sits inside its host's lane
      for (const n of v.nodes) {
        const lane = l.lanes.find((x) => x.host === n.host)!;
        const r = l.nodes[n.node];
        expect(r.y).toBeGreaterThanOrEqual(lane.y);
        expect(r.y + r.h).toBeLessThanOrEqual(lane.y + lane.h);
      }
    }
  });

  it("stored manual positions override the layout and edges still route around nodes", () => {
    for (const v of views()) {
      const l = arrange(inputOf(v, true), {});
      expect(totalCrossings(l)).toBe(0);
      expect(totalLabelOverlaps(l)).toBe(0);
    }
  });

  it("puts edge labels on a free stretch of their edge: never on a node or another label", () => {
    // The screenshot case: an edge runs down beside a node card to a node in the next lane.
    const input: LayoutInput = {
      lanes: [{ host: "vm" }, { host: "build" }],
      nodes: [
        { id: "orch", host: "vm", w: 230, h: 112, pos: { x: 340, y: 26 } },
        { id: "pdf", host: "vm", w: 230, h: 112, pos: { x: 300, y: 228 } },
        { id: "csv", host: "build", w: 230, h: 112, pos: { x: 430, y: 26 } },
        { id: "other", host: "build", w: 230, h: 112, pos: { x: 700, y: 26 } },
      ],
      edges: [
        { id: "orch>pdf", from: "orch", to: "pdf", label: labelSize("Ask each time · 1 task") },
        { id: "orch>csv", from: "orch", to: "csv", label: labelSize("Handoff waiting") },
        { id: "orch>other", from: "orch", to: "other", label: labelSize("Always allow · 2 tasks") },
        { id: "pdf>csv", from: "pdf", to: "csv", label: labelSize("Ask each time") },
      ],
    };
    const l = arrange(input, {});
    expect(totalCrossings(l)).toBe(0);
    expect(totalLabelOverlaps(l)).toBe(0);
    // each label's centre lies on its own edge
    for (const e of Object.values(l.edges)) {
      const on = e.points.some((p, i) => i > 0 && Math.min(p.x, e.points[i - 1].x) <= e.label.x && e.label.x <= Math.max(p.x, e.points[i - 1].x) && Math.min(p.y, e.points[i - 1].y) <= e.label.y && e.label.y <= Math.max(p.y, e.points[i - 1].y));
      expect(on).toBe(true);
    }
  });

  it("routes around a node that sits between source and target", () => {
    const a = { x: 0, y: 0, w: 100, h: 50 }, mid = { x: 0, y: 120, w: 100, h: 50 }, b = { x: 0, y: 240, w: 100, h: 50 };
    const p = route(a, b, [a, mid, b]);
    expect(crossings(p, [a, mid, b])).toBe(0);
    for (let i = 1; i < p.length; i++) expect(p[i].x === p[i - 1].x || p[i].y === p[i - 1].y).toBe(true);
  });
});

describe("workstream model", () => {
  it("summarises status in words and edges by policy and traffic", () => {
    const inv = views().find((v) => v.workstream.id === "ws-invoice")!;
    expect(summary(inv).parts.map((p) => p.text)).toEqual(["2 need you", "1 working"]);
    const e = inv.edges.find((x) => x.to === "print")!;
    expect(edgeLabel(e)).toMatchObject({ text: "Handoff waiting", pending: true });
    expect(edgeLabel(inv.edges.find((x) => x.to === "csv")!)).toMatchObject({ text: "Always allow", tasks: 1 });
    const docs = views().find((v) => v.workstream.id === "ws-docs")!;
    expect(summary(docs).parts[0].text).toBe("Draft, not launched");
    const cl = checklist(docs, (h) => h !== "hz", (h) => h).map((c) => c.text);
    expect(cl).toEqual(expect.arrayContaining(["2 agents on 2 hosts", "1 connection", "hz is not connected"]));
  });
});

describe("workstream screens", () => {
  it("list shows every workstream with status in words; opening one shows the graph and the pending handoff", async () => {
    const user = userEvent.setup();
    const r = await renderApp("/inbox", quietBackend());
    act(() => r.history.push("/workstreams"));
    const list = await screen.findByRole("list", { name: "Workstreams" });
    const row = within(list).getByRole("link", { name: /Invoice export v2/ });
    expect(within(row).getByText("2 need you")).toBeInTheDocument();
    expect(within(list).getByRole("link", { name: /Docs search revamp/ })).toHaveTextContent("Draft, not launched");
    await user.click(row);
    expect(await screen.findByRole("heading", { name: "Invoice export v2" })).toBeInTheDocument();
    await screen.findByTestId("graph-canvas", {}, { timeout: 4000 });
    expect(screen.getAllByText("Handoff waiting").length).toBeGreaterThan(0);
    const gate = await screen.findByTestId("edge-gate");
    await user.click(within(gate).getByRole("button", { name: /Allow once/ }));
    await waitFor(() => expect(r.backend.requests.find((x) => x.method === "session.command")?.params).toMatchObject({ command: { command: "answer_gate", request_id: "wasl-handoff-t4print", decision: { choice: "allow_once" } } }));
  });

  it("an edge's policy is set from the inspector", async () => {
    const user = userEvent.setup();
    const r = await renderApp("/inbox", quietBackend());
    act(() => r.history.push("/workstreams/ws-invoice"));
    await screen.findByTestId("graph-canvas", {}, { timeout: 4000 });
    await user.click(screen.getByRole("button", { name: /Connection invoice-export-orchestrator to csv-schema-worker/ }));
    await user.click(within(screen.getByRole("radiogroup", { name: "Policy for this connection" })).getByRole("radio", { name: /Blocked/ }));
    await waitFor(() => expect(r.backend.requests.find((x) => x.method === "workstreams.set_policy")?.params).toEqual({ workstream: "ws-invoice", from: "orchestrator", to: "csv", policy: "deny" }));
  });

  it("design mode: drag from a node's port to another node connects them; the keyboard path does too", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push("/workstreams/ws-docs"));
    await screen.findByTestId("graph-canvas", {}, { timeout: 4000 });
    expect(screen.getByText("Before you launch")).toBeInTheDocument();
    // remove the existing edge first so the drag creates a new one
    await act(async () => { await backend.request("workstreams.disconnect", { workstream: "ws-docs", from: "lead", to: "index" }); });
    await waitFor(() => expect(document.querySelectorAll("[data-edge]").length).toBe(0));
    const port = screen.getByRole("button", { name: "Connect docs-search-index to another agent" });
    const target = document.querySelector('[data-node="lead"]') as HTMLElement;
    const orig = document.elementFromPoint;
    document.elementFromPoint = () => target;
    fireEvent.pointerDown(port, { button: 0, clientX: 10, clientY: 10 });
    await act(async () => { window.dispatchEvent(new PointerEvent("pointermove", { clientX: 40, clientY: 200 })); });
    expect(screen.getByText(/Drawing a connection/)).toBeInTheDocument();
    await act(async () => { window.dispatchEvent(new PointerEvent("pointerup", { clientX: 40, clientY: 200 })); });
    document.elementFromPoint = orig;
    await waitFor(() => expect(backend.requests.find((x) => x.method === "workstreams.connect")?.params).toEqual({ workstream: "ws-docs", from: "index", to: "lead", policy: "ask" }));
    // keyboard: select the lead node and connect from the inspector
    await user.click(screen.getByRole("button", { name: /^search-lead/ }));
    const insp = screen.getByRole("complementary", { name: "Agent" });
    await user.click(within(insp).getByRole("button", { name: /Connect to/ }));
    await user.click(screen.getByRole("option", { name: "docs-search-index" }));
    await user.click(within(insp).getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(backend.requests.filter((x) => x.method === "workstreams.connect").length).toBe(2));
  });

  it("new workstream from existing sessions creates a draft with those sessions as nodes", async () => {
    const user = userEvent.setup();
    const r = await renderApp("/inbox", quietBackend());
    await user.keyboard("{Shift>}n{/Shift}");
    const dlg = await screen.findByRole("dialog", { name: "New workstream" });
    await user.type(within(dlg).getByLabelText("Name"), "Release prep");
    await user.click(within(dlg).getByRole("radio", { name: /From existing sessions/ }));
    await user.click(within(dlg).getByRole("checkbox", { name: /release-notes-1-4/ }));
    await user.click(within(dlg).getByRole("button", { name: /Continue to design/ }));
    await waitFor(() => expect(r.backend.requests.find((x) => x.method === "workstreams.create")).toBeDefined());
    const p = r.backend.requests.find((x) => x.method === "workstreams.create")!.params as { name: string; nodes: { name: string; session: unknown }[] };
    expect(p.name).toBe("Release prep");
    expect(p.nodes).toEqual([expect.objectContaining({ name: "release-notes-1-4", session: expect.objectContaining({ host: "x1" }) })]);
    await waitFor(() => expect(r.history.store.get()).toMatch(/^\/workstreams\/ws-release-prep/));
  });
});
