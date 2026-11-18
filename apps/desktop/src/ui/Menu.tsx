import { useEffect, useRef } from "react";
import { Icon, type IconName } from "./Icon";
import { Kbd } from "./Kbd";
import { Popover, type PopoverProps } from "./Popover";

export interface MenuItem {
  id: string;
  label: string;
  description?: string;
  icon?: IconName;
  kbd?: string;
  disabled?: boolean;
  onSelect: () => void;
}

export interface MenuProps {
  label: string;
  items: MenuItem[];
  trigger: PopoverProps["trigger"];
  align?: "left" | "right";
  up?: boolean;
}

/** role=menu with roving focus (Up/Down/Home/End), Enter/Space to choose, Esc to close. */
export function Menu({ label, items, trigger, align, up }: MenuProps) {
  return (
    <Popover trigger={trigger} haspopup="menu" className="menu" align={align} up={up}>
      {({ close }) => <MenuList label={label} items={items} close={close} />}
    </Popover>
  );
}

function MenuList({ label, items, close }: { label: string; items: MenuItem[]; close: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('[role="menuitem"]:not([disabled])')?.focus();
  }, []);
  const onKeyDown = (e: React.KeyboardEvent) => {
    const els = Array.from(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])') ?? []);
    const i = els.indexOf(document.activeElement as HTMLElement);
    let n = -1;
    if (e.key === "ArrowDown") n = (i + 1) % els.length;
    else if (e.key === "ArrowUp") n = (i - 1 + els.length) % els.length;
    else if (e.key === "Home") n = 0;
    else if (e.key === "End") n = els.length - 1;
    else if (e.key === "Tab") close();
    if (n >= 0) {
      e.preventDefault();
      els[n]?.focus();
    }
  };
  return (
    <div ref={ref} role="menu" aria-label={label} style={{ display: "contents" }} onKeyDown={onKeyDown}>
      {items.map((it) => (
        <button
          key={it.id}
          type="button"
          role="menuitem"
          disabled={it.disabled}
          onClick={() => {
            close();
            it.onSelect();
          }}
        >
          {it.icon ? <Icon name={it.icon} size="sm" /> : <span />}
          <span>
            {it.label}
            {it.description ? <small>{it.description}</small> : null}
          </span>
          {it.kbd ? <Kbd>{it.kbd}</Kbd> : <span />}
        </button>
      ))}
    </div>
  );
}
