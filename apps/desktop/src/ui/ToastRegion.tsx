import { useEffect } from "react";
import { useStore } from "../lib/store";
import { Button, IconButton } from "./Button";
import { Icon, type IconName } from "./Icon";
import { dismissToast, toastStore, type ToastItem } from "./toast";

const ICON: Record<ToastItem["tone"], IconName> = { ok: "done", warn: "warn", err: "failed", info: "bell" };

function Toast({ t }: { t: ToastItem }) {
  useEffect(() => {
    if (!t.ttl) return;
    const h = setTimeout(() => dismissToast(t.id), t.ttl);
    return () => clearTimeout(h);
  }, [t.id, t.ttl]);
  return (
    <div className={`toast ${t.tone === "info" ? "" : t.tone}`} role={t.tone === "err" ? "alert" : "status"}>
      <Icon name={ICON[t.tone]} className={t.tone === "err" ? "c-failed" : t.tone === "warn" ? "c-needs" : t.tone === "ok" ? "c-done" : ""} />
      <span className="t">{t.title}</span>
      <IconButton icon="x" label="Dismiss" onClick={() => dismissToast(t.id)} />
      {t.detail ? <span className="d">{t.detail}</span> : null}
      {t.actions?.length ? (
        <span className="a">
          {t.actions.map((a) => (
            <Button
              key={a.label}
              size="sm"
              onClick={() => {
                a.onClick();
                dismissToast(t.id);
              }}
            >
              {a.label}
            </Button>
          ))}
        </span>
      ) : null}
    </div>
  );
}

/** Live region for results and errors of fire-and-forget actions. */
export function ToastRegion() {
  const items = useStore(toastStore, (s) => s);
  return (
    <div className="toasts" aria-live="polite" aria-label="Notifications">
      {items.map((t) => (
        <Toast key={t.id} t={t} />
      ))}
    </div>
  );
}
