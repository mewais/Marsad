import type { ExportRequest, ExportsEvent, ExportSource, ExportStarted, ExportStatus, LinkState } from "../wire";
import { err } from "./runtime";

/** What the fixture hub needs to know of the fleet when it exports. */
export interface ExportHost {
  id: string;
  /** The link is up: the host's log can be read. */
  up: boolean;
  link: LinkState | null;
}

export interface ExportHooks {
  now(): number;
  hosts(): ExportHost[];
  /** Run `fn` after `ms` (scaled by the fixture's speed). */
  after(ms: number, fn: () => void): void;
}

/** Why a host's log could not be read, as the hub would say it (from its link). */
function notConnected(link: LinkState | null): string {
  switch (link?.state) {
    case "reconnecting": return `the link is reconnecting (attempt ${link.attempt}): ${link.last_error}`;
    case "auth_needed": return `the host is waiting for you to sign in: ${link.detail}`;
    case "provisioning": return "the host's helper is being installed or updated";
    default: return "the link is not up";
  }
}

/**
 * Fixture-only: the hub's diagnostics exports (`diagnostics.export.start`, topic `diagnostics.exports`). A
 * started export plays a scripted progression, one step at a time: the app and the computer are read, a
 * connected host is read only in part (its budget ran out), every host that is not connected is skipped
 * with its reason, and the file is written, incomplete for that reason. Tests may play the steps by hand
 * (`manual`, `step`) and put exports on the hub before the app asks (`list`).
 */
export class FixtureExports {
  /** The exports the hub knows, in the order it reports them. */
  list: ExportStatus[] = [];
  /** Steps are played by `step()` only (a test looks at each state), not by themselves. */
  manual = false;
  /** Makes the next `start` fail with this error (a hub that refuses). */
  refuse: { code: string; message: string } | null = null;
  private subs = new Set<(e: ExportsEvent) => void>();
  private steps: (() => void)[] = [];
  private n = 0;

  constructor(private h: ExportHooks) {}

  /** The subscription: `reset` is delivered by the caller (after the link allows it); changes follow. */
  attach(onEvent: (e: ExportsEvent) => void): () => void {
    this.subs.add(onEvent);
    return () => this.subs.delete(onEvent);
  }

  reset(): ExportsEvent {
    return { kind: "reset", exports: structuredClone(this.list) };
  }

  /** Tell every subscriber about export `x` (after a test changed it). */
  upsert(x: ExportStatus) {
    this.list = this.list.some((y) => y.id === x.id) ? this.list.map((y) => (y.id === x.id ? x : y)) : [...this.list, x];
    for (const s of [...this.subs]) s({ kind: "upsert", export: structuredClone(x) });
  }

  start(r: ExportRequest): ExportStarted {
    if (this.refuse) throw err(this.refuse.code, this.refuse.message);
    const running = this.list.find((x) => x.state === "running");
    if (running) throw err("conflict", `an export is already running (${running.id}); wait for it to end`);
    const now = this.h.now();
    const id = `fx${String(++this.n).padStart(3, "0")}`;
    const hosts = this.h.hosts();
    const sources: ExportSource[] = [{ source: "app", state: "waiting", bytes: 0 }, ...hosts.map((h): ExportSource => ({ source: `host:${h.id}`, host: h.id, state: "waiting", bytes: 0 }))];
    const x: ExportStatus = { id, state: "running", since_ms: r.since_ms ?? now - 24 * 3_600_000, started_ms: now, sources };
    this.list = [...this.list, x];
    // The script: read each connected source in turn, one host only in part, the others are not connected.
    const set = (source: string, p: Partial<ExportSource>) => () => this.upsert({ ...this.get(id), sources: this.get(id).sources.map((s) => (s.source === source ? { ...s, ...p } : s)) });
    const up = hosts.filter((h) => h.up);
    const down = hosts.filter((h) => !h.up);
    const steps: (() => void)[] = [set("app", { state: "reading", bytes: 4_200 })];
    steps.push(set("app", { state: "done", bytes: 18_400 }));
    up.forEach((h, i) => {
      const src = `host:${h.id}`;
      steps.push(set(src, { state: "reading", bytes: 6_000 }));
      // The last connected host runs out of its budget; the others are read whole.
      steps.push(i === up.length - 1 && up.length > 1
        ? set(src, { state: "partial", bytes: 52_000, reason: "the host's read budget ran out: 3 older sections were left out" })
        : set(src, { state: "done", bytes: 31_000 + i * 7_000 }));
    });
    for (const h of down) steps.push(set(`host:${h.id}`, { state: "not_connected", reason: notConnected(h.link) }));
    steps.push(() => {
      const cur = this.get(id);
      const complete = cur.sources.every((s) => s.state === "done");
      this.upsert({ ...cur, state: "done", file: `marsad-diagnostics-${hosts[0]?.id ?? "hub"}-${id}.ndjson.gz`, bytes: Math.round(cur.sources.reduce((a, s) => a + s.bytes, 0) / 6), complete });
    });
    this.steps = steps;
    if (!this.manual) this.play();
    for (const s of [...this.subs]) s({ kind: "upsert", export: structuredClone(x) });
    return { id };
  }

  /** Play the next step of the running export; false when there is none. */
  step(): boolean {
    const next = this.steps.shift();
    next?.();
    return !!next;
  }

  /** End the running export as failed, with `error`. */
  fail(error: string) {
    const x = this.list.find((y) => y.state === "running");
    if (!x) return;
    this.steps = [];
    this.upsert({ ...x, state: "failed", error });
  }

  private play() {
    if (!this.steps.length) return;
    this.h.after(500, () => {
      if (this.manual) return;
      this.step();
      this.play();
    });
  }

  private get(id: string): ExportStatus {
    return this.list.find((x) => x.id === id)!;
  }
}
