/**
 * Canonical driver registry — the SINGLE source of truth for the set of agent
 * drivers Marsad knows about and their accent colours.
 *
 * NewSession, NodeModal and Settings each used to hardcode their own `DRIVERS`
 * array (and their own `DRIVER_COLOR` map). They drifted: opencode was added to
 * NewSession only, so the live launcher (NodeModal) and the Settings default-agent
 * picker silently dropped it. Import from here; never re-declare a local list.
 *
 * This is NOT model data (that stays capabilities-driven per
 * dynamic-capabilities.md §5) — it is the fixed enum of drivers Marsad can drive,
 * mirroring the `Driver` union in workstreamTypes.ts. Availability + models per
 * driver are still resolved from real per-host discovery at each call site.
 */
import type { Driver } from "./workstreamTypes";

/**
 * The user-pickable drivers — NATIVE only. Whether an agent actually runs over
 * the Wasl transport is decided by the WORKSTREAM TYPE, not by the driver the
 * user picks (a single-session workstream launches native; a "Build it out"
 * multi-agent workstream launches every agent via Wasl behind the scenes). The
 * user never picks — nor sees — a "wasl-*" driver; names are always native (#8).
 */
export const DRIVERS: { id: Driver; label: string }[] = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
  { id: "opencode", label: "opencode" },
];

/**
 * Per-driver accent colour. The theme CSS var (`--d-<driver>`, injected from
 * theme/tokens.ts) with a literal fallback so it renders even before the theme
 * mounts. `Record<Driver, string>` so adding a `Driver` forces an entry here.
 */
export const DRIVER_COLOR: Record<Driver, string> = {
  claude: "var(--d-claude, #d97757)",
  codex: "var(--d-codex, #3fb6a8)",
  opencode: "var(--d-opencode, #f0a050)",
  "wasl-claude": "var(--d-wasl-claude, #5a9fd4)",
  "wasl-opencode": "var(--d-wasl-opencode, #7ec87e)",
  "wasl-codex": "var(--d-wasl-codex, #c07fc0)",
};

/** Returns true iff the driver id is a Wasl-backed driver (starts with "wasl-"). */
export function isWaslDriver(d: Driver): boolean {
  return d.startsWith("wasl-");
}

/**
 * Maps a wasl driver id to its underlying protocol string.
 * "wasl-claude"    → "claude"
 * "wasl-opencode"  → "opencode"
 * "wasl-codex"     → "codex"
 * anything else    → null
 */
export function waslProtocol(d: Driver): "claude" | "opencode" | "codex" | null {
  if (d === "wasl-claude") return "claude";
  if (d === "wasl-opencode") return "opencode";
  if (d === "wasl-codex") return "codex";
  return null;
}
