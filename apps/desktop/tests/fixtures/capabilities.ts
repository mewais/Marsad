/**
 * Test-only mirrors of the U1 `marsad-proto` capability types
 * (docs/specs/dynamic-capabilities.md §1), plus builders for the future
 * `useVm` `capabilities` slice (§3.6: "TS: `VmUpdateEvent` union + `useVm`
 * store gain a `capabilities` slice keyed `(driver, host_id)`").
 *
 * These are declared here, NOT imported from product code, because the TS
 * mirror (`CapabilitiesView`, the `useVm` slice, `api.ts`'s
 * `fetchCapabilities`/`refreshCapabilities`) is U2 work that has not landed
 * yet. Shapes below are copied verbatim from the already-landed Rust types
 * in `crates/marsad-proto/src/lib.rs` (§1 of the spec) so that once U2 lands
 * a real `CapabilitiesView` mirror, these fixtures describe exactly the JSON
 * shape it must produce (`From<Capabilities> for CapabilitiesView` is a pure
 * field mapping per §3.6 — no renaming).
 *
 * This file is a fixture, not product source: it ships no logic that any
 * screen imports today, only shapes + builders consumed by the U2 contract
 * tests in this directory.
 */

// ---------------------------------------------------------------------------
// Wire/view shapes (mirrors `Capabilities`/`ModelCatalog`/`DiscoveredSession`/
// `SessionScope` — dynamic-capabilities.md §1 and the golden JSON in §2)
// ---------------------------------------------------------------------------

export type ModelListSourceFixture = "Listed" | "Observed";

export interface ModelInfoFixture {
  /** Verbatim `--model` id — never normalized (§1: "agy model names contain
   *  spaces/parens"). */
  id: string;
  annotation?: string | null;
}

export interface ModelCatalogFixture {
  models: ModelInfoFixture[];
  source: ModelListSourceFixture;
  /** UI must additionally offer a free-text model field when true. */
  free_text: boolean;
}

export interface DiscoveredSessionFixture {
  id: string;
  project?: string | null;
  snippet: string;
  modified_ms: number;
}

/** Serializes as the bare string `"AllKnown"` or `{"Projects": [...]}`
 *  (golden JSON, dynamic-capabilities.md §2). */
export type SessionScopeFixture = "AllKnown" | { Projects: string[] };

export interface CapabilitiesViewFixture {
  driver: string;
  host_id: string;
  cli_available: boolean;
  /** Whether Marsad has a launch driver for this driver (claude/codex → true,
   *  gemini → false). Defaults to true in `makeCapabilities`. */
  launchable: boolean;
  models: ModelCatalogFixture;
  session_scope: SessionScopeFixture | null;
  sessions: DiscoveredSessionFixture[];
  /** Unix epoch ms; 0 = never successfully probed (§3.3). */
  fetched_at_ms: number;
  errors: string[];
}

/** The honest empty state (mirrors `ModelCatalog::default()`, proto §1). */
export function emptyModelCatalog(): ModelCatalogFixture {
  return { models: [], source: "Observed", free_text: true };
}

export function makeCapabilities(
  overrides: Partial<CapabilitiesViewFixture> & { driver: string; host_id: string }
): CapabilitiesViewFixture {
  return {
    cli_available: true,
    launchable: true,
    models: emptyModelCatalog(),
    session_scope: null,
    sessions: [],
    fetched_at_ms: 0,
    errors: [],
    ...overrides,
  };
}

/** Key format for the future `capabilities[(driver, host_id)]` VM slice. */
export function capsKey(driver: string, hostId: string): string {
  return `${driver}::${hostId}`;
}

export function capsMap(
  list: CapabilitiesViewFixture[]
): Record<string, CapabilitiesViewFixture> {
  const out: Record<string, CapabilitiesViewFixture> = {};
  for (const c of list) out[capsKey(c.driver, c.host_id)] = c;
  return out;
}

/** The future `VmUpdateEvent` arm for a `CapabilitiesUpdate` broadcast
 *  (dynamic-capabilities.md §2/§3.6). Not in the real `VmUpdateEvent` union
 *  yet — cast at the call site, same pattern as `useVmWorkstreams.test.tsx`
 *  uses for pre-landing board/workstreams events. */
export function capabilitiesUpdateEvent(capabilities: CapabilitiesViewFixture) {
  return { kind: "capabilities", capabilities } as const;
}
