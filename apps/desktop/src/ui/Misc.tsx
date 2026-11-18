import { Icon, type IconName } from "./Icon";

export function Card({ children, className = "", ...rest }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={`card ${className}`} {...rest}>
      {children}
    </div>
  );
}

export function EmptyState({ icon = "inbox", title, children, action }: { icon?: IconName; title: string; children?: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="empty" role="status">
      <Icon name={icon} />
      <h3>{title}</h3>
      {children ? <p>{children}</p> : null}
      {action}
    </div>
  );
}

export function Skeleton({ width = "100%", height }: { width?: number | string; height?: number }) {
  return <span className="skel" aria-hidden="true" style={{ width, ...(height ? { height } : {}) }} />;
}
