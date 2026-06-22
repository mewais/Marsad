/**
 * Icon — inline SVG icons for Marsad desktop.
 * All icons are stroke-based, 16px default, currentColor, stroke-width 1.6.
 * No emoji or Unicode symbol fallback needed — these always render.
 */

import React from "react";

interface IconProps {
  size?: number;
  style?: React.CSSProperties;
}

function base(
  size: number,
  style: React.CSSProperties | undefined,
  children: React.ReactNode
) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ display: "inline-block", verticalAlign: "middle", flexShrink: 0, ...style }}
      aria-hidden
    >
      {children}
    </svg>
  );
}

/** ＋ / Plus — new session */
export function Plus({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <line x1="8" y1="2" x2="8" y2="14" />
      <line x1="2" y1="8" x2="14" y2="8" />
    </>
  ));
}

/** ▾ / ChevronDown — dropdown arrow */
export function ChevronDown({ size = 16, style }: IconProps) {
  return base(size, style, (
    <polyline points="3,5.5 8,10.5 13,5.5" />
  ));
}

/** ⌕ / Search — magnifying glass */
export function Search({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <circle cx="7" cy="7" r="4.2" />
      <line x1="10.2" y1="10.2" x2="13.5" y2="13.5" />
    </>
  ));
}

/** ⚙ / Gear — settings */
export function Gear({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <circle cx="8" cy="8" r="2.4" />
      <path d="M8 1.5v1.2M8 13.3v1.2M1.5 8h1.2M13.3 8h1.2M3.4 3.4l0.85 0.85M11.75 11.75l0.85 0.85M3.4 12.6l0.85-0.85M11.75 4.25l0.85-0.85" />
    </>
  ));
}

/** ☀ / Sun — light theme */
export function Sun({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <circle cx="8" cy="8" r="3" />
      <line x1="8" y1="1" x2="8" y2="2.5" />
      <line x1="8" y1="13.5" x2="8" y2="15" />
      <line x1="1" y1="8" x2="2.5" y2="8" />
      <line x1="13.5" y1="8" x2="15" y2="8" />
      <line x1="3.05" y1="3.05" x2="4.1" y2="4.1" />
      <line x1="11.9" y1="11.9" x2="12.95" y2="12.95" />
      <line x1="12.95" y1="3.05" x2="11.9" y2="4.1" />
      <line x1="4.1" y1="11.9" x2="3.05" y2="12.95" />
    </>
  ));
}

/** ☾ / Moon — dark theme */
export function Moon({ size = 16, style }: IconProps) {
  return base(size, style, (
    <path d="M13 10.5A6 6 0 0 1 5.5 3a6 6 0 1 0 7.5 7.5z" />
  ));
}

/** ▦ / Grid — overview / dashboard */
export function Grid({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <rect x="2" y="2" width="5" height="5" rx="1" />
      <rect x="9" y="2" width="5" height="5" rx="1" />
      <rect x="2" y="9" width="5" height="5" rx="1" />
      <rect x="9" y="9" width="5" height="5" rx="1" />
    </>
  ));
}

/** ⛶ / Layers — workspace icon */
export function Layers({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <polygon points="8,1.5 14.5,5 8,8.5 1.5,5" />
      <polyline points="1.5,9 8,12.5 14.5,9" />
      <polyline points="1.5,12 8,15.5 14.5,12" />
    </>
  ));
}

/** ⏸ / Pause — stop/pause session */
export function Pause({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <rect x="3" y="2.5" width="3.5" height="11" rx="1" fill="currentColor" stroke="none" />
      <rect x="9.5" y="2.5" width="3.5" height="11" rx="1" fill="currentColor" stroke="none" />
    </>
  ));
}

/** ↻ / Restart — circular arrow */
export function Restart({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9" />
      <polyline points="10.5,1.5 12,4.5 9,4.5" />
    </>
  ));
}

/** ⎇ / Branch — git branch */
export function Branch({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <circle cx="5" cy="4" r="1.6" />
      <circle cx="5" cy="12" r="1.6" />
      <circle cx="11" cy="4" r="1.6" />
      <line x1="5" y1="5.6" x2="5" y2="10.4" />
      <path d="M5 5.6 C5 8.5 11 7 11 5.6" />
    </>
  ));
}

/** ✕ / Close — X close button */
export function Close({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <line x1="3" y1="3" x2="13" y2="13" />
      <line x1="13" y1="3" x2="3" y2="13" />
    </>
  ));
}

/** ⎋ / Escape — Esc key outline icon */
export function Escape({ size = 16, style }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ display: "inline-block", verticalAlign: "middle", flexShrink: 0, ...style }}
      aria-hidden
    >
      {/* Esc key outline */}
      <rect x="1.5" y="4" width="13" height="8" rx="1.5" />
      {/* "E" */}
      <line x1="4" y1="6.5" x2="4" y2="9.5" />
      <line x1="4" y1="6.5" x2="6" y2="6.5" />
      <line x1="4" y1="8" x2="5.5" y2="8" />
      <line x1="4" y1="9.5" x2="6" y2="9.5" />
      {/* "s" simplified */}
      <path d="M7 7.2 C7 7.2 9 6.8 9 8 C9 9.2 7 8.8 7 10 C7 10 9 9.6 9 9.5" />
      {/* "c" */}
      <path d="M12 7.2 C11 6.8 10 7.5 10 8 C10 8.5 11 9.2 12 8.8" />
    </svg>
  );
}

/** ⎈ / Terminal — helm/prompt indicator */
export function Terminal({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <polyline points="2,4.5 7,8 2,11.5" />
      <line x1="8.5" y1="11.5" x2="14" y2="11.5" />
    </>
  ));
}

/** ❯ / Caret / Prompt — terminal prompt arrow */
export function PromptCaret({ size = 16, style }: IconProps) {
  return base(size, style, (
    <polyline points="5,3 11,8 5,13" />
  ));
}

/** ▲ / Triangle — alert/warning caret (filled) */
export function Triangle({ size = 16, style }: IconProps) {
  return base(size, style, (
    <polygon points="8,2.5 14.5,13.5 1.5,13.5" fill="currentColor" stroke="none" />
  ));
}

/** Edit/Diff marker — replaces ⊟ */
export function EditDiff({ size = 16, style }: IconProps) {
  return base(size, style, (
    <>
      <rect x="2" y="2" width="12" height="12" rx="2" />
      <line x1="5" y1="8" x2="11" y2="8" />
      <line x1="8" y1="5" x2="8" y2="8" />
    </>
  ));
}
