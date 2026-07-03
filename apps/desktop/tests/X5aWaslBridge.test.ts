/**
 * X5a — Wasl bridge/state layer: VM_UPDATE reducer arms for the Wasl gate list,
 * gate decisions, and the observation feed; the answerWaslGate relay wrapper;
 * and the optionality invariant (no Wasl data ⇒ empty slices, no chrome).
 */
import { describe, it, expect } from "vitest";
import { vmReducer, initialVmState } from "../src/bridge/useVm";
import type { VmState } from "../src/bridge/useVm";
import type { AppSnapshot, WaslGateView, WaslAuditEntry } from "../src/bridge/types";

const emptySnapshot: AppSnapshot = {
  sessions: [],
  hosts: [],
  workspaces: [],
  workstreams: [],
  selected_id: null,
} as unknown as AppSnapshot;

function withSnapshot(): VmState {
  return { ...initialVmState, snapshot: emptySnapshot };
}

const gate = (gate_id: string, host_id = "hostA"): WaslGateView => ({
  host_id,
  gate_id,
  kind: "consult",
  from: "alpha",
  summary: "consult beta",
  preview: "what is 2+2?",
  requested_at: "2026-07-03T00:00:00Z",
  deadline: "2026-07-03T00:02:00Z",
});

describe("X5a — Wasl VM_UPDATE reducer arms", () => {
  it("wasl_gates replaces the pending gate list wholesale", () => {
    let s = withSnapshot();
    s = vmReducer(s, {
      type: "VM_UPDATE",
      payload: { kind: "wasl_gates", gates: [gate("g1"), gate("g2")] },
    });
    expect(s.waslGates.map((g) => g.gate_id)).toEqual(["g1", "g2"]);
    // A later full list replaces (not merges).
    s = vmReducer(s, {
      type: "VM_UPDATE",
      payload: { kind: "wasl_gates", gates: [gate("g3")] },
    });
    expect(s.waslGates.map((g) => g.gate_id)).toEqual(["g3"]);
  });

  it("wasl_gate_decided drops the resolved gate from the pending list", () => {
    let s = withSnapshot();
    s = vmReducer(s, {
      type: "VM_UPDATE",
      payload: { kind: "wasl_gates", gates: [gate("g1"), gate("g2")] },
    });
    s = vmReducer(s, {
      type: "VM_UPDATE",
      payload: {
        kind: "wasl_gate_decided",
        host_id: "hostA",
        gate_id: "g1",
        outcome: "approved",
        decided_by: "operator",
        reason: null,
      },
    });
    expect(s.waslGates.map((g) => g.gate_id)).toEqual(["g2"]);
  });

  it("wasl_observation appends to the feed and caps at 200", () => {
    let s = withSnapshot();
    for (let i = 0; i < 205; i++) {
      const entry: WaslAuditEntry = {
        seq: i,
        kind: "message",
        actor: "alpha",
        targets: ["beta"],
        summary: `msg ${i}`,
      };
      s = vmReducer(s, {
        type: "VM_UPDATE",
        payload: { kind: "wasl_observation", host_id: "hostA", entry },
      });
    }
    expect(s.waslObservations).toHaveLength(200);
    expect(s.waslObservations[0].seq).toBe(5); // oldest 5 dropped
    expect(s.waslObservations[s.waslObservations.length - 1].seq).toBe(204);
  });
});

describe("X5a — optionality (no Wasl ⇒ inert)", () => {
  it("initial state has empty Wasl slices", () => {
    expect(initialVmState.waslGates).toEqual([]);
    expect(initialVmState.waslObservations).toEqual([]);
  });

  it("unrelated VM_UPDATE events leave the Wasl slices untouched", () => {
    let s = withSnapshot();
    s = vmReducer(s, {
      type: "VM_UPDATE",
      payload: { kind: "overview", groups: {} as never },
    });
    expect(s.waslGates).toEqual([]);
    expect(s.waslObservations).toEqual([]);
  });
});

describe("X5a — answerWaslGate relay wrapper", () => {
  it("is a no-op outside Tauri (mock mode) — never throws, relays nothing", async () => {
    const { answerWaslGate } = await import("../src/bridge/api");
    await expect(
      answerWaslGate({ host_id: "hostA", gate_id: "g1", outcome: "approved" })
    ).resolves.toBeUndefined();
  });
});
