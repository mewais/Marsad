import { describe, it, expect } from "vitest";
import {
  connHintForStatus,
  connHintForHost,
  connHintForConnHealth,
  connHintForSession,
  tallyConnHints,
} from "../src/bridge/connectivity";

describe("connectivity hint (host-derived, not lifecycle)", () => {
  it("maps host status → hint, case-insensitively", () => {
    expect(connHintForStatus("Connected")).toBeNull();
    expect(connHintForStatus("connected")).toBeNull();
    expect(connHintForStatus("Reconnecting")).toBe("Reconnecting");
    expect(connHintForStatus("Connecting")).toBe("Reconnecting"); // (re)establishing
    expect(connHintForStatus("reconnecting")).toBe("Reconnecting");
    expect(connHintForStatus("Unreachable")).toBe("Disconnected");
    expect(connHintForStatus("unreachable")).toBe("Disconnected");
    expect(connHintForStatus(undefined)).toBeNull();
    expect(connHintForStatus("")).toBeNull();
  });

  it("looks up a session's host status by id", () => {
    const hosts = [
      { id: "local", status: "Connected" },
      { id: "etx", status: "Unreachable" },
    ];
    expect(connHintForHost(hosts, "local")).toBeNull();
    expect(connHintForHost(hosts, "etx")).toBe("Disconnected");
    expect(connHintForHost(hosts, "unknown")).toBeNull(); // host not found → no hint
    expect(connHintForHost(hosts, null)).toBeNull();
  });

  it("maps per-session conn_health → hint", () => {
    expect(connHintForConnHealth("Connected")).toBeNull();
    expect(connHintForConnHealth("Reconnecting")).toBe("Reconnecting");
    expect(connHintForConnHealth("Unstable")).toBe("Unstable");
    expect(connHintForConnHealth("Disconnected")).toBe("Disconnected");
    expect(connHintForConnHealth(undefined)).toBeNull();
  });

  it("prefers the per-session conn_health over the host poll", () => {
    const hosts = [{ id: "etx", status: "Connected" }];
    // Host reads Connected (15s poll) but the session's stream dropped — the
    // per-session signal wins so the outage is shown immediately.
    expect(connHintForSession("Reconnecting", hosts, "etx")).toBe("Reconnecting");
    expect(connHintForSession("Unstable", hosts, "etx")).toBe("Unstable");
    // Session Connected → fall back to host status (host unreachable still shows).
    const down = [{ id: "etx", status: "Unreachable" }];
    expect(connHintForSession("Connected", down, "etx")).toBe("Disconnected");
    // Both clean → no hint.
    expect(connHintForSession("Connected", hosts, "etx")).toBeNull();
    // Missing conn_health falls back to host.
    expect(connHintForSession(undefined, down, "etx")).toBe("Disconnected");
  });

  it("tallies reconnecting / disconnected across statuses", () => {
    const t = tallyConnHints([
      "Connected",
      "Reconnecting",
      "Unreachable",
      "connecting",
      "Unreachable",
      undefined,
    ]);
    expect(t.reconnecting).toBe(2); // Reconnecting + connecting
    expect(t.disconnected).toBe(2); // two Unreachable
  });
});
