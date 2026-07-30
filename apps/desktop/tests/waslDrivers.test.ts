/**
 * waslDrivers — RED-phase tests for the Wasl driver registry additions.
 *
 * Spec:
 *   - `src/bridge/drivers.ts`:
 *       DRIVERS gains { id:"wasl-claude", label:"Wasl · Claude" } and
 *                     { id:"wasl-opencode", label:"Wasl · OpenCode" }
 *       DRIVER_COLOR gains entries for both ids
 *       isWaslDriver(d): boolean  — true iff d starts with "wasl-"
 *       waslProtocol(d): "claude"|"opencode"|null
 *           "wasl-claude"    → "claude"
 *           "wasl-opencode"  → "opencode"
 *           anything else    → null
 *   - `src/bridge/workstreamTypes.ts`:
 *       Driver union adds "wasl-claude" | "wasl-opencode"
 *
 * All tests below are RED until a coder adds the exports.  They must NOT fail
 * with runtime errors that are unrelated to missing exports (imports use
 * ?. / 'in' / type-level checks to stay safe while the code is absent).
 */
import { describe, it, expect } from "vitest";

// ---------------------------------------------------------------------------
// Import what exists — new exports will not be present yet (RED).
// Using a dynamic import-star so the test file itself never throws at module
// load time even when named exports are absent.
// ---------------------------------------------------------------------------

import * as driversModule from "../src/bridge/drivers";

const { DRIVERS, DRIVER_COLOR } = driversModule as {
  DRIVERS: { id: string; label: string }[];
  DRIVER_COLOR: Record<string, string>;
};

// Helpers that will error with "not a function" when absent — that is the
// expected RED failure for those tests.
const isWaslDriver: ((d: string) => boolean) | undefined =
  (driversModule as Record<string, unknown>).isWaslDriver as
  | ((d: string) => boolean)
  | undefined;

const waslProtocol: ((d: string) => "claude" | "opencode" | null) | undefined =
  (driversModule as Record<string, unknown>).waslProtocol as
  | ((d: string) => "claude" | "opencode" | null)
  | undefined;

// ---------------------------------------------------------------------------
// DRIVERS array — wasl-claude entry
// ---------------------------------------------------------------------------

describe("DRIVERS — wasl-claude entry", () => {
  it("DRIVERS contains an entry with id 'wasl-claude'", () => {
    // RED: no such entry exists yet
    const entry = DRIVERS.find((d) => d.id === "wasl-claude");
    expect(entry).toBeDefined();
  });

  it("wasl-claude entry label is exactly 'Wasl · Claude'", () => {
    // RED: entry absent → entry.label throws
    const entry = DRIVERS.find((d) => d.id === "wasl-claude");
    expect(entry?.label).toBe("Wasl · Claude");
  });
});

// ---------------------------------------------------------------------------
// DRIVERS array — wasl-opencode entry
// ---------------------------------------------------------------------------

describe("DRIVERS — wasl-opencode entry", () => {
  it("DRIVERS contains an entry with id 'wasl-opencode'", () => {
    // RED: no such entry exists yet
    const entry = DRIVERS.find((d) => d.id === "wasl-opencode");
    expect(entry).toBeDefined();
  });

  it("wasl-opencode entry label is exactly 'Wasl · OpenCode'", () => {
    const entry = DRIVERS.find((d) => d.id === "wasl-opencode");
    expect(entry?.label).toBe("Wasl · OpenCode");
  });
});

// ---------------------------------------------------------------------------
// DRIVER_COLOR — both wasl entries must have colour strings
// ---------------------------------------------------------------------------

describe("DRIVER_COLOR — wasl entries", () => {
  it("DRIVER_COLOR has a non-empty colour string for 'wasl-claude'", () => {
    // RED: key absent → value is undefined
    const color = DRIVER_COLOR["wasl-claude"];
    expect(typeof color).toBe("string");
    expect(color.length).toBeGreaterThan(0);
  });

  it("DRIVER_COLOR has a non-empty colour string for 'wasl-opencode'", () => {
    const color = DRIVER_COLOR["wasl-opencode"];
    expect(typeof color).toBe("string");
    expect(color.length).toBeGreaterThan(0);
  });

  it("DRIVER_COLOR still has colours for the original four drivers", () => {
    // Regression: adding new entries must not remove existing ones
    expect(typeof DRIVER_COLOR["claude"]).toBe("string");
    expect(typeof DRIVER_COLOR["codex"]).toBe("string");
    expect(typeof DRIVER_COLOR["gemini"]).toBe("string");
    expect(typeof DRIVER_COLOR["opencode"]).toBe("string");
  });
});

// ---------------------------------------------------------------------------
// isWaslDriver — classification helper
// ---------------------------------------------------------------------------

describe("isWaslDriver()", () => {
  it("is exported from drivers.ts", () => {
    // RED: function absent
    expect(typeof isWaslDriver).toBe("function");
  });

  it("returns true for 'wasl-claude'", () => {
    expect(isWaslDriver!("wasl-claude")).toBe(true);
  });

  it("returns true for 'wasl-opencode'", () => {
    expect(isWaslDriver!("wasl-opencode")).toBe(true);
  });

  it("returns false for 'claude'", () => {
    expect(isWaslDriver!("claude")).toBe(false);
  });

  it("returns false for 'codex'", () => {
    expect(isWaslDriver!("codex")).toBe(false);
  });

  it("returns false for 'gemini'", () => {
    expect(isWaslDriver!("gemini")).toBe(false);
  });

  it("returns false for 'opencode'", () => {
    expect(isWaslDriver!("opencode")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// waslProtocol — maps wasl driver id → protocol string or null
// ---------------------------------------------------------------------------

describe("waslProtocol()", () => {
  it("is exported from drivers.ts", () => {
    // RED: function absent
    expect(typeof waslProtocol).toBe("function");
  });

  it("returns 'claude' for 'wasl-claude'", () => {
    expect(waslProtocol!("wasl-claude")).toBe("claude");
  });

  it("returns 'opencode' for 'wasl-opencode'", () => {
    expect(waslProtocol!("wasl-opencode")).toBe("opencode");
  });

  it("returns null for 'claude'", () => {
    expect(waslProtocol!("claude")).toBeNull();
  });

  it("returns null for 'codex'", () => {
    expect(waslProtocol!("codex")).toBeNull();
  });

  it("returns null for 'gemini'", () => {
    expect(waslProtocol!("gemini")).toBeNull();
  });

  it("returns null for 'opencode'", () => {
    expect(waslProtocol!("opencode")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// workstreamTypes.ts — Driver union must include wasl variants
// ---------------------------------------------------------------------------
//
// We cannot import a union type at runtime, but we can exercise the DRIVERS
// array as a proxy: the registry is typed `{ id: Driver; label: string }[]`,
// so if "wasl-claude" compiles as a valid id it must be in the union.
// At runtime we just check the array contains the ids (already done above).
// The *type-level* RED failure will manifest when `Driver` is narrowed in
// the production sources before the coder extends it.
//
// Add a simple runtime check that no runtime object rejects the string:

describe("workstreamTypes.ts — Driver union coverage (runtime proxy)", () => {
  it("DRIVERS array accepts 'wasl-claude' as an id value (union extended)", () => {
    // RED in practice: DRIVERS.find returns undefined until the code is added
    const ids = DRIVERS.map((d) => d.id);
    expect(ids).toContain("wasl-claude");
  });

  it("DRIVERS array accepts 'wasl-opencode' as an id value (union extended)", () => {
    const ids = DRIVERS.map((d) => d.id);
    expect(ids).toContain("wasl-opencode");
  });
});
