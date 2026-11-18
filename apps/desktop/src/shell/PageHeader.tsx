export function PageHeader({ title, subtitle, summary, actions }: { title: string; subtitle?: string; summary?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <div className="ph">
      <div className="ph-sub">
        <h1 className="h1">{title}</h1>
        {subtitle ? <span className="faint">{subtitle}</span> : null}
      </div>
      {summary ? <div className="summary">{summary}</div> : null}
      <span className="sp" />
      {actions}
    </div>
  );
}
