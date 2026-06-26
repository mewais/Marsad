/**
 * Watchtower — the Marsad mark (مرصد = observatory / watchtower).
 *
 * A fire-lookout silhouette: peaked roof, lookout cabin, splayed legs with a
 * cross-brace, and a base bar. Uses `currentColor`, so the parent sets the fill
 * (white inside the accent-gradient badge; matches the app/Windows icon).
 */
export function Watchtower({ size = 16 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
    >
      {/* peaked roof */}
      <path d="M12 1.6 L20.2 8.2 H3.8 Z" />
      {/* lookout cabin */}
      <rect x="6.3" y="8.2" width="11.4" height="4.4" rx="0.7" />
      {/* tower legs */}
      <path d="M8 12.6 L9.8 12.6 L7.1 22 L4.6 22 Z" />
      <path d="M14.2 12.6 L16 12.6 L19.4 22 L16.9 22 Z" />
      {/* cross brace */}
      <rect x="7" y="16.2" width="10" height="1.6" rx="0.5" />
      {/* base bar */}
      <rect x="4.2" y="21.9" width="15.6" height="1.5" rx="0.5" />
    </svg>
  );
}
