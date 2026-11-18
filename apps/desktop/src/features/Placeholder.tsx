import { useState } from "react";
import { applyTheme, loadTheme, type ThemeChoice } from "../lib/theme";
import { PageHeader } from "../shell/PageHeader";
import { EmptyState, Segmented, type IconName } from "../ui";

/** Honest placeholder for screens that arrive in later rounds. Settings carries the theme switch. */
export function Placeholder({ title, icon, settings }: { title: string; icon: IconName; settings?: boolean }) {
  const [theme, setTheme] = useState<ThemeChoice>(loadTheme);
  return (
    <>
      <PageHeader title={title} />
      {settings ? (
        <div style={{ padding: "18px 24px", display: "grid", gap: 8 }}>
          <span className="h3">Theme</span>
          <Segmented<ThemeChoice> label="Theme" value={theme} onChange={(t) => { setTheme(t); applyTheme(t); }} options={[{ value: "system", label: "System" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} />
        </div>
      ) : null}
      <div className="placeholder">
        <EmptyState icon={icon} title={`${title} is not built yet`}>This screen is planned for a later round; the foundation covers the Inbox and Session views.</EmptyState>
      </div>
    </>
  );
}
