import type { Backend } from "./backend";
import { FixtureBackend } from "./fixture/FixtureBackend";
import { applyLongStrings } from "./fixture/long";
import { applyMedia } from "./fixture/media";
import { TauriBackend } from "./tauri";

export * from "./backend";
export * from "./wire";
export * from "./api";

/** Tauri when its runtime is present, otherwise the in-memory fixture (browser dev, Playwright). */
export function createBackend(search = typeof location !== "undefined" ? location.search : ""): Backend {
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) return new TauriBackend();
  const q = new URLSearchParams(search);
  // ?live=0 freezes background chatter for screenshots and e2e; ?latency=ms and ?speed=n tune the script.
  // ?long=1 swaps in long paths, names and messages (overflow checks).
  if (q.get("long") === "1") applyLongStrings();
  // ?media=1 adds a session whose transcript shows images and files.
  if (q.get("media") === "1") applyMedia();
  return new FixtureBackend({
    live: q.get("live") !== "0",
    latencyMs: q.has("latency") ? Number(q.get("latency")) : undefined,
    speed: q.has("speed") ? Number(q.get("speed")) : undefined,
  });
}
