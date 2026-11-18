import { cloneElement, useId, useState } from "react";

export interface TooltipProps {
  text: string;
  children: React.ReactElement<{ "aria-describedby"?: string }>;
}

/** Shows on hover and keyboard focus; wired with aria-describedby. */
export function Tooltip({ text, children }: TooltipProps) {
  const id = useId();
  const [open, setOpen] = useState(false);
  return (
    <span
      className="relwrap"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
      onFocus={() => setOpen(true)}
      onBlur={() => setOpen(false)}
    >
      {cloneElement(children, { "aria-describedby": open ? id : undefined })}
      {open ? (
        <span role="tooltip" id={id} className="tip">
          {text}
        </span>
      ) : null}
    </span>
  );
}
