/**
 * waslDrivers — the Wasl driver *vocabulary* in drivers.ts.
 *
 * Post-#8: a user never PICKS a wasl driver — the picker (DRIVERS) is NATIVE
 * only; whether an agent runs over Wasl is a workstream-TYPE decision. But the
 * classification helpers + colours remain valid vocabulary (used to coerce a
 * legacy wasl-* setting back to its native protocol, and to colour any residual
 * value). This suite pins that contract:
 *   - DRIVERS is native-only (no "wasl-*" entries).
 *   - isWaslDriver(d): true iff d starts with "wasl-".
 *   - waslProtocol(d): "wasl-claude"→"claude", "wasl-opencode"→"opencode",
 *     "wasl-codex"→"codex", anything native → null.
 *   - DRIVER_COLOR still carries entries for the wasl-* ids (residual mapping).
 */
import { describe, it, expect } from "vitest";
import * as driversModule from "../src/bridge/drivers";

const { DRIVERS, DRIVER_COLOR } = driversModule as {
  DRIVERS: { id: string; label: string }[];
  DRIVER_COLOR: Record<string, string>;
};

const isWaslDriver = (driversModule as Record<string, unknown>).isWaslDriver as (d: string) => boolean;
const waslProtocol = (driversModule as Record<string, unknown>).waslProtocol as (
  d: string,
) => "claude" | "opencode" | "codex" | null;

// ---------------------------------------------------------------------------
// DRIVERS — native only (the picker never offers a wasl-* driver, #8)
// ---------------------------------------------------------------------------

describe("DRIVERS — native only", () => {
  it("lists exactly the native drivers, in order", () => {
    expect(DRIVERS.map((d) => d.id)).toEqual(["claude", "codex", "opencode"]);
  });

  it("contains NO wasl-* entry", () => {
    expect(DRIVERS.some((d) => d.id.startsWith("wasl-"))).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// DRIVER_COLOR — native + residual wasl-* mapping
// ---------------------------------------------------------------------------

describe("DRIVER_COLOR", () => {
  it("has colours for the three native drivers", () => {
    for (const id of ["claude", "codex", "opencode"]) {
      expect(typeof DRIVER_COLOR[id]).toBe("string");
      expect(DRIVER_COLOR[id].length).toBeGreaterThan(0);
    }
  });

  it("still carries colours for residual wasl-* ids", () => {
    for (const id of ["wasl-claude", "wasl-opencode", "wasl-codex"]) {
      expect(typeof DRIVER_COLOR[id]).toBe("string");
      expect(DRIVER_COLOR[id].length).toBeGreaterThan(0);
    }
  });
});

// ---------------------------------------------------------------------------
// isWaslDriver — classification helper
// ---------------------------------------------------------------------------

describe("isWaslDriver()", () => {
  it("returns true for wasl-* ids", () => {
    expect(isWaslDriver("wasl-claude")).toBe(true);
    expect(isWaslDriver("wasl-opencode")).toBe(true);
    expect(isWaslDriver("wasl-codex")).toBe(true);
  });

  it("returns false for native ids", () => {
    for (const id of ["claude", "codex", "opencode"]) {
      expect(isWaslDriver(id)).toBe(false);
    }
  });
});

// ---------------------------------------------------------------------------
// waslProtocol — maps a wasl driver id → its native protocol (or null)
// ---------------------------------------------------------------------------

describe("waslProtocol()", () => {
  it("maps each wasl-* id to its native protocol", () => {
    expect(waslProtocol("wasl-claude")).toBe("claude");
    expect(waslProtocol("wasl-opencode")).toBe("opencode");
    expect(waslProtocol("wasl-codex")).toBe("codex");
  });

  it("returns null for native ids", () => {
    for (const id of ["claude", "codex", "opencode"]) {
      expect(waslProtocol(id)).toBeNull();
    }
  });
});
