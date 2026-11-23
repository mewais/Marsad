import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { summary } from "../../src/features/workstreams/model";
import { claudeChoices } from "../../src/bridge/fixture/sessions";
import { renderApp, quietBackend, sessionByName } from "../helpers";
import type { WorkstreamView } from "../../src/bridge/wire";

const row = (name: string) => screen.getByText(name, { selector: "button.nm" }).closest("[data-session]") as HTMLElement;
const badge = () => within(screen.getByRole("navigation", { name: "Main" })).queryByLabelText(/need you/);

describe("sessions of a host that has not delivered its list over the current link", () => {
  it("are marked last known everywhere they are listed; live hosts' are not", async () => {
    await renderApp("/inbox");
    // win-build is reconnecting in the fixture: its list is not live.
    for (const n of ["windows-print-check", "msi-installer", "ci-repro-windows"]) {
      const r = row(n);
      expect(r).toHaveAttribute("data-stale");
      expect(within(r).getByText("last known")).toBeInTheDocument();
      expect(r.querySelector(".st")).toHaveClass("stale");
      expect(r.querySelector(".st svg.spin")).toBeNull();
    }
    const live = row("migrate-auth-middleware");
    expect(live).not.toHaveAttribute("data-stale");
    expect(live.querySelector(".st")).not.toHaveClass("stale");
    // The host's own link says so too, in words.
    const sb = screen.getByRole("contentinfo", { name: "Status" });
    expect(sb).toHaveTextContent(/win-build · reconnecting/);
  });

  it("do not count as live activity: not in the needs-you badge, not in the summary's live counts", async () => {
    const backend = quietBackend();
    const r = await renderApp("/inbox", backend);
    void r;
    const before = Number(badge()?.textContent ?? 0);
    expect(before).toBeGreaterThan(0);
    const s = sessionByName(backend, "windows-print-check");
    act(() => backend.raiseGate(s.host, s.id, { gate_type: "plan", request_id: "pl-stale", summary: "Print", choices: claudeChoices() }));
    // The session now needs the human, but the host is not reporting: it shows where it was and is not counted.
    await waitFor(() => expect(row("windows-print-check")).toHaveAttribute("data-status", "needs"));
    expect(row("windows-print-check")).toHaveAttribute("data-stale");
    expect(Number(badge()?.textContent ?? 0)).toBe(before);
    expect(screen.getByTestId("summary-stale")).toBeInTheDocument();
    // Its gate cannot be answered: the choices are off and say why.
    for (const b of within(row("windows-print-check")).queryAllByRole("button", { name: /Allow|Deny/ })) expect(b).toBeDisabled();
    // Once the host connects and reports, it is live and counted.
    act(() => backend.setLink("win", { state: "connected", hello: { protocol: { major: 1, minor: 0 }, build: "fixture", host_id: "win", role: "daemon", features: [] } }));
    await waitFor(() => expect(row("windows-print-check")).not.toHaveAttribute("data-stale"));
    expect(Number(badge()?.textContent ?? 0)).toBe(before + 1);
  });

  it("a live host that drops becomes last known at once (the hub clears sessions_live with the link)", async () => {
    const backend = quietBackend();
    await renderApp("/inbox", backend);
    expect(row("pdf-renderer-worker")).not.toHaveAttribute("data-stale");
    act(() => backend.setLink("hz", { state: "reconnecting", attempt: 1, next_at: { secs_since_epoch: 0, nanos_since_epoch: 0 }, last_error: "timed out" }));
    await waitFor(() => expect(row("pdf-renderer-worker")).toHaveAttribute("data-stale"));
    expect(row("pdf-renderer-worker").querySelector(".st svg.spin")).toBeNull();
    // A host that is connected again but has not yet sent its list is still last known.
    act(() => backend.setLink("hz", { state: "connecting" }));
    expect(row("pdf-renderer-worker")).toHaveAttribute("data-stale");
    // Local sessions were never affected.
    expect(row("migrate-auth-middleware")).not.toHaveAttribute("data-stale");
  });

  it("the session page says so, and nothing in it can act on a session the host is not reporting", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, "windows-print-check");
    await renderApp(`/session/${s.host}/${s.id}`, backend);
    await screen.findByRole("heading", { name: "windows-print-check" });
    const note = await screen.findByTestId("stale-note");
    expect(note).toHaveTextContent(/Last known state/);
    expect(note).toHaveTextContent(/win-build is not connected \(reconnecting\)/);
    expect(screen.getByRole("button", { name: /Interrupt/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Delete|End session/ })).toBeDisabled();
    expect(await screen.findByTestId("composer-blocked")).toBeInTheDocument();
    expect(screen.getByText("last known")).toBeInTheDocument();
  });

  it("workstream summaries count a stale node apart instead of as working or needing you", () => {
    const node = (host: string, state: string) => ({ host, session: { phase: { state }, pending_gates: [], usage: { cost_usd: 0 } }, launching: false }) as unknown as WorkstreamView["nodes"][number];
    const v = { workstream: { state: "active", nodes: [] }, nodes: [node("a", "working"), node("b", "working"), node("a", "needs_you")], edges: [], cost: { sessions_usd: 0 }, pending_spawns: [] } as unknown as WorkstreamView;
    expect(summary(v).parts.map((p) => p.text)).toEqual(["1 needs you", "2 working"]);
    const s = summary(v, (h) => h === "b");
    expect(s.parts.map((p) => p.text)).toEqual(["1 needs you", "1 working", "1 last known"]);
    expect(summary(v, () => true).parts.map((p) => p.text)).toEqual(["3 last known"]);
  });
});
