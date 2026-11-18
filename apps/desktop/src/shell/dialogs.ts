import { createStore } from "../lib/store";

/** App-wide dialogs that any screen, shortcut or palette action can open. */
export interface NewSessionPreset {
  host?: string;
  driver?: string;
  cwd?: string;
}
export interface DialogState {
  newSession: NewSessionPreset | null;
  newWorkstream: boolean;
}
export const dialogStore = createStore<DialogState>({ newSession: null, newWorkstream: false });
export const openNewSession = (preset: NewSessionPreset = {}) => dialogStore.set((s) => ({ ...s, newSession: preset }));
export const openNewWorkstream = () => dialogStore.set((s) => ({ ...s, newWorkstream: true }));
export const closeDialogs = () => dialogStore.set({ newSession: null, newWorkstream: false });
