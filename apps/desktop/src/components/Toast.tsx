import { useEffect, useState } from "react";

export interface ToastData {
  icon: string;
  text: string;
  id: number;
}

interface ToastItemProps {
  toast: ToastData;
  onDismiss: (id: number) => void;
}

function ToastItem({ toast, onDismiss }: ToastItemProps) {
  useEffect(() => {
    const t = setTimeout(() => onDismiss(toast.id), 4000);
    return () => clearTimeout(t);
  }, [toast.id, onDismiss]);

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        padding: "12px 16px",
        background: "var(--elev)",
        border: "1px solid var(--border2)",
        borderRadius: 12,
        boxShadow: "var(--shadow)",
        fontSize: 13,
        color: "var(--txt)",
        animation: "slideup 0.2s ease",
        minWidth: 260,
        maxWidth: 380,
        cursor: "pointer",
      }}
      onClick={() => onDismiss(toast.id)}
    >
      <span style={{ fontSize: 16 }}>{toast.icon}</span>
      <span style={{ flex: 1 }}>{toast.text}</span>
    </div>
  );
}

interface ToastContainerProps {
  toasts: ToastData[];
  onDismiss: (id: number) => void;
}

/** Fixed toast stack in the bottom-right corner. */
export function ToastContainer({ toasts, onDismiss }: ToastContainerProps) {
  return (
    <div
      style={{
        position: "fixed",
        bottom: 24,
        right: 24,
        zIndex: 200,
        display: "flex",
        flexDirection: "column",
        gap: 8,
        alignItems: "flex-end",
      }}
    >
      {toasts.map((t) => (
        <ToastItem key={t.id} toast={t} onDismiss={onDismiss} />
      ))}
    </div>
  );
}

/** Hook to manage toast state. */
export function useToasts() {
  const [toasts, setToasts] = useState<ToastData[]>([]);
  let nextId = 0;

  function addToast(icon: string, text: string) {
    const id = ++nextId;
    setToasts((prev) => [...prev, { icon, text, id }]);
  }

  function dismissToast(id: number) {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }

  return { toasts, addToast, dismissToast };
}
