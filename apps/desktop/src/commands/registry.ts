import { createStore } from "../lib/store";
import type { IconName } from "../ui/Icon";

export interface ActionDef {
  id: string;
  title: string;
  section: string;
  icon?: IconName;
  keywords?: string;
  /** Pretty shortcut shown on the right (display only; the binding lives in the shortcut registry). */
  shortcut?: string;
  run: () => void;
}

export const actionStore = createStore<ActionDef[]>([]);

export function registerAction(a: ActionDef): () => void {
  actionStore.set((l) => [...l.filter((x) => x.id !== a.id), a]);
  return () => actionStore.set((l) => l.filter((x) => x !== a));
}

export function searchActions(list: ActionDef[], q: string): ActionDef[] {
  const n = q.trim().toLowerCase();
  if (!n) return list;
  return list.filter((a) => `${a.title} ${a.section} ${a.keywords ?? ""}`.toLowerCase().includes(n));
}
