/**
 * Single source of truth for the selectable Claude models.
 *
 * Used by every model picker (New Session, session Config tab, Settings →
 * Session defaults) so labels, order, and IDs stay consistent. IDs are the
 * REAL `--model` values the agent passes to the `claude` CLI — keep them valid.
 * Ordered most-capable first to match the design.
 */
export interface ModelOption {
  /** Real `--model` id passed to the claude CLI. */
  id: string;
  /** Human label shown in the UI. */
  label: string;
  /** Short qualifier shown under/next to the label. */
  sublabel: string;
}

export const MODELS: readonly ModelOption[] = [
  { id: "claude-opus-4-8", label: "Claude Opus 4.8", sublabel: "Most capable" },
  { id: "claude-sonnet-4-6", label: "Claude Sonnet 4.6", sublabel: "Balanced" },
  { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", sublabel: "Fast · cheap" },
] as const;

/** Sensible default selection for a fresh session. */
export const DEFAULT_MODEL = "claude-sonnet-4-6";

/** Look up a model's label by id, falling back to the raw id. */
export function modelLabel(id: string): string {
  return MODELS.find((m) => m.id === id)?.label ?? id;
}
