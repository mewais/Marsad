/**
 * U2 — new bridge surface: api.ts wrappers for capabilities()/refreshCapabilities().
 *
 * Contract pinned (docs/specs/dynamic-capabilities.md §3.6):
 *   "New Tauri commands (async, `rename_all = "snake_case"`, per the
 *   bridge.rs:122-145 lock discipline...):
 *     - `capabilities() -> Vec<CapabilitiesView>` — lock, clone, return
 *       (initial snapshot for `useVm`, same pattern as `board`).
 *     - `refresh_capabilities(host_id?, driver?, project?)` — forwards to
 *       `Core::refresh_capabilities`, returns immediately.
 *   TS: ... `api.ts` gains `fetchCapabilities()` / `refreshCapabilities()`
 *   with the usual mock-mode branch."
 *
 * Mock-mode convention observed across every existing wrapper in
 * bridge/api.ts (`fetchBoard`, `listTemplates`, `listWslDistros`,
 * `resumeSession`, `interrupt`, ...): `if (!isTauri()) return <honest
 * mock-mode value>;` with NO throw and NO network/Tauri call. Vitest always
 * runs outside Tauri (`isTauri()` is false under happy-dom — neither
 * `__TAURI_INTERNALS__` nor `__TAURI__` exist on `window`), so this suite
 * only exercises the mock-mode branch, matching every other bridge test in
 * this directory (see tests/bridgeWorkstreams.test.ts).
 *
 * Neither `fetchCapabilities` nor `refreshCapabilities` exists on
 * `bridge/api.ts` today — every test below is RED until U2 adds them.
 */
import { describe, it, expect } from "vitest";
import * as api from "../src/bridge/api";

// Cast surface: these exports don't exist on the real module yet, so `api`
// is typed loosely here rather than importing named bindings that would
// fail to resolve at module-eval time.
const bridge = api as unknown as {
  fetchCapabilities?: (hostId?: string, driver?: string) => Promise<unknown[]>;
  refreshCapabilities?: (args?: {
    host_id?: string;
    driver?: string;
    project?: string;
  }) => Promise<void>;
};

describe("bridge/api.ts — capabilities() wrapper", () => {
  it("exports fetchCapabilities as a function", () => {
    expect(typeof bridge.fetchCapabilities).toBe("function");
  });

  it("in mock mode (no Tauri runtime), fetchCapabilities() resolves to an array without throwing", async () => {
    const result = await bridge.fetchCapabilities!();
    expect(Array.isArray(result)).toBe(true);
  });
});

describe("bridge/api.ts — refresh_capabilities() wrapper", () => {
  it("exports refreshCapabilities as a function", () => {
    expect(typeof bridge.refreshCapabilities).toBe("function");
  });

  it("in mock mode, refreshCapabilities({host_id, driver, project}) resolves without throwing (fire-and-forget, matches resumeSession()/interrupt())", async () => {
    await expect(
      bridge.refreshCapabilities!({ host_id: "local", driver: "claude", project: "~/projects/api" })
    ).resolves.toBeUndefined();
  });

  it("in mock mode, refreshCapabilities() with no args (refresh-all) also resolves without throwing", async () => {
    await expect(bridge.refreshCapabilities!()).resolves.toBeUndefined();
  });
});
