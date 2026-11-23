import type { Api } from "../bridge/api";
import { errorMessage } from "../bridge/api";
import type { ExportStatus, ExportsEvent, UiEvent } from "../bridge/wire";
import { codeOf } from "./diaglog";
import type { Feeds } from "./feeds";
import type { AppStore, ExportSave, ExportsSlice } from "./model";
import { reachBlock } from "./selectors";

/** The export the card is about: the one running, else the one that began last. */
export function currentExport(list: readonly ExportStatus[]): ExportStatus | undefined {
  return list.find((x) => x.state === "running") ?? [...list].sort((a, b) => b.started_ms - a.started_ms)[0];
}

/** Pure: apply one `diagnostics.exports` event (`reset` replaces everything, `upsert` one export by its id). */
export function applyExports(list: readonly ExportStatus[], e: ExportsEvent): ExportStatus[] {
  if (e?.kind === "reset") return [...e.exports];
  if (e?.kind === "upsert") return list.some((x) => x.id === e.export.id) ? list.map((x) => (x.id === e.export.id ? e.export : x)) : [...list, e.export];
  return [...list];
}

/**
 * The hub's diagnostics exports for the Settings card: the topic `diagnostics.exports` (through the
 * feed tracking, so an ended subscription is shown and can be started again), held only while the
 * card is open, and the card's two acts: starting an export and saving a finished one. Neither waits:
 * what the card shows is what the hub reported, and an act is pending until the hub (or the shell) answers.
 * The acts are recorded in the app's log as `ui.export`.
 */
export function createExports(store: AppStore, api: Api, feeds: Feeds, log: (e: UiEvent) => void) {
  let holds = 0;
  let off: () => void = () => undefined;

  const patch = (p: (x: ExportsSlice) => Partial<ExportsSlice>) =>
    store.set((s) => (s.exports ? { ...s, exports: { ...s.exports, ...p(s.exports) } } : s));

  const apply = (e: ExportsEvent) =>
    patch((x) => {
      const list = applyExports(x.list, e);
      const running = list.some((y) => y.state === "running");
      // The export we started is on the topic now; a refusal that named a running export is over when none runs.
      const start = x.start.state === "awaiting" && list.some((y) => y.id === (x.start as { id: string }).id) ? { state: "idle" as const } : x.start.state === "refused" && !running ? { state: "idle" as const } : x.start;
      return { state: "live", list, start };
    });

  function open(): () => void {
    if (holds++ === 0) {
      store.set((s) => ({ ...s, exports: { state: "waiting", list: [], start: { state: "idle" }, saves: {} } }));
      off = feeds.open("exports", (onEnd) => api.subscribe.exports(apply, onEnd));
    }
    let released = false;
    return () => {
      if (released) return;
      released = true;
      if (--holds > 0) return;
      off();
      store.set((s) => ({ ...s, exports: null }));
    };
  }

  /** Ask the hub for an export of the time since `sinceMs` (with the trails of the sessions active in it). */
  async function start(sinceMs: number): Promise<void> {
    const cur = store.get().exports;
    if (!cur || cur.start.state === "starting" || cur.start.state === "awaiting" || cur.list.some((x) => x.state === "running")) return;
    const blocked = reachBlock(store.get());
    if (blocked) return patch(() => ({ start: { state: "refused", reason: blocked } }));
    patch(() => ({ start: { state: "starting" } }));
    log({ ev: "ui.export", stage: "start" });
    try {
      const { id } = await api.startExport({ since_ms: sinceMs, include_trails: true });
      patch((x) => ({ start: x.list.some((y) => y.id === id) ? { state: "idle" } : { state: "awaiting", id } }));
    } catch (e) {
      const code = codeOf(e);
      log({ ev: "ui.export", stage: "failed", ...(code ? { code } : {}) });
      patch(() => ({ start: { state: "refused", reason: errorMessage(e) } }));
    }
  }

  /** Copy the finished export `file` to where the user chooses (the shell asks; cancelling is not a failure). */
  async function save(file: string): Promise<void> {
    const setSave = (v: ExportSave) => patch((x) => ({ saves: { ...x.saves, [file]: v } }));
    if (store.get().exports?.saves[file]?.state === "saving") return;
    setSave({ state: "saving" });
    try {
      const r = await api.desktop.exportSave(file);
      if ("path" in r) {
        log({ ev: "ui.export", stage: "saved" });
        setSave({ state: "saved", path: r.path });
      } else {
        log({ ev: "ui.export", stage: "save_cancelled" });
        setSave({ state: "cancelled" });
      }
    } catch (e) {
      const code = codeOf(e);
      log({ ev: "ui.export", stage: "failed", ...(code ? { code } : {}) });
      setSave({ state: "failed", reason: errorMessage(e) });
    }
  }

  return { open, start, save };
}
export type Exports = ReturnType<typeof createExports>;
