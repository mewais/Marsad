import { ICON_PATHS, type IconName } from "./iconPaths";

export type { IconName };

export interface IconProps {
  name: IconName;
  size?: "xs" | "sm" | "md" | "lg";
  spin?: boolean;
  className?: string;
  /** Decorative by default; pass a title to expose it to assistive tech. */
  title?: string;
}

export function Icon({ name, size = "md", spin, className, title }: IconProps) {
  const cls = ["i", size === "md" ? "" : size, spin ? "spin" : "", className ?? ""].filter(Boolean).join(" ");
  return (
    <svg className={cls} aria-hidden={title ? undefined : true} role={title ? "img" : undefined} aria-label={title}>
      <use href={`#i-${name}`} />
    </svg>
  );
}

/** Render once near the root; every <Icon> references its symbols. */
export function IconSprite() {
  return (
    <svg width="0" height="0" className="abs0" aria-hidden="true" focusable="false">
      <defs>
        {(Object.keys(ICON_PATHS) as IconName[]).map((n) => (
          <symbol key={n} id={`i-${n}`} viewBox="0 0 24 24" dangerouslySetInnerHTML={{ __html: ICON_PATHS[n] }} />
        ))}
      </defs>
    </svg>
  );
}
