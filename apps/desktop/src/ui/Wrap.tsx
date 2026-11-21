import { Fragment } from "react";

/**
 * A path, address, version or other long token that wraps inside its box: preferably after a
 * separator (`/ \ @ . : _ -`), anywhere as a last resort. The full value is the tooltip.
 */
export function Wrap({ text, className, title }: { text: string; className?: string; title?: string }) {
  const parts = text.split(/(?<=[/\\@.:_-])/);
  return (
    <span className={className ? `wrap ${className}` : "wrap"} title={title ?? text}>
      {parts.map((p, i) => (i ? <Fragment key={i}><wbr />{p}</Fragment> : p))}
    </span>
  );
}
