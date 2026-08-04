/**
 * WaslCodexDriver — unit tests for wasl-codex additions to drivers.ts.
 *
 * Section A of the capability-alias spec. Pure module tests, no React.
 *
 * Spec:
 *   - DRIVERS gains { id: "wasl-codex", label: "Wasl · Codex" } after wasl-opencode.
 *   - DRIVER_COLOR gains a non-empty, distinct colour for "wasl-codex".
 *   - waslProtocol("wasl-codex") → "codex".
 *   - isWaslDriver("wasl-codex") → true.
 *   - Driver union in workstreamTypes.ts includes "wasl-codex" (runtime proxy via DRIVERS array).
 */
import { describe, it, expect } from "vitest";
import * as driversModule from "../src/bridge/drivers";

const { DRIVERS, DRIVER_COLOR } = driversModule as {
  DRIVERS: { id: string; label: string }[];
  DRIVER_COLOR: Record<string, string>;
};

const waslProtocol = (driversModule as Record<string, unknown>).waslProtocol as
  | ((d: string) => string | null)
  | undefined;

const isWaslDriver = (driversModule as Record<string, unknown>).isWaslDriver as
  | ((d: string) => boolean)
  | undefined;

// ---------------------------------------------------------------------------
// waslProtocol — wasl-codex and regressions
// ---------------------------------------------------------------------------

describe("waslProtocol() — wasl-codex", () => {
  it("is exported from drivers.ts", () => {
    expect(typeof waslProtocol).toBe("function");
  });

  it("wasl-codex → 'codex'", () => {
    expect(waslProtocol!("wasl-codex")).toBe("codex");
  });

  it("wasl-claude → 'claude' (regression)", () => {
    expect(waslProtocol!("wasl-claude")).toBe("claude");
  });

  it("wasl-opencode → 'opencode' (regression)", () => {
    expect(waslProtocol!("wasl-opencode")).toBe("opencode");
  });

  it("claude → null (non-wasl driver)", () => {
    expect(waslProtocol!("claude")).toBeNull();
  });

  it("codex → null (non-wasl driver)", () => {
    expect(waslProtocol!("codex")).toBeNull();
  });

  it("opencode → null (non-wasl driver)", () => {
    expect(waslProtocol!("opencode")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// isWaslDriver — wasl-codex
// ---------------------------------------------------------------------------

describe("isWaslDriver() — wasl-codex", () => {
  it("returns true for 'wasl-codex'", () => {
    expect(isWaslDriver!("wasl-codex")).toBe(true);
  });

  it("returns false for 'codex'", () => {
    expect(isWaslDriver!("codex")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// DRIVERS array — wasl-codex entry
// ---------------------------------------------------------------------------

describe("DRIVERS — wasl-codex entry", () => {
  it("DRIVERS contains an entry with id 'wasl-codex'", () => {
    const entry = DRIVERS.find((d) => d.id === "wasl-codex");
    expect(entry).toBeDefined();
  });

  it("wasl-codex entry label is 'Wasl · Codex'", () => {
    const entry = DRIVERS.find((d) => d.id === "wasl-codex");
    expect(entry?.label).toBe("Wasl · Codex");
  });

  it("wasl-codex entry appears after wasl-opencode (ordering)", () => {
    const ids = DRIVERS.map((d) => d.id);
    const ocIdx = ids.indexOf("wasl-opencode");
    const cdxIdx = ids.indexOf("wasl-codex");
    expect(ocIdx).toBeGreaterThanOrEqual(0);
    expect(cdxIdx).toBeGreaterThan(ocIdx);
  });
});

// ---------------------------------------------------------------------------
// DRIVER_COLOR — wasl-codex entry
// ---------------------------------------------------------------------------

describe("DRIVER_COLOR — wasl-codex entry", () => {
  it("DRIVER_COLOR has a non-empty colour string for 'wasl-codex'", () => {
    const color = DRIVER_COLOR["wasl-codex"];
    expect(typeof color).toBe("string");
    expect(color.length).toBeGreaterThan(0);
  });

  it("wasl-codex colour is distinct from wasl-opencode", () => {
    expect(DRIVER_COLOR["wasl-codex"]).not.toBe(DRIVER_COLOR["wasl-opencode"]);
  });

  it("wasl-codex colour is distinct from wasl-claude", () => {
    expect(DRIVER_COLOR["wasl-codex"]).not.toBe(DRIVER_COLOR["wasl-claude"]);
  });

  it("existing driver colours are still present (regression)", () => {
    for (const id of ["claude", "codex", "gemini", "opencode", "wasl-claude", "wasl-opencode"]) {
      expect(typeof DRIVER_COLOR[id]).toBe("string");
      expect(DRIVER_COLOR[id].length).toBeGreaterThan(0);
    }
  });
});

// ---------------------------------------------------------------------------
// workstreamTypes.ts — Driver union includes wasl-codex (runtime proxy)
// ---------------------------------------------------------------------------

describe("workstreamTypes.ts — Driver union includes wasl-codex (runtime proxy)", () => {
  it("DRIVERS array accepts 'wasl-codex' as an id value (union extended)", () => {
    const ids = DRIVERS.map((d) => d.id);
    expect(ids).toContain("wasl-codex");
  });
});
