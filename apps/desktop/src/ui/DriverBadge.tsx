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

export function makeDriver(id: string, displayName: string, slot: number): DriverDescriptor {
  return { id, displayName, initials: deriveInitials(displayName), slot: slot % 8 };
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
