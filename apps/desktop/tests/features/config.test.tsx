import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { rowAction, summarize, toggled } from "../../src/features/config/layers";
import { provenance } from "../../src/features/config/provenance";
import { mergeEffective } from "../../src/bridge/fixture/config";
import { driverDef } from "../../src/bridge/fixture/catalog";
import type { EffectiveItem } from "../../src/bridge/wire";
import { quietBackend, renderApp, sessionByName } from "../helpers";

const empty = { items: [], env: [], disallowed_tools: [] };

describe("config: override layers", () => {
  it("toggling an item writes the selected layer: disable, undo, or reset a broader layer's change", () => {
    const g = { scope: "global" as const };
    const h = { scope: "host" as const, host: "x1" };
    const active = { kind: "mcp_server", name: "github", active: true } as EffectiveItem;
    expect(toggled(empty, active, h).items).toEqual([{ kind: "mcp_server", name: "github", action: "disable" }]);
    const offHere = { ...active, active: false, changed_by: { from: "layer", scope: h } } as EffectiveItem;
    expect(toggled({ ...empty, items: [{ kind: "mcp_server", name: "github", action: "disable" }] }, offHere, h).items).toEqual([]);
    const offGlobal = { ...active, active: false, changed_by: { from: "layer", scope: g } } as EffectiveItem;
    expect(toggled(empty, offGlobal, h).items).toEqual([{ kind: "mcp_server", name: "github", action: "reset" }]);
  });

  it("an item a layer added offers remove (no reset badge); one it changed or switched off offers reset", () => {
    const g = { scope: "global" as const };
    const h = { scope: "host" as const, host: "x1" };
    const added = { kind: "mcp_server", name: "docs", active: true, state: "added", origin: { from: "layer", scope: g }, changed_by: { from: "layer", scope: g } } as EffectiveItem;
    expect(provenance(added, "Claude Code").map((b) => b.text)).toEqual(["added at Global"]);
    expect(rowAction(added, g)).toBe("remove");
    expect(rowAction(added, h)).toBeNull();
    const changed = { ...added, state: "changed", origin: { from: "agent", source: { scope: "user" } } } as EffectiveItem;
    expect(rowAction(changed, g)).toBe("reset");
    const off = { ...changed, state: "disabled", active: false } as EffectiveItem;
    expect(rowAction(off, g)).toBe("reset");
    expect(rowAction(off, h)).toBeNull();
    expect(provenance(off, "Claude Code").map((b) => b.text)).toEqual(["from Claude Code config", "turned off at Global"]);
  });

  it("layer summary marks added, changed and disabled items", () => {
    const lines = summarize({ ...empty, items: [{ kind: "mcp_server", name: "sentry", action: "disable" }, { kind: "skill", name: "pdf", action: "set", value: { kind: "skill", root: "~/s" } }] }, driverDef("claude").config);
    expect(lines.map((l) => l.op + l.text)).toEqual(["−sentry turned off", "+pdf skill"]);
  });

  it("the merge shows provenance and Marsad's automatic Wasl item only in a workstream", () => {
    const base = driverDef("claude").config;
    const layers = [{ scope: { scope: "host" as const, host: "x1" }, layer: { ...empty, items: [{ kind: "mcp_server" as const, name: "sentry", action: "disable" as const }] } }];
    const solo = mergeEffective({ driver: "claude", base, layers, workstream: false });
    expect(solo.items.find((i) => i.name === "sentry")).toMatchObject({ state: "disabled", active: false, changed_by: { from: "layer" } });
    expect(solo.items.some((i) => i.name === "wasl")).toBe(false);
    const ws = mergeEffective({ driver: "claude", base, layers, workstream: true });
    expect(ws.items.find((i) => i.name === "wasl")).toMatchObject({ state: "automatic", locked: true });
    expect(mergeEffective({ driver: "kilo", base: driverDef("kilo").config, layers: [{ scope: { scope: "global" }, layer: { ...empty, items: [{ kind: "plugin", name: "p", action: "set", value: { kind: "plugin", path: "/p" } }] } }], workstream: false }).items.find((i) => i.name === "p")?.unsupported).toMatch(/plugin/);
  });
});

describe("config page", () => {
  it("mirrors the agent's base, shows effective items with provenance, and a switch edits the selected layer", async () => {
    const user = userEvent.setup();
    const backend = quietBackend();
    const s = sessionByName(backend, "migrate-auth-middleware");
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push(`/config?driver=claude&host=x1&session=${s.id}`));
    expect(await screen.findByText("turned off at Host")).toBeInTheDocument();
    expect(screen.getByText("read-only")).toBeInTheDocument();
    expect((await screen.findAllByText("~/.claude.json")).length).toBeGreaterThan(0);
    await user.click(within(screen.getByRole("complementary", { name: "Configuration layers" })).getByRole("button", { name: /^Host/ }));
    const row = document.querySelector('[data-item="mcp_server:github"]') as HTMLElement;
    await user.click(within(row).getByRole("switch"));
    await waitFor(() => expect(backend.requests.find((x) => x.method === "store.layers.set")?.params).toMatchObject({ scope: { scope: "host", host: "x1" } }));
    const sent = backend.requests.find((x) => x.method === "store.layers.set")!.params as { layer: { items: { name: string; action: string }[] } };
    expect(sent.layer.items).toEqual(expect.arrayContaining([{ kind: "mcp_server", name: "github", action: "disable" }, expect.objectContaining({ name: "sentry", action: "disable" })]));
    await waitFor(() => expect(within(document.querySelector('[data-item="mcp_server:github"]') as HTMLElement).getByRole("switch")).toHaveAttribute("aria-checked", "false"));
  });

  it("a workstream session gets the locked Wasl connector", async () => {
    const backend = quietBackend();
    const s = sessionByName(backend, "invoice-export-orchestrator");
    const r = await renderApp("/inbox", backend);
    act(() => r.history.push(`/config?driver=claude&host=x1&session=${s.id}`));
    const wasl = await waitFor(() => { const el = document.querySelector('[data-item="mcp_server:wasl"]'); if (!el) throw new Error("no wasl"); return el as HTMLElement; });
    expect(within(wasl).getByText("automatic · Wasl")).toBeInTheDocument();
    expect(within(wasl).queryByRole("switch")).toBeNull();
    expect((await screen.findAllByText(/playwright/)).length).toBeGreaterThan(0);
  });
});
