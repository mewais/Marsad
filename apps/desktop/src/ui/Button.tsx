import { forwardRef } from "react";
import { Icon, type IconName } from "./Icon";
import { Kbd } from "./Kbd";
import { Menu, type MenuItem } from "./Menu";

export type ButtonVariant = "default" | "primary" | "ghost" | "allow" | "always" | "deny" | "warn";

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: "sm" | "md";
  icon?: IconName;
  kbd?: string;
  /** Request in flight: disables the button, shows a spinner, sets aria-busy. */
  pending?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "default", size = "md", icon, kbd, pending, className, children, disabled, type = "button", ...rest },
  ref,
) {
  const cls = ["btn", variant === "default" ? "" : variant, size === "sm" ? "sm" : "", className ?? ""].filter(Boolean).join(" ");
  return (
    <button ref={ref} type={type} className={cls} disabled={disabled || pending} aria-busy={pending || undefined} {...rest}>
      {pending ? <Icon name="working" size="xs" spin /> : icon ? <Icon name={icon} size="xs" /> : null}
      {children}
      {kbd ? <Kbd>{kbd}</Kbd> : null}
    </button>
  );
});

export interface IconButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  icon: IconName;
  /** Required: icon-only buttons need an accessible name. */
  label: string;
  badge?: number;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { icon, label, badge, className, type = "button", ...rest },
  ref,
) {
  return (
    <button ref={ref} type={type} className={`iconbtn ${className ?? ""}`} aria-label={label} title={label} {...rest}>
      <Icon name={icon} size="sm" />
      {badge ? <span className="dotn" aria-hidden="true">{badge}</span> : null}
    </button>
  );
});

export interface SplitButtonProps {
  label: string;
  icon?: IconName;
  kbd?: string;
  onClick: () => void;
  menuLabel: string;
  items: MenuItem[];
  variant?: ButtonVariant;
}

/** Primary action plus a caret that opens a menu of alternatives. */
export function SplitButton({ label, icon, kbd, onClick, menuLabel, items, variant = "primary" }: SplitButtonProps) {
  return (
    <div className="split">
      <Button variant={variant} size="sm" icon={icon} kbd={kbd} onClick={onClick}>
        {label}
      </Button>
      <Menu
        label={menuLabel}
        items={items}
        trigger={({ props }) => (
          <button type="button" className={`btn ${variant} sm caret`} title={menuLabel} {...props}>
            <Icon name="chev" size="sm" />
            <span className="sr-only">{menuLabel}</span>
          </button>
        )}
      />
    </div>
  );
}
