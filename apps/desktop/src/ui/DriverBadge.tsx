export interface DriverDescriptor {
  id: string;
  displayName: string;
  /** Two letters shown in the badge. */
  initials: string;
  /** Colour slot (0..7) into the --drv-N tokens. */
  slot: number;
}

export function deriveInitials(displayName: string): string {
  const words = displayName
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .split(/[\s_-]+/)
    .filter(Boolean);
  if (words.length >= 2) return (words[0][0] + words[1][0]).toUpperCase();
  const w = words[0] ?? "?";
  return (w.length > 1 ? w[0] + w[w.length - 1] : w[0]).toUpperCase();
}

/** Colour slot derived deterministically from the driver id (FNV-1a; seeded so the four known CLIs get distinct slots). */
export function driverSlot(id: string): number {
  let x = (2166136261 ^ 74711) >>> 0;
  for (const c of id) {
    x ^= c.charCodeAt(0);
    x = Math.imul(x, 16777619) >>> 0;
  }
  x ^= x >>> 15;
  return x % 8;
}

/** Presentation of a driver: colour from its id, initials from the display name the inventory reports (else the id). */
export function makeDriver(id: string, displayName?: string): DriverDescriptor {
  const name = displayName || id;
  return { id, displayName: name, initials: deriveInitials(name), slot: driverSlot(id) };
}

export interface DriverBadgeProps {
  driver: DriverDescriptor;
  compact?: boolean;
}

export function DriverBadge({ driver, compact }: DriverBadgeProps) {
  return (
    <span className={`drv s${driver.slot}${compact ? " compact" : ""}`} title={driver.displayName}>
      <b aria-hidden="true">{driver.initials}</b>
      <span className={compact ? "sr-only" : undefined}>{driver.displayName}</span>
    </span>
  );
}
