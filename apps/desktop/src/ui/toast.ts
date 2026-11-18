import { createStore } from "../lib/store";

export type ToastTone = "ok" | "warn" | "err" | "info";

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface ToastItem {
  id: number;
  tone: ToastTone;
  title: string;
  detail?: string;
  actions?: ToastAction[];
  /** ms until auto-dismiss; 0 = sticky. */
  ttl: number;
}

export const toastStore = createStore<ToastItem[]>([]);
let seq = 0;

export function toast(t: Omit<ToastItem, "id" | "ttl"> & { ttl?: number }): number {
  const id = ++seq;
  const ttl = t.ttl ?? (t.tone === "err" ? 0 : 5000);
  toastStore.set((l) => [...l.slice(-4), { ...t, id, ttl }]);
  return id;
}

export function dismissToast(id: number) {
  toastStore.set((l) => l.filter((t) => t.id !== id));
}
