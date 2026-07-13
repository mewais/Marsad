import { describe, it, expect } from "vitest";
import {
  connHintForStatus,
  connHintForHost,
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
