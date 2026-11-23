/** Running inside the desktop shell (Tauri webview) rather than a browser tab. */
export const inShell = (): boolean => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
