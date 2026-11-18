export function PageHeader({ title, subtitle, summary, actions }: { title: string; subtitle?: string; summary?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <div className="ph">
      <div style={{ display: "flex", flexDirection: "column", gap: 1, marginRight: 8 }}>
        <h1 className="h1">{title}</h1>
        {subtitle ? <span className="faint" style={{ fontSize: 12 }}>{subtitle}</span> : null}
      </div>
      {summary ? <div className="summary">{summary}</div> : null}
      <span className="sp" />
      {actions}
    </div>
  );
}
