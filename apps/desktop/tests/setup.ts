import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
});

// Sandbox-only: Node's undici fires a Wasm OOM rejection under RLIMIT_AS; unrelated to the code.
if (typeof process !== "undefined") {
  process.on("unhandledRejection", (reason) => {
    const msg = String(reason);
    if (msg.includes("lazyllhttp") || msg.includes("WebAssembly.instantiate")) return;
    throw reason;
  });
}
