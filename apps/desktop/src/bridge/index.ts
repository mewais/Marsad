import type { Backend } from "./backend";
import { FixtureBackend } from "./fixture/FixtureBackend";
import { applyBackground } from "./fixture/background";
import { applyEarlier } from "./fixture/earlier";
import { applyHuge } from "./fixture/huge";
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
  // ?background=1 adds a session that works in the background (subagents and a shell command that outlive its turn).
  if (q.get("background") === "1") applyBackground();
  // ?earlier=1 adds three resumed sessions: one whose earlier messages arrived whole, one still receiving them, one whose history cannot be had.
  if (q.get("earlier") === "1") applyEarlier();
  // ?huge=1 adds a session with thousands of transcript rows (virtualised-transcript scroll demo).
  if (q.get("huge") === "1") applyHuge();
  const backend = new FixtureBackend({
    live: q.get("live") !== "0",
    latencyMs: q.has("latency") ? Number(q.get("latency")) : undefined,
    speed: q.has("speed") ? Number(q.get("speed")) : undefined,
  });
  // ?developer=1 turns Developer mode on (the Diagnostics panels), as the hub's settings would.
  if (q.get("developer") === "1") backend.hub.settings["developer.mode"] = true;
  // ?hub=connecting|reconnecting|offline (&hub_error=...) shows the app without its link to the local daemon.
  const hub = q.get("hub");
  if (hub === "connecting" || hub === "reconnecting" || hub === "offline") backend.setHubLink({ state: hub, ...(q.get("hub_error") ? { error: q.get("hub_error")! } : {}) });
  // ?connecting=1 (or ?connecting=<stage>) sits pi-runner in a connecting stage (default: long helper upload).
  const conn = q.get("connecting");
  if (conn) {
    const stage = conn === "1" ? "Sending the Marsad helper to pi-runner (3145728 of 9437184 bytes)" : conn;
    backend.setLink("pi", { state: "connecting", stage, since_ms: backend.opts.now() - 42_000 });
  }
  // ?waiting=2 puts a second host (hetzner-ax41) into the held sign-in state (for multi-host SignInStrip checks).
  if (q.get("waiting") === "2") {
    backend.markInteractive("hz");
    backend.dropLink("hz", "connection reset by peer");
  }
  // ?auth=secret|code|info|confirm|1 sits gpu-box in an active sign-in prompt sequence (during connecting).
  const auth = q.get("auth");
  if (auth) {
    const kind = auth === "code" ? "code" : auth === "info" ? "info" : auth === "confirm" ? "confirm" : "secret";
    const prompt =
      kind === "code" ? "Verification code: "
      : kind === "info" ? "Duo Push sent to “iPhone 15”. Approve the request on your device to continue."
      : kind === "confirm" ? "The authenticity of host 'gpu-box (10.0.4.21)' can't be established.\nAre you sure you want to continue connecting (yes/no)?"
      : "ml@10.0.4.21's password: ";
    backend.setLink("gpu", { state: "connecting", stage: "signing in to the host (opening the shared ssh connection)", since_ms: backend.opts.now() - 18_000 });
    backend.setAuthPrompt("gpu", { prompt, kind, attempt: 1 });
  }
  return backend;
}
